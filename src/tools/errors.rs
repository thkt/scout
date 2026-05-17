use std::error::Error;
use std::fmt;
use tracing::warn;

use crate::brave::client::BraveError;
use crate::envelope::{Degradation, DegradedReason, ErrorCode};
use crate::fetch::FetchError;
use crate::github;
use crate::retry::is_transient_network;
use crate::slack::SlackError;

/// Reusable next_step hints so transient/network errors stay consistent.
const HINT_RETRY_DELAY: &str = "Retry after a short delay";
const HINT_CHECK_NETWORK: &str = "Check your network connection";

/// Builds a transient `ScoutError` with the "retry after a short delay" hint.
/// Used for rate-limit, 5xx, and other timing-recoverable failures.
fn transient_with_retry_hint(e: &impl fmt::Display) -> ScoutError {
    ScoutError::transient(e.to_string()).with_next_step(HINT_RETRY_DELAY)
}

/// Builds a transient `ScoutError` with the "check your network" hint.
/// Used for connect-level network failures where retry alone will not help.
fn transient_with_network_hint(e: &impl fmt::Display) -> ScoutError {
    ScoutError::transient(e.to_string()).with_next_step(HINT_CHECK_NETWORK)
}

/// Builds a timeout `ScoutError` with the "retry after a short delay" hint.
/// Used for transport-timeout failures distinct from generic transients.
fn timeout_with_retry_hint(e: &impl fmt::Display) -> ScoutError {
    ScoutError::timeout(e.to_string()).with_next_step(HINT_RETRY_DELAY)
}

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
    /// Shared construction path. `retryable` is derived from `kind` so the
    /// public exit-code/JSON contract cannot drift between callers.
    fn new(kind: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: kind.is_retryable(),
            kind,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn user_error(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::UsageError, msg)
    }

    /// External tool / IO failure outside scout's invariants (e.g. headless
    /// browser CDP error). Maps to `ErrorCode::IoError` (exit 74 EX_IOERR).
    /// Use [`Self::internal_bug`] for scout-side schema bugs (exit 70).
    pub(super) fn io_error(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::IoError, msg)
    }

    /// scout-side invariant violation (e.g., unexpected API schema during
    /// deserialize). Maps to `ErrorCode::Internal` (exit 70 EX_SOFTWARE) per
    /// ADR-0065 priority 5.
    pub(super) fn internal_bug(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, msg)
    }

    /// Unclassifiable failure — the priority rules (1-5) did not match.
    /// Maps to `ErrorCode::Unknown` (exit 104, PJ extension) per ADR-0065
    /// §Classification Priority. A rising Unknown rate signals the
    /// classification design needs revisiting.
    pub(super) fn unknown(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unknown, msg)
    }

    pub(super) fn transient(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::TempFailure, msg)
    }

    /// Timeout (request-level or transport-level). Maps to `ErrorCode::Timeout`
    /// (exit 124, GNU coreutils `timeout`) per ADR-0065. Retryable like
    /// `transient`, but separated so caller scripts/agents can apply a longer
    /// backoff than for rate-limit / 5xx temp failures.
    pub(super) fn timeout(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, msg)
    }

    pub(super) fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, msg)
    }

    pub(super) fn data_error(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::DataError, msg)
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

// Match arms in each `From<...>` impl below evaluate in classification-priority
// order per ADR-0065 §Classification Priority:
//   1. USAGE_ERROR  — env/config/argument misuse
//   2. DATA_ERROR   — format violations (URL, owner/repo, encoding, 4xx body)
//   3. NOT_FOUND    — resource absence (404, search 0 hits)
//   4. TEMP_FAILURE — retryable (rate limit, 5xx, network); TIMEOUT(124) splits off
//   5. INTERNAL     — scout-side invariant violation (unexpected schema); IO_ERROR(74)
//                     is the sibling for external tool failure (browser)
// Disjoint variants are otherwise free to be reordered; the priority comments
// document intent so a reviewer can spot a misclassification mechanically.
impl From<github::GitHubError> for ScoutError {
    fn from(e: github::GitHubError) -> Self {
        match &e {
            // Priority 1: USAGE_ERROR
            github::GitHubError::Forbidden(_) => Self::user_error(e.to_string())
                .with_next_step("Check that your GITHUB_TOKEN has the required scopes"),
            // 401 must precede the 4xx arm below to avoid falling into DataError.
            github::GitHubError::Api { code: 401, .. } => Self::user_error(e.to_string())
                .with_next_step("Set GITHUB_TOKEN or run `gh auth login` to authenticate"),
            // Priority 2: DATA_ERROR
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
            github::GitHubError::InvalidPerPage(_) => Self::data_error(e.to_string())
                .with_next_step(
                    "GitHub API limits per_page to 100; pass a value between 1 and 100",
                ),
            github::GitHubError::NonUtf8(_) => Self::data_error(e.to_string())
                .with_next_step("Pass --encoding to decode non-UTF-8 files (e.g., shift_jis)"),
            github::GitHubError::InsecureUrl => Self::data_error(e.to_string()),
            github::GitHubError::Api { code, .. } if (400..500).contains(code) => {
                Self::data_error(e.to_string())
            }
            // Priority 3: NOT_FOUND
            github::GitHubError::NotFound(_) => Self::not_found(e.to_string()).with_next_step(
                "Check that the repository or path exists, and that you have access",
            ),
            // Priority 4: TIMEOUT (request timeout via reqwest builder)
            github::GitHubError::Network(re) if re.is_timeout() => timeout_with_retry_hint(&e),
            // Priority 4: TEMP_FAILURE
            github::GitHubError::RateLimited { retry_after } => Self::transient(e.to_string())
                .with_next_step(match retry_after {
                    Some(secs) => format!(
                        "Retry after {secs} seconds, or set GITHUB_TOKEN to increase rate limit"
                    ),
                    None => "Set GITHUB_TOKEN to increase rate limit".to_owned(),
                }),
            github::GitHubError::Network(_) => transient_with_network_hint(&e),
            github::GitHubError::Api { code, .. } if (500..=599).contains(code) => {
                transient_with_retry_hint(&e)
            }
            // Priority 5: INTERNAL — scout-side bug (unexpected schema)
            github::GitHubError::Decode(_) => Self::internal_bug(e.to_string()),
            // Unknown — Api codes that did not match 4xx or 5xx (e.g., 1xx/3xx leak)
            github::GitHubError::Api { .. } => Self::unknown(e.to_string()),
        }
    }
}

impl From<FetchError> for ScoutError {
    fn from(e: FetchError) -> Self {
        match &e {
            // Priority 1: USAGE_ERROR
            FetchError::BrowserNotFound(_) => Self::user_error(e.to_string()),
            // Priority 2: DATA_ERROR (non-Status variants)
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
            FetchError::TooLarge => Self::data_error(e.to_string())
                .with_next_step("URL response exceeds 10MB; fetch a smaller resource"),
            FetchError::TooManyRedirects(_) => Self::data_error(e.to_string())
                .with_next_step("URL has too many redirects; check for a redirect loop"),
            // Status arms order specific HTTP codes before the 4xx / _ fallback;
            // the per-arm priority label restores ADR-0065 ranking for review.
            // Priority 1: USAGE_ERROR
            FetchError::Status(401 | 403) => Self::user_error(e.to_string())
                .with_next_step("URL requires authentication that scout does not support"),
            // Priority 3: NOT_FOUND
            FetchError::Status(404) => Self::not_found(e.to_string())
                .with_next_step("Check that the URL is correct and the resource exists"),
            // Priority 4: TEMP_FAILURE
            FetchError::Status(408 | 429) => transient_with_retry_hint(&e),
            // Priority 2: DATA_ERROR (4xx body)
            FetchError::Status(code) if (400..500).contains(code) => {
                Self::data_error(e.to_string())
            }
            // Priority 4: TEMP_FAILURE (5xx and other unmatched)
            FetchError::Status(_) => transient_with_retry_hint(&e),
            // Priority 4: TIMEOUT (transport timeout — long-backoff retry advised)
            FetchError::Timeout(_) => timeout_with_retry_hint(&e),
            // Priority 4: TEMP_FAILURE (non-Status variants)
            FetchError::DnsResolution(_) => Self::transient(e.to_string())
                .with_next_step("Check the URL's domain name and your DNS resolver"),
            // `is_transient_network` covers both connect and timeout, but
            // ADR-0065 splits timeout into 124. Check `is_timeout()` first.
            FetchError::Http(re) if re.is_timeout() => timeout_with_retry_hint(&e),
            FetchError::Http(re) if is_transient_network(re) => transient_with_network_hint(&e),
            // Priority 5 sibling: IO_ERROR — external tool failure (browser)
            FetchError::BrowserFailed(_) => Self::io_error(e.to_string()),
            // Unknown — reqwest errors that do not match transient network patterns
            FetchError::Http(_) => Self::unknown(e.to_string()),
        }
    }
}

impl From<SlackError> for ScoutError {
    fn from(e: SlackError) -> Self {
        match &e {
            // Priority 1: USAGE_ERROR
            SlackError::TokenNotSet => Self::user_error(e.to_string())
                .with_next_step("Export a User OAuth token to SLACK_TOKEN (xoxp-…)"),
            // Priority 2: DATA_ERROR (insecure URL — peer to BraveError::InsecureBaseUrl)
            SlackError::InsecureUrl => Self::data_error(e.to_string()),
            // Slack API surfaces failures as error code strings (not HTTP status),
            // so per-string classification replaces the priority-2 HTTP arm.
            SlackError::Api { error } => match error.as_str() {
                // Priority 3: NOT_FOUND
                "channel_not_found" | "message_not_found" | "thread_not_found" => {
                    Self::not_found(e.to_string())
                }
                // Priority 4: TEMP_FAILURE
                "internal_error" | "service_unavailable" | "fatal_error" => {
                    transient_with_retry_hint(&e)
                }
                // Priority 1: USAGE_ERROR (invalid_auth, missing_scope, etc.)
                _ => Self::user_error(e.to_string()),
            },
            // Priority 4: TEMP_FAILURE
            SlackError::RateLimited { .. } => transient_with_retry_hint(&e),
            SlackError::Network(_) => transient_with_network_hint(&e),
            // Priority 4: TIMEOUT
            SlackError::Timeout(_) => timeout_with_retry_hint(&e),
            // Priority 5: INTERNAL — scout-side bug (unexpected schema)
            SlackError::Decode(_) => Self::internal_bug(e.to_string()),
        }
    }
}

impl From<BraveError> for ScoutError {
    fn from(e: BraveError) -> Self {
        match &e {
            // Priority 1: USAGE_ERROR / config
            BraveError::ApiKeyNotSet => Self::user_error(e.to_string())
                .with_next_step("Set BRAVE_SEARCH_API_KEY environment variable"),
            BraveError::Unauthorized => Self::user_error(e.to_string()).with_next_step(
                "Verify BRAVE_SEARCH_API_KEY at https://api-dashboard.search.brave.com/",
            ),
            // Priority 2: DATA_ERROR (4xx body, URL parse failure, or insecure base URL)
            BraveError::ParseUrl(_) | BraveError::InsecureBaseUrl => {
                Self::data_error(e.to_string())
            }
            BraveError::Api { code, .. } if (400..500).contains(code) => {
                Self::data_error(e.to_string())
            }
            // Priority 4: TIMEOUT
            BraveError::Network(re) if re.is_timeout() => timeout_with_retry_hint(&e),
            // Priority 4: TEMP_FAILURE
            BraveError::RateLimited { .. } => transient_with_retry_hint(&e),
            BraveError::Server(_) => transient_with_retry_hint(&e),
            BraveError::Network(_) => transient_with_network_hint(&e),
            BraveError::Api { code, .. } if (500..=599).contains(code) => {
                transient_with_retry_hint(&e)
            }
            // Priority 5: INTERNAL — schema drift is a scout-side invariant;
            // peer to `GitHubError::Decode` / `SlackError::Decode`.
            BraveError::ParseJson(_) => Self::internal_bug(e.to_string()),
            // Unknown — Api codes that did not match 4xx or 5xx
            BraveError::Api { .. } => Self::unknown(e.to_string()),
        }
    }
}

/// Unwrap a `Result<Vec<T>, GitHubError>` returning the value on success, or
/// push a degradation entry (paired `notes` message + typed `reason`) on
/// failure and return an empty vec. Per ADR-0003, callers supply only the
/// typed `reason`; the human-readable label is derived from the variant via
/// [`DegradedReason::label`] so the `(label, reason)` pair stays in sync.
pub(super) fn unwrap_or_degraded<T>(
    result: Result<Vec<T>, github::GitHubError>,
    reason: DegradedReason,
    degradation: &mut Degradation,
) -> Vec<T> {
    match result {
        Ok(v) => v,
        Err(e) => {
            let label = reason.label();
            warn!(%e, "failed to fetch {}", label);
            degradation.push(format!("Could not fetch {label} ({e})"), reason);
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

    /// [T-ER012] io_error returns ErrorCode::IoError
    #[test]
    fn io_error_kind_is_io_error() {
        let err = ScoutError::io_error("test");
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

    /// [T-NS001] ApiKeyNotSet sets next_step pointing to BRAVE_SEARCH_API_KEY env var
    #[test]
    fn brave_api_key_not_set_has_next_step() {
        let err = ScoutError::from(BraveError::ApiKeyNotSet);
        assert_eq!(
            err.next_step(),
            Some("Set BRAVE_SEARCH_API_KEY environment variable")
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

    /// [T-NS005] BraveError::Unauthorized points users at the Brave dashboard
    #[test]
    fn brave_unauthorized_separates_dashboard_hint() {
        let err = ScoutError::from(BraveError::Unauthorized);
        assert!(
            err.next_step()
                .is_some_and(|h| h.contains("api-dashboard.search.brave.com"))
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
        let err = ScoutError::io_error("io failure");
        let display = err.to_string();
        assert_eq!(display, "io failure");
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

    /// [T-ER020] SlackError::Api with internal_error classifies as TempFailure (ADR-0003)
    #[test]
    fn slack_internal_error_classifies_as_temp_failure() {
        use crate::slack::SlackError;
        let err = ScoutError::from(SlackError::Api {
            error: "internal_error".to_owned(),
        });
        assert_eq!(err.error_kind(), ErrorCode::TempFailure);
    }

    /// [T-ER021] SlackError::Api with channel_not_found classifies as NotFound (ADR-0003)
    #[test]
    fn slack_channel_not_found_classifies_as_not_found() {
        use crate::slack::SlackError;
        let err = ScoutError::from(SlackError::Api {
            error: "channel_not_found".to_owned(),
        });
        assert_eq!(err.error_kind(), ErrorCode::NotFound);
    }

    /// [T-ER022] SlackError::Api with other error codes (e.g., invalid_auth) classifies as UsageError
    #[test]
    fn slack_other_api_error_classifies_as_usage_error() {
        use crate::slack::SlackError;
        let err = ScoutError::from(SlackError::Api {
            error: "invalid_auth".to_owned(),
        });
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-ER023] ADR-0065 priority 2 wins over priority 5 for Api 4xx codes.
    /// Prior to the priority rule reflection, `GitHubError::Api { code: 4xx }` and
    /// `BraveError::Api { code: 4xx }` folded onto `internal()` (IoError, exit 74).
    /// Per ADR-0065 they must classify as DataError (exit 65).
    #[test]
    fn api_4xx_classifies_as_data_error_per_priority_2() {
        let github_400 = ScoutError::from(github::GitHubError::Api {
            code: 400,
            message: "bad request".into(),
        });
        let github_422 = ScoutError::from(github::GitHubError::Api {
            code: 422,
            message: "unprocessable entity".into(),
        });
        let brave_400 = ScoutError::from(BraveError::Api {
            code: 400,
            message: "err".into(),
        });
        for err in [&github_400, &github_422, &brave_400] {
            assert_eq!(err.error_kind(), ErrorCode::DataError, "{err}");
            assert_eq!(err.exit_code(), 65, "{err}");
            assert!(!err.retryable(), "4xx must not be retryable: {err}");
        }
    }

    /// [T-ER024] ADR-0065 priority 4 (TEMP_FAILURE) takes precedence for `Api { 5xx }`
    /// even though priority 5 (INTERNAL) could match the bare `Api { .. }` arm.
    /// Match-arm ordering enforces the priority ranking.
    #[test]
    fn api_5xx_classifies_as_temp_failure_per_priority_4() {
        let github_502 = ScoutError::from(github::GitHubError::Api {
            code: 502,
            message: "bad gateway".into(),
        });
        let brave_503 = ScoutError::from(BraveError::Api {
            code: 503,
            message: "unavailable".into(),
        });
        for err in [&github_502, &brave_503] {
            assert_eq!(err.error_kind(), ErrorCode::TempFailure, "{err}");
            assert_eq!(err.exit_code(), 75, "{err}");
            assert!(err.retryable(), "5xx must be retryable: {err}");
        }
    }

    /// [T-ER025] INTERNAL (70) reserved for scout-side schema bugs.
    /// `Decode` / `ParseJson` variants from GitHub, Slack, and Brave APIs signal
    /// an unexpected response shape — by ADR-0065 priority 5 these are scout's
    /// invariant violation, not external IO failure (which maps to IoError 74).
    #[test]
    fn schema_decode_classifies_as_internal_exit_70() {
        let serde_err =
            serde_json::from_str::<serde_json::Value>("{not valid").expect_err("malformed json");
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::Decode("decode error".into()).into(),
            SlackError::Decode("err".into()).into(),
            BraveError::ParseJson(serde_err).into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::Internal, "{err}");
            assert_eq!(err.exit_code(), 70, "expected EX_SOFTWARE (70): {err}");
            assert!(!err.retryable(), "Internal must not be retryable: {err}");
        }
    }

    /// [T-ER030] GitHub `Api { code: 401 }` classifies as UsageError(64) with auth hint
    /// (issue #101).
    ///
    /// Prior to this fix, 401 fell through the generic `(400..500)` DataError arm
    /// (exit 65) because the GitHubClient surfaces every non-special 4xx as
    /// `GitHubError::Api`. 401 is an auth-class failure — the user must set
    /// `GITHUB_TOKEN` or run `gh auth login` — so ADR-0065 priority 1 (USAGE_ERROR)
    /// is the correct landing, peer to `GitHubError::Forbidden`.
    #[test]
    fn github_401_classifies_as_usage_error_with_auth_hint() {
        let err = ScoutError::from(github::GitHubError::Api {
            code: 401,
            message: "Bad credentials".into(),
        });
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
        assert_eq!(err.exit_code(), 64, "expected EX_USAGE (64)");
        assert!(
            err.next_step().is_some_and(|h| h.contains("GITHUB_TOKEN")),
            "expected auth hint mentioning GITHUB_TOKEN, got: {:?}",
            err.next_step()
        );
    }

    /// [T-ER026] UNKNOWN (104) is the escape hatch for Api codes that match
    /// neither 4xx (priority 2) nor 5xx (priority 4). Exit 104 is the PJ
    /// extension reserved by ADR-0065 §Classification Priority. A rising rate
    /// of Unknown signals the classification design needs revisiting.
    #[test]
    fn unclassified_api_classifies_as_unknown_exit_104() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::Api {
                code: 304,
                message: "not modified".into(),
            }
            .into(),
            BraveError::Api {
                code: 304,
                message: "not modified".into(),
            }
            .into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::Unknown, "{err}");
            assert_eq!(err.exit_code(), 104, "expected PJ extension (104): {err}");
            assert!(!err.retryable(), "Unknown must not be retryable: {err}");
        }
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
            BraveError::ApiKeyNotSet.into(),
            BraveError::Unauthorized.into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::UsageError, "{err}");
            assert_eq!(err.exit_code(), 64, "expected EX_USAGE (64): {err}");
        }
    }

    /// [T-ER001b] DataError errors surface with exit 65 (EX_DATAERR per ADR-0002).
    /// Per ADR-0065 priority 2, `*Error::Api { code }` 4xx (other than 401/403/404) now
    /// routes to DataError instead of folding onto IoError via `internal()`.
    #[test]
    fn data_errors_have_exit_code_65() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::InvalidRepo("bad".into()).into(),
            github::GitHubError::Api {
                code: 400,
                message: "bad request".into(),
            }
            .into(),
            github::GitHubError::Api {
                code: 422,
                message: "unprocessable entity".into(),
            }
            .into(),
            FetchError::InvalidScheme.into(),
            FetchError::InternalHost.into(),
            FetchError::UnsupportedContentType("image/png".into()).into(),
            FetchError::RedirectMissingLocation.into(),
            FetchError::Status(400).into(),
            FetchError::Status(499).into(),
            FetchError::TooLarge.into(),
            FetchError::TooManyRedirects(10).into(),
            BraveError::Api {
                code: 400,
                message: "err".into(),
            }
            .into(),
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

    /// [T-ER002] IoError errors surface with exit 74 (EX_IOERR) and are non-retryable.
    /// Reserved for external-tool IO failures (browser); scout-side schema bugs
    /// route to `Internal(70)` (T-ER025) and unclassifiable Api codes to
    /// `Unknown(104)` (T-ER026).
    #[test]
    fn io_errors_have_exit_code_74() {
        let cases: Vec<ScoutError> =
            vec![FetchError::BrowserFailed("CDP protocol error".into()).into()];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::IoError, "{err}");
            assert_eq!(err.exit_code(), 74, "expected EX_IOERR (74): {err}");
            assert!(!err.retryable(), "IoError should not be retryable: {err}");
        }
    }

    /// [T-ER003] TempFailure errors are retryable, display retry hint, exit 75 (EX_TEMPFAIL).
    /// Timeout cases moved to T-ER027 with exit 124 per ADR-0065.
    #[test]
    fn temp_failure_errors_have_exit_code_75() {
        let cases: Vec<ScoutError> = vec![
            FetchError::Status(408).into(),
            FetchError::Status(429).into(),
            FetchError::Status(500).into(),
            FetchError::Status(503).into(),
            FetchError::DnsResolution("dns failed".into()).into(),
            github::GitHubError::RateLimited { retry_after: None }.into(),
            github::GitHubError::Api {
                code: 502,
                message: "bad gateway".into(),
            }
            .into(),
            BraveError::RateLimited { retry_after: None }.into(),
            BraveError::Api {
                code: 503,
                message: "unavailable".into(),
            }
            .into(),
            SlackError::RateLimited { retry_after: None }.into(),
            SlackError::Network("err".into()).into(),
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

    /// [T-ER027] Timeout errors are retryable, surface exit 124 (GNU coreutils
    /// `timeout`) independent from TempFailure(75). The split lets caller
    /// scripts apply a longer retry backoff than for rate-limit / 5xx since
    /// timeouts imply an unknown counterparty load condition.
    #[test]
    fn timeout_errors_have_exit_code_124() {
        let cases: Vec<ScoutError> = vec![
            FetchError::Timeout("timed out".into()).into(),
            SlackError::Timeout("timed out".into()).into(),
        ];
        for err in &cases {
            assert_eq!(err.error_kind(), ErrorCode::Timeout, "{err}");
            assert!(err.retryable(), "Timeout must be retryable: {err}");
            assert_eq!(err.exit_code(), 124, "expected 124 (GNU timeout): {err}");
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
