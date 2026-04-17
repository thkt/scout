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

pub(crate) fn assert_https(url: &str) {
    assert!(
        url.starts_with("https://") || cfg!(test),
        "credentials must only be sent over HTTPS"
    );
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
}
