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
/// gate the call with a per-client `should_check_https` so wiremock servers
/// on `http://127.0.0.1` keep working in tests.
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

    /// [T-RD001] Redacted value hides contents in Debug output
    #[test]
    fn debug_is_redacted() {
        let secret = Redacted::new("super-secret");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
    }

    // T-RC007: validate_https_is_generic_over_caller_error_type
    /// FR-004 / FR-005: `validate_https` rejects any input that does not begin
    /// with `https://` (plain `http://`, empty string) and accepts a real
    /// `https://` URL. The error variant is whatever the closure constructs,
    /// so each backend plugs in its own type. The closure runs only on
    /// failure; a passing URL never instantiates the error.
    #[test]
    fn validate_https_is_generic_over_caller_error_type() {
        #[derive(Debug, PartialEq, Eq)]
        struct CallerError;

        let http: Result<(), CallerError> = validate_https("http://insecure", || CallerError);
        assert_eq!(http, Err(CallerError));

        let empty: Result<(), CallerError> = validate_https("", || CallerError);
        assert_eq!(empty, Err(CallerError));

        let https: Result<(), CallerError> = validate_https("https://ok.example", || CallerError);
        assert_eq!(https, Ok(()));
    }
}
