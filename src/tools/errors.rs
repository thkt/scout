use std::fmt;
use tracing::warn;

use crate::fetch::FetchError;
use crate::gemini::client::GeminiError;
use crate::github;
use crate::retry::is_transient_network;
use crate::slack::SlackError;

#[derive(Debug)]
pub struct ScoutError {
    message: String,
    exit_code: i32,
    retryable: bool,
}

impl fmt::Display for ScoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if self.retryable {
            write!(f, " (temporary failure; retry may succeed)")?;
        }
        Ok(())
    }
}

impl std::error::Error for ScoutError {}

impl ScoutError {
    pub(super) fn user_error(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 1,
            retryable: false,
        }
    }

    pub(super) fn internal(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 2,
            retryable: false,
        }
    }

    pub(super) fn transient(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 2,
            retryable: true,
        }
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Whether the error is transient and the operation may succeed on retry.
    #[allow(dead_code)] // public API for future --json output
    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

pub(super) fn parse_repo_param(repository: &str) -> Result<(&str, &str), ScoutError> {
    github::parse_repo(repository).map_err(ScoutError::from)
}

impl From<github::GitHubError> for ScoutError {
    fn from(e: github::GitHubError) -> Self {
        match &e {
            github::GitHubError::NotFound(_)
            | github::GitHubError::InvalidRepo(_)
            | github::GitHubError::InvalidRef(_)
            | github::GitHubError::InvalidPath(_)
            | github::GitHubError::InvalidLineRange(_)
            | github::GitHubError::InvalidPattern(_) => Self::user_error(e.to_string()),
            github::GitHubError::RateLimited => Self::transient(e.to_string()),
            github::GitHubError::Forbidden(_) => Self::user_error(format!(
                "{e} — check that your GITHUB_TOKEN has the required scopes"
            )),
            github::GitHubError::Network(_) => Self::transient(e.to_string()),
            github::GitHubError::Api { code, .. } if (500..=599).contains(code) => {
                Self::transient(e.to_string())
            }
            github::GitHubError::Api { .. } | github::GitHubError::Decode(_) => {
                Self::internal(e.to_string())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FetchHttpKind {
    Transient,
    Permanent,
}

fn classify_fetch_http(transient: bool) -> FetchHttpKind {
    if transient {
        FetchHttpKind::Transient
    } else {
        FetchHttpKind::Permanent
    }
}

impl From<FetchError> for ScoutError {
    fn from(e: FetchError) -> Self {
        match &e {
            FetchError::InvalidScheme
            | FetchError::InvalidUrl(_)
            | FetchError::InternalHost
            | FetchError::UnsupportedContentType(_)
            | FetchError::RedirectMissingLocation => Self::user_error(e.to_string()),
            FetchError::BrowserNotFound(_) => Self::user_error(e.to_string()),
            FetchError::BrowserFailed(_) => Self::internal(e.to_string()),
            FetchError::Status(408 | 429) => Self::transient(e.to_string()),
            FetchError::Status(code) if (400..500).contains(code) => {
                Self::user_error(e.to_string())
            }
            FetchError::TooLarge | FetchError::TooManyRedirects(_) => {
                Self::user_error(e.to_string())
            }
            FetchError::Status(_) | FetchError::Timeout(_) | FetchError::DnsResolution(_) => {
                Self::transient(e.to_string())
            }
            FetchError::Http(re) => match classify_fetch_http(is_transient_network(re)) {
                FetchHttpKind::Transient => Self::transient(e.to_string()),
                FetchHttpKind::Permanent => Self::internal(e.to_string()),
            },
        }
    }
}

impl From<SlackError> for ScoutError {
    fn from(e: SlackError) -> Self {
        match &e {
            SlackError::TokenNotSet | SlackError::Api { .. } => Self::user_error(e.to_string()),
            SlackError::RateLimited { .. } | SlackError::Network(_) | SlackError::Timeout(_) => {
                Self::transient(e.to_string())
            }
            SlackError::Decode(_) => Self::internal(e.to_string()),
        }
    }
}

impl From<GeminiError> for ScoutError {
    fn from(e: GeminiError) -> Self {
        match &e {
            GeminiError::ApiKeyNotSet => Self::user_error(e.to_string()),
            GeminiError::RateLimited => Self::transient(e.to_string()),
            GeminiError::QuotaExhausted(_) => Self::user_error(format!(
                "{e} — check your API billing at https://aistudio.google.com"
            )),
            GeminiError::Network(_) => Self::transient(e.to_string()),
            GeminiError::Api { code, .. } if (500..=599).contains(code) => {
                Self::transient(e.to_string())
            }
            GeminiError::Api { .. } => Self::internal(e.to_string()),
        }
    }
}

pub(super) fn unwrap_or_note<T>(
    result: Result<Vec<T>, github::GitHubError>,
    label: &str,
    notes: &mut Vec<String>,
) -> Vec<T> {
    match result {
        Ok(v) => v,
        Err(e) => {
            warn!(%e, "failed to fetch {}", label);
            notes.push(format!("Could not fetch {label} ({e})"));
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_errors_have_exit_code_1() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::NotFound("/test".into()).into(),
            github::GitHubError::Forbidden("denied".into()).into(),
            github::GitHubError::InvalidRepo("bad".into()).into(),
            FetchError::InvalidScheme.into(),
            FetchError::InternalHost.into(),
            FetchError::UnsupportedContentType("image/png".into()).into(),
            FetchError::RedirectMissingLocation.into(),
            FetchError::BrowserNotFound("not installed".into()).into(),
            FetchError::Status(400).into(),
            FetchError::Status(404).into(),
            FetchError::Status(403).into(),
            FetchError::Status(499).into(),
            FetchError::TooLarge.into(),
            FetchError::TooManyRedirects(10).into(),
            SlackError::TokenNotSet.into(),
            SlackError::Api {
                error: "err".into(),
            }
            .into(),
            GeminiError::ApiKeyNotSet.into(),
            GeminiError::QuotaExhausted("limit".into()).into(),
        ];
        for err in &cases {
            assert_eq!(err.exit_code(), 1, "expected user error (1): {err}");
        }
    }

    #[test]
    fn internal_errors_have_exit_code_2() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::Api {
                code: 400,
                message: "bad request".into(),
            }
            .into(),
            github::GitHubError::Decode("decode error".into()).into(),
            FetchError::BrowserFailed("CDP protocol error".into()).into(),
            SlackError::Decode("err".into()).into(),
            GeminiError::Api {
                code: 400,
                message: "err".into(),
            }
            .into(),
        ];
        for err in &cases {
            assert_eq!(err.exit_code(), 2, "expected internal error (2): {err}");
            assert!(!err.retryable(), "internal should not be retryable: {err}");
        }
    }

    #[test]
    fn transient_errors_are_retryable() {
        let cases: Vec<ScoutError> = vec![
            FetchError::Status(408).into(),
            FetchError::Status(429).into(),
            FetchError::Status(500).into(),
            FetchError::Status(503).into(),
            FetchError::DnsResolution("dns failed".into()).into(),
            FetchError::Timeout("timed out".into()).into(),
            github::GitHubError::RateLimited.into(),
            github::GitHubError::Api {
                code: 502,
                message: "bad gateway".into(),
            }
            .into(),
            GeminiError::RateLimited.into(),
            GeminiError::Api {
                code: 503,
                message: "unavailable".into(),
            }
            .into(),
            SlackError::RateLimited { retry_after: None }.into(),
            SlackError::Network("err".into()).into(),
            SlackError::Timeout("err".into()).into(),
        ];
        for err in &cases {
            assert!(err.retryable(), "expected retryable: {err}");
            assert_eq!(err.exit_code(), 2, "retryable should be exit 2: {err}");
            assert!(
                err.to_string().contains("retry may succeed"),
                "should include retry hint: {err}"
            );
        }
    }

    #[test]
    fn non_transient_errors_are_not_retryable() {
        let cases: Vec<ScoutError> = vec![
            FetchError::InvalidScheme.into(),
            FetchError::Status(404).into(),
            FetchError::BrowserFailed("err".into()).into(),
            github::GitHubError::Decode("err".into()).into(),
            SlackError::Decode("err".into()).into(),
        ];
        for err in &cases {
            assert!(!err.retryable(), "expected not retryable: {err}");
            assert!(
                !err.to_string().contains("retry may succeed"),
                "should not include retry hint: {err}"
            );
        }
    }

    #[test]
    fn classify_fetch_http_transient_input() {
        assert_eq!(classify_fetch_http(true), FetchHttpKind::Transient);
    }

    #[test]
    fn classify_fetch_http_permanent_input() {
        assert_eq!(classify_fetch_http(false), FetchHttpKind::Permanent);
    }

    #[test]
    fn github_forbidden_hints_token() {
        let err = ScoutError::from(github::GitHubError::Forbidden("denied".into()));
        assert!(err.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn quota_exhausted_hints_billing_url() {
        let err = ScoutError::from(GeminiError::QuotaExhausted("limit".into()));
        assert!(err.to_string().contains("aistudio.google.com"));
    }

    // TcpListener::drop is synchronous, so the port is immediately closed
    // with no async shutdown race (unlike MockServer).
    #[tokio::test]
    async fn t003_fetch_error_http_connection_refused_is_transient() {
        use reqwest::Client;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let dead_url = format!("http://{addr}/should-refuse");

        let client = Client::new();
        let reqwest_err = client
            .get(&dead_url)
            .send()
            .await
            .expect_err("request to dead port should fail");

        assert!(
            is_transient_network(&reqwest_err),
            "expected transient network error, got: {reqwest_err}"
        );

        let fetch_err = FetchError::Http(reqwest_err);
        let scout_err = ScoutError::from(fetch_err);

        assert!(
            scout_err.retryable(),
            "connection-refused FetchError::Http should produce transient ScoutError"
        );
        assert!(
            scout_err.to_string().contains("retry may succeed"),
            "transient error should contain retry hint: {}",
            scout_err
        );
    }
}
