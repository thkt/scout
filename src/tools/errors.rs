use std::error::Error;
use std::fmt;
use tracing::warn;

use crate::envelope::ErrorCode;
use crate::fetch::FetchError;
use crate::gemini::client::GeminiError;
use crate::github;
use crate::retry::is_transient_network;
use crate::slack::SlackError;

/// Reusable next_step hints so transient/network errors stay consistent.
const HINT_RETRY_DELAY: &str = "Retry after a short delay";
const HINT_CHECK_NETWORK: &str = "Check your network connection";

#[derive(Debug)]
pub struct ScoutError {
    message: String,
    retryable: bool,
    kind: ErrorCode,
    next_step: Option<String>,
    candidates: Vec<String>,
}

impl fmt::Display for ScoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.next_step {
            write!(f, " — {hint}")?;
        }
        if self.retryable {
            write!(f, " (temporary failure; retry may succeed)")?;
        }
        Ok(())
    }
}

impl Error for ScoutError {}

impl ScoutError {
    pub(super) fn user_error(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: false,
            kind: ErrorCode::UsageError,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn internal(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: false,
            kind: ErrorCode::IoError,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn transient(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: true,
            kind: ErrorCode::TempFailure,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn not_found(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: false,
            kind: ErrorCode::NotFound,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn data_error(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: false,
            kind: ErrorCode::DataError,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    /// Attach a recovery hint to this error per ADR-0002 `error.next_step`.
    pub(super) fn with_next_step(mut self, hint: impl Into<String>) -> Self {
        self.next_step = Some(hint.into());
        self
    }

    /// Attach correction candidates per ADR-0002 `error.candidates` (e.g., typo suggestions).
    pub(super) fn with_candidates(mut self, candidates: Vec<String>) -> Self {
        self.candidates = candidates;
        self
    }

    /// sysexits.h exit code derived from `kind` per ADR-0002.
    pub fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    /// JSON-serializable error classification per ADR-0002.
    pub fn error_kind(&self) -> ErrorCode {
        self.kind
    }

    /// Plain message without next_step / retry hints. Use for JSON `error.message`
    /// where `error.next_step` and `error.retryable` are surfaced separately.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Recovery hint per ADR-0002 `error.next_step`.
    pub fn next_step(&self) -> Option<&str> {
        self.next_step.as_deref()
    }

    /// Correction candidates per ADR-0002 `error.candidates` (e.g., similar paths after typo).
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }
}

pub(super) fn parse_repo_param(repository: &str) -> Result<(&str, &str), ScoutError> {
    github::parse_repo(repository).map_err(ScoutError::from)
}

impl From<github::GitHubError> for ScoutError {
    fn from(e: github::GitHubError) -> Self {
        match &e {
            github::GitHubError::NotFound(_) => Self::not_found(e.to_string()).with_next_step(
                "Check that the repository or path exists, and that you have access",
            ),
            github::GitHubError::InvalidRepo(_) => Self::data_error(e.to_string())
                .with_next_step("Use 'owner/repo' format, e.g., 'facebook/react'"),
            github::GitHubError::InvalidRef(_) => Self::data_error(e.to_string())
                .with_next_step("Use a branch name, tag, or commit SHA"),
            github::GitHubError::InvalidPath(_) => {
                Self::data_error(e.to_string()).with_next_step("Use a path within the repository")
            }
            github::GitHubError::InvalidLineRange(_) => Self::data_error(e.to_string())
                .with_next_step("Use format like '1-80', '50-', or '100' (first N lines)"),
            github::GitHubError::InvalidPattern(_) => Self::data_error(e.to_string())
                .with_next_step("Use a glob pattern like '*.rs' or '*.{ts,tsx}'"),
            github::GitHubError::NonUtf8(_) => Self::data_error(e.to_string())
                .with_next_step("Pass --encoding to decode non-UTF-8 files (e.g., shift_jis)"),
            github::GitHubError::RateLimited { retry_after } => Self::transient(e.to_string())
                .with_next_step(match retry_after {
                    Some(secs) => format!(
                        "Retry after {secs} seconds, or set GITHUB_TOKEN to increase rate limit"
                    ),
                    None => "Set GITHUB_TOKEN to increase rate limit".to_owned(),
                }),
            github::GitHubError::Forbidden(_) => Self::user_error(e.to_string())
                .with_next_step("Check that your GITHUB_TOKEN has the required scopes"),
            github::GitHubError::Network(_) => {
                Self::transient(e.to_string()).with_next_step(HINT_CHECK_NETWORK)
            }
            github::GitHubError::Api { code, .. } if (500..=599).contains(code) => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
            }
            github::GitHubError::Api { .. } | github::GitHubError::Decode(_) => {
                Self::internal(e.to_string())
            }
        }
    }
}

impl From<FetchError> for ScoutError {
    fn from(e: FetchError) -> Self {
        match &e {
            FetchError::InvalidScheme => {
                Self::data_error(e.to_string()).with_next_step("URL must use http:// or https://")
            }
            FetchError::InvalidUrl(_) => {
                Self::data_error(e.to_string()).with_next_step("URL must include scheme and host")
            }
            FetchError::InternalHost => Self::data_error(e.to_string())
                .with_next_step("URL must point to an external host (private IPs are blocked)"),
            FetchError::UnsupportedContentType(_) => Self::data_error(e.to_string())
                .with_next_step("URL must serve HTML or text content"),
            FetchError::RedirectMissingLocation => Self::data_error(e.to_string()),
            FetchError::BrowserNotFound(_) => Self::user_error(e.to_string()),
            FetchError::BrowserFailed(_) => Self::internal(e.to_string()),
            FetchError::Status(408 | 429) => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
            }
            FetchError::Status(404) => Self::not_found(e.to_string())
                .with_next_step("Check that the URL is correct and the resource exists"),
            FetchError::Status(401 | 403) => Self::user_error(e.to_string())
                .with_next_step("URL requires authentication that scout does not support"),
            FetchError::Status(code) if (400..500).contains(code) => {
                Self::data_error(e.to_string())
            }
            FetchError::TooLarge => Self::data_error(e.to_string())
                .with_next_step("URL response exceeds 10MB; fetch a smaller resource"),
            FetchError::TooManyRedirects(_) => Self::data_error(e.to_string())
                .with_next_step("URL has too many redirects; check for a redirect loop"),
            FetchError::Status(_) | FetchError::Timeout(_) => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
            }
            FetchError::DnsResolution(_) => Self::transient(e.to_string())
                .with_next_step("Check the URL's domain name and your DNS resolver"),
            FetchError::Http(re) => {
                if is_transient_network(re) {
                    Self::transient(e.to_string()).with_next_step(HINT_CHECK_NETWORK)
                } else {
                    Self::internal(e.to_string())
                }
            }
        }
    }
}

impl From<SlackError> for ScoutError {
    fn from(e: SlackError) -> Self {
        match &e {
            SlackError::TokenNotSet => Self::user_error(e.to_string())
                .with_next_step("Export a User OAuth token to SLACK_TOKEN (xoxp-…)"),
            SlackError::Api { .. } => Self::user_error(e.to_string()),
            SlackError::RateLimited { .. } => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
            }
            SlackError::Network(_) => {
                Self::transient(e.to_string()).with_next_step(HINT_CHECK_NETWORK)
            }
            SlackError::Timeout(_) => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
            }
            SlackError::Decode(_) => Self::internal(e.to_string()),
        }
    }
}

impl From<GeminiError> for ScoutError {
    fn from(e: GeminiError) -> Self {
        match &e {
            GeminiError::ApiKeyNotSet => Self::user_error(e.to_string())
                .with_next_step("Set GEMINI_API_KEY environment variable"),
            GeminiError::RateLimited { .. } => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
            }
            GeminiError::QuotaExhausted(_) => Self::user_error(e.to_string())
                .with_next_step("Check your API billing at https://aistudio.google.com"),
            GeminiError::Network(_) => {
                Self::transient(e.to_string()).with_next_step(HINT_CHECK_NETWORK)
            }
            GeminiError::Api { code, .. } if (500..=599).contains(code) => {
                Self::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
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

    /// [T-ER010] user_error returns ErrorCode::UsageError
    #[test]
    fn user_error_kind_is_usage() {
        let err = ScoutError::user_error("test");
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-ER011] transient returns ErrorCode::TempFailure
    #[test]
    fn transient_kind_is_temp_failure() {
        let err = ScoutError::transient("test");
        assert_eq!(err.error_kind(), ErrorCode::TempFailure);
    }

    /// [T-ER012] internal returns ErrorCode::IoError
    #[test]
    fn internal_kind_is_io_error() {
        let err = ScoutError::internal("test");
        assert_eq!(err.error_kind(), ErrorCode::IoError);
    }

    /// [T-CD001] Errors default to empty candidates list
    #[test]
    fn default_candidates_empty() {
        let err = ScoutError::not_found("test");
        assert!(err.candidates().is_empty());
    }

    /// [T-CD002] with_candidates attaches correction suggestions
    #[test]
    fn with_candidates_attaches_list() {
        let err = ScoutError::not_found("path not found")
            .with_candidates(vec!["README.md".into(), "REDAME.md".into()]);
        assert_eq!(err.candidates(), &["README.md", "REDAME.md"]);
    }

    /// [T-NS001] ApiKeyNotSet sets next_step pointing to GEMINI_API_KEY env var
    #[test]
    fn gemini_api_key_not_set_has_next_step() {
        let err = ScoutError::from(GeminiError::ApiKeyNotSet);
        assert_eq!(
            err.next_step(),
            Some("Set GEMINI_API_KEY environment variable")
        );
    }

    /// [T-NS002] GitHubError::Forbidden separates GITHUB_TOKEN hint into next_step (not message)
    #[test]
    fn github_forbidden_separates_hint_into_next_step() {
        let err = ScoutError::from(github::GitHubError::Forbidden("denied".into()));
        assert_eq!(
            err.next_step(),
            Some("Check that your GITHUB_TOKEN has the required scopes")
        );
    }

    /// [T-NS003] GitHubError::NotFound has actionable next_step
    #[test]
    fn github_not_found_has_next_step() {
        let err = ScoutError::from(github::GitHubError::NotFound("/test".into()));
        assert!(
            err.next_step()
                .is_some_and(|h| h.contains("Check that the repository or path exists"))
        );
    }

    /// [T-NS004] FetchError::Status(404) has next_step about the URL
    #[test]
    fn fetch_404_has_next_step() {
        let err = ScoutError::from(FetchError::Status(404));
        assert!(
            err.next_step()
                .is_some_and(|h| h.contains("Check that the URL is correct"))
        );
    }

    /// [T-NS005] GeminiError::QuotaExhausted separates billing URL hint into next_step
    #[test]
    fn gemini_quota_exhausted_separates_billing_hint() {
        let err = ScoutError::from(GeminiError::QuotaExhausted("limit".into()));
        assert!(
            err.next_step()
                .is_some_and(|h| h.contains("aistudio.google.com"))
        );
    }

    /// [T-NS006] GitHubError::RateLimited with retry_after embeds the duration in next_step
    #[test]
    fn github_rate_limited_with_retry_after_embeds_duration() {
        let err = ScoutError::from(github::GitHubError::RateLimited {
            retry_after: Some(42),
        });
        assert!(
            err.next_step().is_some_and(|h| h.contains("42 seconds")),
            "next_step should mention retry_after seconds, got: {:?}",
            err.next_step()
        );
    }

    /// [T-NS007] GitHubError::RateLimited without retry_after still suggests setting GITHUB_TOKEN
    #[test]
    fn github_rate_limited_without_retry_after_suggests_token() {
        let err = ScoutError::from(github::GitHubError::RateLimited { retry_after: None });
        assert!(err.next_step().is_some_and(|h| h.contains("GITHUB_TOKEN")));
    }

    /// [T-NS008] Display includes next_step appended to message
    #[test]
    fn display_includes_next_step() {
        let err = ScoutError::user_error("Something is wrong").with_next_step("Try X");
        let display = err.to_string();
        assert!(display.contains("Something is wrong"));
        assert!(display.contains("Try X"));
    }

    /// [T-NS009] Errors without next_step omit the hint from Display
    #[test]
    fn display_omits_next_step_when_absent() {
        let err = ScoutError::internal("internal failure");
        let display = err.to_string();
        assert_eq!(display, "internal failure");
    }

    /// [T-ER013] GitHubError::NotFound classifies as ErrorCode::NotFound
    #[test]
    fn github_not_found_classifies_as_not_found() {
        let err = ScoutError::from(github::GitHubError::NotFound("/test".into()));
        assert_eq!(err.error_kind(), ErrorCode::NotFound);
    }

    /// [T-ER014] GitHubError::InvalidRepo classifies as ErrorCode::DataError
    #[test]
    fn github_invalid_repo_classifies_as_data_error() {
        let err = ScoutError::from(github::GitHubError::InvalidRepo("bad".into()));
        assert_eq!(err.error_kind(), ErrorCode::DataError);
    }

    /// [T-ER015] FetchError::InvalidScheme classifies as ErrorCode::DataError
    #[test]
    fn fetch_invalid_scheme_classifies_as_data_error() {
        let err = ScoutError::from(FetchError::InvalidScheme);
        assert_eq!(err.error_kind(), ErrorCode::DataError);
    }

    /// [T-ER016] FetchError::Status(404) classifies as ErrorCode::NotFound
    #[test]
    fn fetch_status_404_classifies_as_not_found() {
        let err = ScoutError::from(FetchError::Status(404));
        assert_eq!(err.error_kind(), ErrorCode::NotFound);
    }

    /// [T-ER001a] UsageError errors surface with exit 64 (EX_USAGE per ADR-0002)
    #[test]
    fn usage_errors_have_exit_code_64() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::Forbidden("denied".into()).into(),
            FetchError::BrowserNotFound("not installed".into()).into(),
            FetchError::Status(401).into(),
            FetchError::Status(403).into(),
            SlackError::TokenNotSet.into(),
            SlackError::Api {
                error: "err".into(),
            }
            .into(),
            GeminiError::ApiKeyNotSet.into(),
            GeminiError::QuotaExhausted("limit".into()).into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::UsageError, "{err}");
            assert_eq!(err.exit_code(), 64, "expected EX_USAGE (64): {err}");
        }
    }

    /// [T-ER001b] DataError errors surface with exit 65 (EX_DATAERR per ADR-0002)
    #[test]
    fn data_errors_have_exit_code_65() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::InvalidRepo("bad".into()).into(),
            FetchError::InvalidScheme.into(),
            FetchError::InternalHost.into(),
            FetchError::UnsupportedContentType("image/png".into()).into(),
            FetchError::RedirectMissingLocation.into(),
            FetchError::Status(400).into(),
            FetchError::Status(499).into(),
            FetchError::TooLarge.into(),
            FetchError::TooManyRedirects(10).into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::DataError, "{err}");
            assert_eq!(err.exit_code(), 65, "expected EX_DATAERR (65): {err}");
        }
    }

    /// [T-ER001c] NotFound errors surface with exit 66 (EX_NOINPUT per ADR-0002)
    #[test]
    fn not_found_errors_have_exit_code_66() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::NotFound("/test".into()).into(),
            FetchError::Status(404).into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::NotFound, "{err}");
            assert_eq!(err.exit_code(), 66, "expected EX_NOINPUT (66): {err}");
        }
    }

    /// [T-ER002] IoError errors surface with exit 74 (EX_IOERR) and are non-retryable
    #[test]
    fn io_errors_have_exit_code_74() {
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
            assert_eq!(err.error_kind(), ErrorCode::IoError, "{err}");
            assert_eq!(err.exit_code(), 74, "expected EX_IOERR (74): {err}");
            assert!(!err.retryable(), "IoError should not be retryable: {err}");
        }
    }

    /// [T-ER003] TempFailure errors are retryable, display retry hint, exit 75 (EX_TEMPFAIL)
    #[test]
    fn temp_failure_errors_have_exit_code_75() {
        let cases: Vec<ScoutError> = vec![
            FetchError::Status(408).into(),
            FetchError::Status(429).into(),
            FetchError::Status(500).into(),
            FetchError::Status(503).into(),
            FetchError::DnsResolution("dns failed".into()).into(),
            FetchError::Timeout("timed out".into()).into(),
            github::GitHubError::RateLimited { retry_after: None }.into(),
            github::GitHubError::Api {
                code: 502,
                message: "bad gateway".into(),
            }
            .into(),
            GeminiError::RateLimited { retry_after: None }.into(),
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
            assert_eq!(err.error_kind(), ErrorCode::TempFailure, "{err}");
            assert!(err.retryable(), "expected retryable: {err}");
            assert_eq!(err.exit_code(), 75, "expected EX_TEMPFAIL (75): {err}");
            assert!(
                err.to_string().contains("retry may succeed"),
                "should include retry hint: {err}"
            );
        }
    }

    /// [T-ER004] Non-transient errors are not retryable and omit retry hint
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

    // TcpListener::drop is synchronous, so the port is immediately closed
    // with no async shutdown race (unlike MockServer).
    /// [T-ER009] Connection-refused FetchError::Http maps to transient ScoutError
    #[tokio::test]
    async fn fetch_error_http_connection_refused_is_transient() {
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
