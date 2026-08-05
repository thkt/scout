use std::env;
use std::fmt;

/// A secret value that hides its contents from `Debug` formatting.
/// Construction returns `None` for empty/whitespace input so callers
/// cannot store an effectively missing credential.
#[derive(Clone)]
pub(crate) struct Redacted(String);

impl Redacted {
    /// Construct a redacted secret. Returns `None` when `s` is empty or
    /// contains only whitespace; otherwise stores the trimmed value.
    pub fn new(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_owned()))
        }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Reads `name` via `get_var` and constructs a `Redacted` from it,
    /// yielding the error produced by `err` when the variable is unset OR
    /// its value is empty/whitespace-only. Unset and blank collapse to the
    /// same injected error so callers cannot tell the two apart from the
    /// error alone; each backend supplies its own missing-credential
    /// variant, keeping this helper decoupled from any single client's
    /// error enum (mirrors `validate_https`). Prefix/shape checks specific
    /// to one backend (e.g. Slack's `xoxp-` token prefix) stay at the call
    /// site instead of here.
    pub(crate) fn from_env_var<F, E>(
        name: &str,
        get_var: F,
        err: impl FnOnce() -> E,
    ) -> Result<Self, E>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        get_var(name)
            .ok()
            .and_then(|raw| Self::new(&raw))
            .ok_or_else(err)
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
        let secret = Redacted::new("super-secret").expect("static literal is non-empty");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
    }

    /// [T-RD002]
    #[test]
    fn new_rejects_empty_input() {
        assert!(Redacted::new("").is_none());
    }

    /// [T-RD003]
    #[test]
    fn new_rejects_whitespace_only_input() {
        assert!(Redacted::new("   ").is_none());
        assert!(Redacted::new("\t\n").is_none());
    }

    /// [T-RD004]
    #[test]
    fn new_trims_surrounding_whitespace() {
        let secret = Redacted::new("  abc  ").expect("non-empty after trim");
        assert_eq!(secret.expose(), "abc");
    }

    /// [T-RD005] FR-004 / FR-005: `validate_https` rejects any input that does not begin
    /// with `https://` (plain `http://`, empty string) and accepts a real
    /// `https://` URL.
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

    /// [T-002] 未設定の env 変数は注入された欠落エラーになる
    #[test]
    fn unset_env_var_becomes_the_injected_missing_error() {
        #[derive(Debug, PartialEq, Eq)]
        struct CallerError;

        let get_var = |_: &str| Err(env::VarError::NotPresent);
        let result: Result<Redacted, CallerError> =
            Redacted::from_env_var("SOME_VAR", get_var, || CallerError);
        assert_eq!(result.err(), Some(CallerError));
    }

    /// [T-003] 空白のみの env 値も同じ欠落エラーになる
    #[test]
    fn whitespace_only_env_value_becomes_the_same_missing_error() {
        #[derive(Debug, PartialEq, Eq)]
        struct CallerError;

        let get_var = |_: &str| Ok("   ".to_owned());
        let result: Result<Redacted, CallerError> =
            Redacted::from_env_var("SOME_VAR", get_var, || CallerError);
        assert_eq!(result.err(), Some(CallerError));
    }
}
