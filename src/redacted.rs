#[derive(Clone)]
pub(crate) struct Redacted(String);

impl Redacted {
    pub fn new(s: String) -> Self {
        Self(s.trim().to_string())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

    #[test]
    fn debug_is_redacted() {
        let secret = Redacted::new("super-secret".into());
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
    }
}
