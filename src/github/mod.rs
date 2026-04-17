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
use std::time::Duration;

use reqwest::Client;
use tracing::{debug, info, warn};

use crate::redacted::Redacted;

use types::{
    BlobResponse, ContentsResponse, IssueInfo, PullInfo, ReleaseInfo, RepoInfo, TreeResponse,
};

const API_BASE: &str = "https://api.github.com";
const TOKEN_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

use crate::retry::{
    is_transient_network, parse_retry_after, retry_after_or_backoff, retry_after_within_cap,
    retry_with,
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

    #[error("Content decode error: {0}")]
    Decode(String),

    #[error("{0}")]
    NonUtf8(String),
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
            base_url: API_BASE.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            token: None,
            base_url: base_url.to_string(),
        }
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.base_url);
        let mut req = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", crate::USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(ref token) = self.token {
            crate::redacted::assert_https(&url);
            req = req.header("Authorization", format!("Bearer {}", token.expose()));
        }
        req
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GitHubError> {
        retry_with(
            || self.get_json_once(path),
            is_retriable,
            github_delay,
            || GitHubError::RateLimited { retry_after: None },
        )
        .await
    }

    async fn get_json_once<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, GitHubError> {
        debug!(path, "github API request");
        let response = self.request(path).send().await?;
        let status = response.status();
        debug!(path, status = %status, "github API response");
        match status.as_u16() {
            200..=299 => Ok(response.json().await?),
            404 => Err(GitHubError::NotFound(path.to_string())),
            429 => {
                let retry_after = parse_retry_after(response.headers());
                warn!(retry_after_secs = retry_after, "GitHub API rate limited");
                Err(GitHubError::RateLimited { retry_after })
            }
            403 => {
                let remaining = response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                let retry_after = secs_until_ratelimit_reset(response.headers());
                if remaining == Some(0) {
                    Err(GitHubError::RateLimited { retry_after })
                } else {
                    let message = extract_error_message(&response.text().await.unwrap_or_default());
                    Err(GitHubError::Forbidden(message))
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
        let per_page = per_page.min(100);
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
        let per_page = per_page.min(100);
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
        let per_page = per_page.min(100);
        self.get_json(&format!(
            "/repos/{owner}/{repo}/releases?per_page={per_page}"
        ))
        .await
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

fn github_delay(e: &GitHubError, attempt: u32) -> Duration {
    let retry_after = match e {
        GitHubError::RateLimited { retry_after } => *retry_after,
        _ => None,
    };
    retry_after_or_backoff(retry_after, attempt)
}

fn secs_until_ratelimit_reset(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let reset_ts = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(reset_ts.saturating_sub(now))
}

async fn resolve_token() -> Option<Redacted> {
    resolve_token_with(|var| env::var(var).ok()).await
}

async fn resolve_token_with(env_reader: impl Fn(&str) -> Option<String>) -> Option<Redacted> {
    let from_env = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|var| env_reader(var))
        .map(|t| t.trim().to_string())
        .find(|t| !t.is_empty());

    if let Some(token) = from_env {
        return Some(Redacted::new(token));
    }

    let output = tokio::time::timeout(
        TOKEN_RESOLVE_TIMEOUT,
        tokio::process::Command::new("gh")
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

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(Redacted::new(token))
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::test_support::try_spawn_mock_server;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

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
            token.as_ref().map(|t| t.expose()),
            Some("test-token-from-env")
        );
    }

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

    #[tokio::test]
    async fn get_json_403_with_ratelimit_reset_carries_delay() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        let future_reset = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
}
