pub(crate) mod encoding;
pub(crate) mod format;
mod helpers;
pub(crate) mod types;

use std::fmt;
use std::sync::Arc;

use reqwest::Client;
use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use tracing::{debug, info, warn};

use crate::clock::{Clock, SystemClock};
use crate::redacted::{Redacted, validate_https};
#[cfg(test)]
use crate::retry::DEFAULT_MAX_RETRIES;
use crate::retry::{
    is_schema_decode_fail, is_transient_network, parse_retry_after, retry_after_within_cap,
    retry_with_rate_limit,
};
use crate::rng::{FastrandRng, Rng};
use crate::token_source::TokenSource;

use helpers::encode_path;
pub(crate) use helpers::{
    apply_line_range, decode_content, filter_tree_entries, parse_line_range, parse_repo,
    validate_path, validate_ref,
};
use types::{
    BlobResponse, ContentsResponse, IssueInfo, PullInfo, ReleaseInfo, RepoInfo, TreeResponse,
};

const API_BASE: &str = "https://api.github.com";

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
    max_retries: u32,
    /// Wall-clock source for `secs_until_ratelimit_reset`. Set at construction
    /// and read on every retry; defaults to `SystemClock`.
    clock: Arc<dyn Clock>,
    /// Backoff jitter source handed to `retry_with_rate_limit` per attempt.
    /// Set at construction; defaults to `FastrandRng`.
    rng: Arc<dyn Rng>,
    /// Test-only escape hatch for wiremock servers on `http://127.0.0.1`.
    /// Production constructors leave this `false` so `request` always runs
    /// `validate_https`; only `with_base_url` opts in.
    #[cfg(test)]
    skip_https_check: bool,
}

impl GitHubClient {
    /// Production constructor parameterized by the `TokenSource`. `Scout`
    /// picks `GhCliSource` by default; tests pick `StaticTokenSource(...)` to
    /// avoid spawning `gh auth token`.
    pub(crate) async fn from_env_with_source(
        http: Client,
        max_retries: u32,
        source: &dyn TokenSource,
    ) -> Self {
        let token = source.fetch().await;
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
            max_retries,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
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
            max_retries: DEFAULT_MAX_RETRIES,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            skip_https_check: true,
        }
    }

    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub(crate) fn with_rng(mut self, rng: Arc<dyn Rng>) -> Self {
        self.rng = rng;
        self
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
            self.max_retries,
            is_retriable,
            |e| match e {
                GitHubError::RateLimited { retry_after } => *retry_after,
                _ => None,
            },
            || GitHubError::RateLimited { retry_after: None },
            self.rng.as_ref(),
        )
        .await
    }

    async fn get_json_once<T: DeserializeOwned>(&self, path: &str) -> Result<T, GitHubError> {
        debug!(path, "github API request");
        let response = self.request(path)?.send().await?;
        let status = response.status();
        debug!(path, status = %status, "github API response");
        match status.as_u16() {
            // Schema fail → Decode (terminal). Transport drop / connect /
            // timeout → Network → retry loop. See `is_schema_decode_fail`
            // for the source-chain discrimination (issue #113).
            200..=299 => response.json().await.map_err(|e| {
                if is_schema_decode_fail(&e) {
                    GitHubError::Decode(e.to_string())
                } else {
                    e.into()
                }
            }),
            404 => Err(GitHubError::NotFound(path.to_owned())),
            429 => {
                let retry_after = parse_retry_after(response.headers(), self.clock.as_ref());
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
                let retry_after =
                    secs_until_ratelimit_reset(response.headers(), self.clock.as_ref());
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
        per_page: PerPage,
    ) -> Result<Vec<IssueInfo>, GitHubError> {
        self.get_json(&format!(
            "/repos/{owner}/{repo}/issues?state=open&sort=updated&direction=desc&per_page={per_page}"
        ))
        .await
    }

    pub async fn get_pulls(
        &self,
        owner: &str,
        repo: &str,
        per_page: PerPage,
    ) -> Result<Vec<PullInfo>, GitHubError> {
        self.get_json(&format!(
            "/repos/{owner}/{repo}/pulls?state=open&sort=updated&direction=desc&per_page={per_page}"
        ))
        .await
    }

    pub async fn get_releases(
        &self,
        owner: &str,
        repo: &str,
        per_page: PerPage,
    ) -> Result<Vec<ReleaseInfo>, GitHubError> {
        self.get_json(&format!(
            "/repos/{owner}/{repo}/releases?per_page={per_page}"
        ))
        .await
    }
}

/// ADR-0004 Rule 2: per_page is constrained to 1..=100 at the type level rather
/// than silently clamped (originally `per_page.min(100)`). 0 — which the GitHub
/// API treats as implementation-defined — is rejected for the same reason as
/// values over 100.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PerPage(u8);

impl PerPage {
    /// Compile-time validated constructor. The `assert!` panics at compile
    /// time when called from a `const` context with an out-of-range literal,
    /// and at runtime for non-`const` callers.
    pub const fn new(value: u8) -> Self {
        assert!(
            value >= 1 && value <= 100,
            "PerPage must be 1..=100 (GitHub API limit)"
        );
        Self(value)
    }
}

impl fmt::Display for PerPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .inspect_err(
            |e| debug!(error = %e, "GitHub error body is not JSON, falling back to truncated text"),
        )
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

fn secs_until_ratelimit_reset(headers: &HeaderMap, clock: &dyn Clock) -> Option<u64> {
    let reset_ts = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())?;
    Some(reset_ts.saturating_sub(clock.now_secs()))
}

#[cfg(test)]
mod http_tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::clock::FixedClock;
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

    /// [T-GH011a] PerPage::new accepts boundary values 1 and 100
    #[test]
    fn per_page_new_accepts_boundary_values() {
        let _low = super::PerPage::new(1);
        let _high = super::PerPage::new(100);
    }

    /// [T-GH011b] PerPage::new panics on 0 (ADR-0004 Rule 2; 0 is
    /// implementation-defined behavior in GitHub API)
    #[test]
    #[should_panic(expected = "PerPage must be 1..=100")]
    fn per_page_new_panics_on_zero() {
        let _ = super::PerPage::new(0);
    }

    /// [T-GH011c] PerPage::new panics on values over 100 (ADR-0004 Rule 2)
    #[test]
    #[should_panic(expected = "PerPage must be 1..=100")]
    fn per_page_new_panics_on_over_100() {
        let _ = super::PerPage::new(101);
    }

    /// [T-GH018] from_env_with_source threads the injected TokenSource through
    /// to the constructed client's token field. Proves the seam reaches the
    /// constructor without spawning `gh auth token`.
    #[tokio::test]
    async fn from_env_with_static_source_installs_token() {
        use crate::token_source::StaticTokenSource;
        let source = StaticTokenSource(Some(
            Redacted::new("injected-token").expect("static literal is non-empty"),
        ));
        let client = GitHubClient::from_env_with_source(Client::new(), 0, &source).await;
        assert_eq!(
            client.token.as_ref().map(Redacted::expose),
            Some("injected-token")
        );
    }

    /// [T-GH019] from_env_with_source propagates a None source so callers can
    /// simulate the unauthenticated path without env-var manipulation.
    #[tokio::test]
    async fn from_env_with_none_source_leaves_token_empty() {
        use crate::token_source::StaticTokenSource;
        let client =
            GitHubClient::from_env_with_source(Client::new(), 0, &StaticTokenSource(None)).await;
        assert!(client.token.is_none());
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

    /// [T-GH008] get_json_once uses x-ratelimit-reset to compute delay on 403 rate limit.
    /// Pinned clock + reset 60s later asserts the exact subtraction result.
    #[tokio::test]
    async fn get_json_403_with_ratelimit_reset_carries_delay() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .append_header("x-ratelimit-reset", "1060")
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri())
            .with_clock(Arc::new(FixedClock(1000)));
        let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
        assert!(
            matches!(
                result,
                Err(GitHubError::RateLimited {
                    retry_after: Some(60)
                })
            ),
            "expected retry_after = 60 (reset 1060 - clock 1000), got: {result:?}"
        );
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
    /// retry loop exhausts the configured max_retries attempts before failing
    /// (issue #113). The test uses the client default (3); issue #120 lets
    /// production callers override the budget via `SCOUT_MAX_RETRIES`.
    ///
    /// reqwest 0.13 surfaces a mid-stream drop as `is_decode() == true` with
    /// an io::Error in the source chain. Without `is_transient_decode`,
    /// every attempt would route to `GitHubError::Decode` → Internal(70)
    /// retryable=false. With it, attempts route to `GitHubError::Network` →
    /// TempFailure(75) and the retry loop kicks in.
    ///
    /// `start_paused = true` advances the tokio runtime past `retry_with`'s
    /// `sleep` calls as soon as the task parks; the std::thread-driven
    /// TcpListener is unaffected. Total wall time stays under 100 ms.
    #[tokio::test(start_paused = true)]
    async fn get_json_2xx_mid_stream_drop_exhausts_retries() {
        // Total attempts = 1 (initial) + DEFAULT_MAX_RETRIES (retries).
        let expected_attempts =
            usize::try_from(DEFAULT_MAX_RETRIES + 1).expect("DEFAULT_MAX_RETRIES + 1 fits usize");
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
            "retry loop must consume the full max_retries budget"
        );

        let _ = handle.join();
    }

    /// [T-GH014] secs_until_ratelimit_reset subtracts the injected clock
    /// from the x-ratelimit-reset header. Pinning the clock removes wall-clock
    /// flakiness from the arithmetic test.
    #[test]
    fn secs_until_ratelimit_reset_uses_injected_clock() {
        use reqwest::header::HeaderValue;
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1300"));
        let clock = FixedClock(1000);
        assert_eq!(secs_until_ratelimit_reset(&headers, &clock), Some(300));
    }

    /// [T-GH015] secs_until_ratelimit_reset saturates to 0 when the injected
    /// clock has already passed the reset timestamp — `u64::saturating_sub`
    /// prevents an underflow that would otherwise wrap to a huge delay.
    #[test]
    fn secs_until_ratelimit_reset_saturates_when_clock_past_reset() {
        use reqwest::header::HeaderValue;
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("100"));
        let clock = FixedClock(200);
        assert_eq!(secs_until_ratelimit_reset(&headers, &clock), Some(0));
    }

    /// [T-GH016] secs_until_ratelimit_reset returns None when the
    /// x-ratelimit-reset header is absent; production callers then fall back
    /// to jittered backoff.
    #[test]
    fn secs_until_ratelimit_reset_returns_none_when_header_missing() {
        let headers = HeaderMap::new();
        let clock = FixedClock(1000);
        assert_eq!(secs_until_ratelimit_reset(&headers, &clock), None);
    }

    /// [T-GH017] with_clock injection threads through to the 403 retry_after
    /// calculation. End-to-end proof that the seam works for callers (the
    /// pure-function tests above only cover the helper in isolation).
    #[tokio::test]
    async fn get_json_403_uses_injected_clock_for_retry_after() {
        let Some(server) = try_spawn_mock_server("github::http_clock_inject").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .append_header("x-ratelimit-reset", "1300")
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri())
            .with_clock(Arc::new(FixedClock(1000)));
        let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
        assert!(
            matches!(
                result,
                Err(GitHubError::RateLimited {
                    retry_after: Some(300)
                })
            ),
            "expected retry_after = 300 (reset 1300 - clock 1000), got: {result:?}"
        );
    }
}
