use std::fmt;

#[derive(Clone)]
pub(crate) struct Redacted(String);

impl Redacted {
    pub fn new(s: &str) -> Self {
        Self(s.trim().to_owned())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Returns `Ok(())` when `url` begins with `https://`; otherwise yields the
/// error produced by `err`. Each backend supplies its own error variant so
/// the helper stays decoupled from any single client's error enum. Callers
/// gate the call with a per-client `skip_https_check` flag (see
/// `BraveClient::should_check_https` / `GitHubClient::should_check_https` /
/// `SlackClient::should_check_https`) when targeting wiremock servers on
/// `http://127.0.0.1`.
pub(crate) fn validate_https<E>(url: &str, err: impl FnOnce() -> E) -> Result<(), E> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brave::client::BraveError;

    /// [T-RD001] Redacted value hides contents in Debug output
    #[test]
    fn debug_is_redacted() {
        let secret = Redacted::new("super-secret");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
    }

    // T-RC004: validate_https_rejects_non_https_and_empty
    /// FR-004 / FR-005: `validate_https` must reject any input that does not begin with
    /// `https://`, including plain `http://` URLs and the empty string. Both surface as
    /// the caller-supplied error variant.
    #[test]
    fn validate_https_rejects_non_https_and_empty() {
        let http_result = validate_https("http://insecure.example", || BraveError::InsecureBaseUrl);
        assert!(
            matches!(http_result, Err(BraveError::InsecureBaseUrl)),
            "expected InsecureBaseUrl for http URL, got: {http_result:?}"
        );

        let empty_result = validate_https("", || BraveError::InsecureBaseUrl);
        assert!(
            matches!(empty_result, Err(BraveError::InsecureBaseUrl)),
            "expected InsecureBaseUrl for empty URL, got: {empty_result:?}"
        );
    }

    // T-RC005: validate_https_accepts_https_url
    /// FR-004 / FR-005: a real `https://` URL (production `API_BASE`) must pass
    /// validation and return `Ok(())`.
    #[test]
    fn validate_https_accepts_https_url() {
        let result = validate_https("https://api.search.brave.com/res/v1/web/search", || {
            BraveError::InsecureBaseUrl
        });
        assert!(matches!(result, Ok(())), "expected Ok, got: {result:?}");
    }

    /// [T-RC007] `validate_https` is generic over the error type so each backend
    /// can plug its own variant. The closure runs only on failure; a passing
    /// URL never instantiates the error.
    #[test]
    fn validate_https_propagates_caller_supplied_error_type() {
        #[derive(Debug, PartialEq, Eq)]
        struct CallerError;

        let rejected: Result<(), CallerError> = validate_https("http://insecure", || CallerError);
        assert_eq!(rejected, Err(CallerError));

        let accepted: Result<(), CallerError> =
            validate_https("https://ok.example", || CallerError);
        assert_eq!(accepted, Ok(()));
    }
}
