pub(crate) mod encoding;
mod errors;
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
    MAX_GITHUB_RESPONSE_BYTES, is_transient_network, parse_retry_after, read_body_capped,
    retry_after_within_cap, retry_with_rate_limit,
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

pub(crate) use errors::GitHubError;

const API_BASE: &str = "https://api.github.com";

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
            // Body capped at `MAX_GITHUB_RESPONSE_BYTES` to bound the memory an
            // oversized response can consume (issue #186). Splitting the read
            // from the parse also separates failure modes: a transport drop is
            // mapped to `Network` by `GitHubError::from` (→ retry loop), while a
            // schema mismatch surfaces from `serde_json::from_slice` as terminal
            // `Decode` (issue #113).
            200..=299 => {
                let bytes = read_body_capped(
                    response,
                    MAX_GITHUB_RESPONSE_BYTES,
                    || GitHubError::ResponseTooLarge,
                    GitHubError::from,
                )
                .await?;
                serde_json::from_slice(&bytes).map_err(|e| GitHubError::Decode(e.to_string()))
            }
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
mod http_tests;
