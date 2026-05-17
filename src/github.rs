pub(crate) mod encoding;
pub(crate) mod format;
mod helpers;
pub(crate) mod types;

use helpers::encode_path;
pub(crate) use helpers::{
    apply_line_range, decode_content, filter_tree_entries, parse_line_range, parse_repo,
    validate_path, validate_ref,
};

use std::env;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::redacted::{Redacted, validate_https};

use types::{
    BlobResponse, ContentsResponse, IssueInfo, PullInfo, ReleaseInfo, RepoInfo, TreeResponse,
};

const API_BASE: &str = "https://api.github.com";
const TOKEN_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

use crate::retry::{
    is_transient_decode, is_transient_network, parse_retry_after, retry_after_within_cap,
    retry_with_rate_limit,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum GitHubError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error(
        "GitHub API rate limit exceeded. Set GITHUB_TOKEN or run `gh auth login` for higher limits."
    )]
    RateLimited { retry_after: Option<u64> },

    #[error("Access denied: {0}")]
    Forbidden(String),

    #[error("GitHub API error ({code}): {message}")]
    Api { code: u16, message: String },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Invalid repository format: expected 'owner/repo', got '{0}'")]
    InvalidRepo(String),

    #[error("Invalid ref: {0}")]
    InvalidRef(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Invalid line range: '{0}'. Use formats like '1-80', '50-', or '100' (first N lines).")]
    InvalidLineRange(String),

    #[error("Invalid glob pattern: {0}")]
    InvalidPattern(String),

    #[error("per_page must be <= 100 (GitHub API limit), got {0}")]
    InvalidPerPage(u8),

    #[error("Content decode error: {0}")]
    Decode(String),

    #[error("{0}")]
    NonUtf8(String),

    #[error("Insecure URL: HTTPS required for token-bearing request")]
    InsecureUrl,
}

/// HTTP client for the GitHub REST API v3.
///
/// Auth resolution order: `GITHUB_TOKEN` env → `GH_TOKEN` env → `gh auth token` CLI → unauthenticated.
/// Owner/repo parameters are safe for direct URL interpolation because `parse_repo`
/// restricts them to `[a-zA-Z0-9._-]`.
#[derive(Clone)]
pub(crate) struct GitHubClient {
    http: Client,
    token: Option<Redacted>,
    base_url: String,
    /// Test-only escape hatch for wiremock servers on `http://127.0.0.1`.
    /// Production constructors leave this `false` so `request` always runs
    /// `validate_https`; only `with_base_url` opts in.
    #[cfg(test)]
    skip_https_check: bool,
}

impl GitHubClient {
    pub async fn from_env(http: Client) -> Self {
        let token = resolve_token().await;
        if token.is_some() {
            debug!("GitHub token configured");
        } else {
            info!(
                "No GitHub token found. Rate limit: 60 req/hour. Set GITHUB_TOKEN or run `gh auth login`."
            );
        }
        Self {
            http,
            token,
            base_url: API_BASE.to_owned(),
            #[cfg(test)]
            skip_https_check: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            token: None,
            base_url: base_url.to_owned(),
            skip_https_check: true,
        }
    }

    /// Test-only override of the production HTTPS gate. See [`validate_https`].
    fn should_check_https(&self) -> bool {
        #[cfg(test)]
        {
            !self.skip_https_check
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    fn request(&self, path: &str) -> Result<reqwest::RequestBuilder, GitHubError> {
        let url = format!("{}{path}", self.base_url);
        if self.should_check_https() {
            validate_https(&url, || GitHubError::InsecureUrl)?;
        }
        let mut req = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", crate::USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(ref token) = self.token {
            req = req.header("Authorization", format!("Bearer {}", token.expose()));
        }
        Ok(req)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, GitHubError> {
        retry_with_rate_limit(
            || self.get_json_once(path),
            is_retriable,
            |e| match e {
                GitHubError::RateLimited { retry_after } => *retry_after,
                _ => None,
            },
            || GitHubError::RateLimited { retry_after: None },
        )
        .await
    }

    async fn get_json_once<T: DeserializeOwned>(&self, path: &str) -> Result<T, GitHubError> {
        debug!(path, "github API request");
        let response = self.request(path)?.send().await?;
        let status = response.status();
        debug!(path, status = %status, "github API response");
        match status.as_u16() {
            // Schema mismatch is a scout-side invariant — non-retryable.
            // Transport errors (timeout, connect, mid-stream body drop — issue
            // #113) fall through to Network so the retry loop can recover.
            200..=299 => response.json().await.map_err(|e| {
                if e.is_decode() && !is_transient_decode(&e) {
                    GitHubError::Decode(e.to_string())
                } else {
                    e.into()
                }
            }),
            404 => Err(GitHubError::NotFound(path.to_owned())),
            429 => {
                let retry_after = parse_retry_after(response.headers());
                warn!(retry_after_secs = retry_after, "GitHub API rate limited");
                Err(GitHubError::RateLimited { retry_after })
            }
            403 => {
                // ADR-0004 Rule 1: 403 + missing `x-ratelimit-remaining` defaults to
                // RateLimited (retry) because GitHub does not guarantee this header
                // on all 403 responses (e.g., secondary rate limits). Only treat as
                // Forbidden when the header is present and remaining > 0 (genuine
                // auth misconfig).
                let remaining = response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                let retry_after = secs_until_ratelimit_reset(response.headers());
                match remaining {
                    Some(r) if r > 0 => {
                        let message =
                            extract_error_message(&response.text().await.unwrap_or_default());
                        Err(GitHubError::Forbidden(message))
                    }
                    _ => {
                        warn!(
                            retry_after_secs = retry_after,
                            "GitHub 403 with missing or zero rate-limit-remaining, treating as RateLimited (ADR-0004)"
                        );
                        Err(GitHubError::RateLimited { retry_after })
                    }
                }
            }
            _ => {
                let message = extract_error_message(
                    &response
                        .text()
                        .await
                        .unwrap_or_else(|_| format!("HTTP {status}")),
                );
                Err(GitHubError::Api {
                    code: status.as_u16(),
                    message,
                })
            }
        }
    }

    pub async fn get_repo(&self, owner: &str, repo: &str) -> Result<RepoInfo, GitHubError> {
        self.get_json(&format!("/repos/{owner}/{repo}")).await
    }

    pub async fn get_tree(
        &self,
        owner: &str,
        repo: &str,
        ref_: &str,
    ) -> Result<TreeResponse, GitHubError> {
        let ref_ = encode_path(ref_);
        self.get_json(&format!(
            "/repos/{owner}/{repo}/git/trees/{ref_}?recursive=1"
        ))
        .await
    }

    pub async fn get_contents(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        ref_: Option<&str>,
    ) -> Result<ContentsResponse, GitHubError> {
        let path = encode_path(path);
        let query = ref_
            .map(|r| format!("?ref={}", encode_path(r)))
            .unwrap_or_default();
        self.get_json(&format!("/repos/{owner}/{repo}/contents/{path}{query}"))
            .await
    }

    pub async fn get_blob(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<BlobResponse, GitHubError> {
        self.get_json(&format!("/repos/{owner}/{repo}/git/blobs/{sha}"))
            .await
    }

    pub async fn get_readme(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<ContentsResponse, GitHubError> {
        self.get_json(&format!("/repos/{owner}/{repo}/readme"))
            .await
    }

    pub async fn get_issues(
        &self,
        owner: &str,
        repo: &str,
        per_page: u8,
    ) -> Result<Vec<IssueInfo>, GitHubError> {
        validate_per_page(per_page)?;
        self.get_json(&format!(
            "/repos/{owner}/{repo}/issues?state=open&sort=updated&direction=desc&per_page={per_page}"
        ))
        .await
    }

    pub async fn get_pulls(
        &self,
        owner: &str,
        repo: &str,
        per_page: u8,
    ) -> Result<Vec<PullInfo>, GitHubError> {
        validate_per_page(per_page)?;
        self.get_json(&format!(
            "/repos/{owner}/{repo}/pulls?state=open&sort=updated&direction=desc&per_page={per_page}"
        ))
        .await
    }

    pub async fn get_releases(
        &self,
        owner: &str,
        repo: &str,
        per_page: u8,
    ) -> Result<Vec<ReleaseInfo>, GitHubError> {
        validate_per_page(per_page)?;
        self.get_json(&format!(
            "/repos/{owner}/{repo}/releases?per_page={per_page}"
        ))
        .await
    }
}

/// ADR-0004 Rule 2: per_page > 100 returns explicit error rather than silent
/// clamp. GitHub API caps per_page at 100; previously the code did
/// `per_page.min(100)` silently, returning fewer results than requested with
/// no diagnostic. Now callers receive `GitHubError::InvalidPerPage` (mapped
/// to `data_error`, exit 65) and can correct the input.
fn validate_per_page(per_page: u8) -> Result<(), GitHubError> {
    if per_page > 100 {
        Err(GitHubError::InvalidPerPage(per_page))
    } else {
        Ok(())
    }
}

fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(200).collect())
}

fn is_retriable(e: &GitHubError) -> bool {
    match e {
        GitHubError::RateLimited { retry_after } => retry_after_within_cap(*retry_after),
        GitHubError::Api {
            code: 500..=599, ..
        } => true,
        GitHubError::Network(e) => is_transient_network(e),
        _ => false,
    }
}

fn secs_until_ratelimit_reset(headers: &HeaderMap) -> Option<u64> {
    let reset_ts = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(reset_ts.saturating_sub(now))
}

async fn resolve_token() -> Option<Redacted> {
    resolve_token_with(|var| env::var(var).ok()).await
}

async fn resolve_token_with(env_reader: impl Fn(&str) -> Option<String>) -> Option<Redacted> {
    let from_env = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|var| env_reader(var))
        .map(|t| t.trim().to_owned())
        .find(|t| !t.is_empty());

    if let Some(token) = from_env {
        return Some(Redacted::new(&token));
    }

    let output = timeout(
        TOKEN_RESOLVE_TIMEOUT,
        Command::new("gh")
            .args(["auth", "token"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .inspect_err(|_| {
        info!(
            "gh auth token timed out after {}s",
            TOKEN_RESOLVE_TIMEOUT.as_secs()
        )
    })
    .ok()?
    .inspect_err(|e| info!("gh auth token command failed: {e}"))
    .ok()?;

    if !output.status.success() {
        info!(
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "gh auth token failed"
        );
        return None;
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if token.is_empty() {
        None
    } else {
        Some(Redacted::new(&token))
    }
}

#[cfg(test)]
mod http_tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::retry::MAX_RETRIES;
    use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    /// [T-GH001] get_json maps 404 responses to NotFound error
    #[tokio::test]
    async fn get_json_404_returns_not_found() {
        let Some(server) = try_spawn_mock_server("github::get_json_404").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::NotFound(_))));
    }

    /// [T-GH002] get_json maps 429 responses to RateLimited error
    #[tokio::test]
    async fn get_json_429_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
    }

    /// [T-GH003] get_json maps 403 with zero remaining to RateLimited error
    #[tokio::test]
    async fn get_json_403_with_zero_remaining_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
    }

    /// [T-GH004] get_json maps 403 with non-zero remaining to Forbidden error
    #[tokio::test]
    async fn get_json_403_with_remaining_returns_forbidden() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "50")
                    .set_body_json(serde_json::json!({"message": "access denied"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::Forbidden(ref msg)) if msg == "access denied"));
    }

    /// [T-GH010] 403 without x-ratelimit-remaining header maps to RateLimited (ADR-0004 Rule 1)
    #[tokio::test]
    async fn get_json_403_with_missing_remaining_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"message": "secondary rate limit"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
    }

    /// [T-GH011] validate_per_page rejects values over 100 (ADR-0004 Rule 2)
    #[test]
    fn validate_per_page_rejects_over_100() {
        assert!(matches!(
            super::validate_per_page(101),
            Err(GitHubError::InvalidPerPage(101))
        ));
        assert!(matches!(
            super::validate_per_page(255),
            Err(GitHubError::InvalidPerPage(255))
        ));
        assert!(super::validate_per_page(100).is_ok());
        assert!(super::validate_per_page(1).is_ok());
    }

    /// [T-GH005] resolve_token_with reads token from GITHUB_TOKEN env var
    #[tokio::test]
    async fn resolve_token_reads_env_var() {
        let token = resolve_token_with(|key| {
            if key == "GITHUB_TOKEN" {
                Some("test-token-from-env".into())
            } else {
                None
            }
        })
        .await;
        assert_eq!(
            token.as_ref().map(super::super::redacted::Redacted::expose),
            Some("test-token-from-env")
        );
    }

    /// [T-GH006] get_json maps 500 responses to generic Api error
    #[tokio::test]
    async fn get_json_500_returns_api_error() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({"message": "internal server error"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<serde_json::Value, _> = client.get_json("/test").await;
        assert!(matches!(result, Err(GitHubError::Api { code: 500, .. })));
    }

    /// [T-GH007] get_json_once propagates Retry-After header value on 429
    #[tokio::test]
    async fn get_json_429_with_retry_after_carries_delay() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "30"))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
        assert!(matches!(
            result,
            Err(GitHubError::RateLimited {
                retry_after: Some(30)
            })
        ));
    }

    /// [T-GH008] get_json_once uses x-ratelimit-reset to compute delay on 403 rate limit
    #[tokio::test]
    async fn get_json_403_with_ratelimit_reset_carries_delay() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        let future_reset = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 60;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .append_header("x-ratelimit-reset", future_reset.to_string().as_str())
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
        assert!(matches!(
            result,
            Err(GitHubError::RateLimited {
                retry_after: Some(_)
            })
        ));
    }

    /// [T-GH012] 2xx response with malformed JSON classifies as Decode (issue #101).
    ///
    /// Before this fix, `Ok(response.json().await?)` routed schema failures through
    /// `#[from] reqwest::Error` to `GitHubError::Network`, surfacing as TempFailure(75)
    /// retryable=true. Schema fail is a scout-side invariant violation that retry
    /// cannot resolve — must classify as Decode → Internal(70) retryable=false.
    #[tokio::test]
    async fn get_json_2xx_malformed_body_returns_decode() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not valid json"))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(
            matches!(result, Err(GitHubError::Decode(_))),
            "expected GitHubError::Decode for 2xx malformed JSON, got: {result:?}"
        );
    }

    /// [T-GH013] 2xx mid-stream body drop is treated as transient and the
    /// retry loop exhausts MAX_RETRIES attempts before failing (issue #113).
    ///
    /// reqwest 0.13 surfaces a mid-stream drop as `is_decode() == true` with
    /// an io::Error in the source chain. Without `is_transient_decode`,
    /// every attempt would route to `GitHubError::Decode` → Internal(70)
    /// retryable=false. With it, attempts route to `GitHubError::Network` →
    /// TempFailure(75) and the retry loop kicks in.
    #[tokio::test]
    async fn get_json_2xx_mid_stream_drop_exhausts_retries() {
        let expected_attempts = usize::try_from(MAX_RETRIES).expect("MAX_RETRIES fits usize");
        let Some((url, counter, handle)) = spawn_mid_stream_drop_server(expected_attempts) else {
            return;
        };

        let client = GitHubClient::with_base_url(Client::new(), &url);
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;

        assert!(
            matches!(result, Err(GitHubError::Network(_))),
            "expected GitHubError::Network for exhausted mid-stream drop, got: {result:?}"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            expected_attempts,
            "retry loop must consume MAX_RETRIES connections"
        );

        let _ = handle.join();
    }
}
