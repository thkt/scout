pub mod format;
mod helpers;
pub mod types;

pub use helpers::{
    apply_line_range, decode_content, filter_tree_entries, parse_line_range, parse_repo,
    validate_path, validate_ref,
};
use helpers::encode_path;

use reqwest::Client;
use std::env;
use tracing::{debug, warn};

use types::*;

const API_BASE: &str = "https://api.github.com";

/// Errors returned by GitHub API operations.
#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error(
        "GitHub API rate limit exceeded. Set GITHUB_TOKEN or run `gh auth login` for higher limits."
    )]
    RateLimited,

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
}

/// HTTP client for the GitHub REST API v3.
///
/// Auth resolution order: `GITHUB_TOKEN` env → `GH_TOKEN` env → `gh auth token` CLI → unauthenticated.
/// Owner/repo parameters are safe for direct URL interpolation because `parse_repo`
/// restricts them to `[a-zA-Z0-9._-]`.
#[derive(Clone)]
pub struct GitHubClient {
    http: Client,
    token: Option<String>,
    base_url: String,
}

impl GitHubClient {
    /// Create a client using standard GitHub API and auto-detected auth.
    pub fn from_env(http: Client) -> Self {
        let token = resolve_token();
        if token.is_some() {
            debug!("GitHub token configured");
        } else {
            warn!("No GitHub token found. Rate limit: 60 req/hour. Set GITHUB_TOKEN or run `gh auth login`.");
        }
        Self {
            http,
            token,
            base_url: API_BASE.to_string(),
        }
    }

    #[cfg(test)]
    fn with_base_url(http: Client, base_url: &str) -> Self {
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
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        req
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, GitHubError> {
        let response = self.request(path).send().await?;
        let status = response.status();
        match status.as_u16() {
            200..=299 => Ok(response.json().await?),
            404 => Err(GitHubError::NotFound(path.to_string())),
            429 => Err(GitHubError::RateLimited),
            403 => {
                let remaining = response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                if remaining == Some(0) {
                    Err(GitHubError::RateLimited)
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

fn resolve_token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|var| env::var(var).ok())
        .map(|t| t.trim().to_string())
        .find(|t| !t.is_empty())
        .or_else(|| {
            std::process::Command::new("gh")
                .args(["auth", "token"])
                .output()
                .ok()
                .filter(|o| {
                    if !o.status.success() {
                        debug!(
                            stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                            "gh auth token failed"
                        );
                    }
                    o.status.success()
                })
                .and_then(|o| {
                    let token = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if token.is_empty() { None } else { Some(token) }
                })
        })
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_json_404_returns_not_found() {
        let server = MockServer::start().await;
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::RateLimited)));
    }

    #[tokio::test]
    async fn get_json_403_with_zero_remaining_returns_rate_limited() {
        let server = MockServer::start().await;
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
        assert!(matches!(result, Err(GitHubError::RateLimited)));
    }

    #[tokio::test]
    async fn get_json_403_with_remaining_returns_forbidden() {
        let server = MockServer::start().await;
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
    async fn get_json_500_returns_api_error() {
        let server = MockServer::start().await;
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
}
