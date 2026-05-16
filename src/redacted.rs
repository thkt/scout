use std::fmt;

use crate::brave::client::BraveError;

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

/// Returns `Ok(())` when `url` begins with `https://`; otherwise yields
/// `BraveError::InsecureBaseUrl`. Used as a defense-in-depth check before
/// sending an API key over HTTP.
///
/// Unlike [`assert_https`], this function returns a `Result` and is never
/// bypassed in test builds, so callers can exercise the rejection path
/// directly.
pub(crate) fn validate_https(url: &str) -> Result<(), BraveError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(BraveError::InsecureBaseUrl)
    }
}

/// Panicking HTTPS check retained for backends that have not migrated to
/// the [`validate_https`] `Result` form. Bypassed under `cfg!(test)` so
/// wiremock servers on `http://127.0.0.1` keep working.
// FIXME: callers `github.rs::request` and `slack.rs::api_get_once` still
//        use this panic form; migrate both to validate_https + a per-client
//        skip flag (parallel to BraveClient::skip_https_check) and delete
//        this function.
pub(crate) fn assert_https(url: &str) {
    assert!(
        url.starts_with("https://") || cfg!(test),
        "credentials must only be sent over HTTPS"
    );
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
    /// `BraveError::InsecureBaseUrl`.
    #[test]
    fn validate_https_rejects_non_https_and_empty() {
        let http_result = validate_https("http://insecure.example");
        assert!(
            matches!(http_result, Err(BraveError::InsecureBaseUrl)),
            "expected InsecureBaseUrl for http URL, got: {http_result:?}"
        );

        let empty_result = validate_https("");
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
        let result = validate_https("https://api.search.brave.com/res/v1/web/search");
        assert!(matches!(result, Ok(())), "expected Ok, got: {result:?}");
    }
}
