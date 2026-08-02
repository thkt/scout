use super::*;

/// [T-F007]
#[test]
fn accepts_textual_content_types() {
    for ct in [
        "text/html; charset=utf-8",
        "text/plain",
        "application/xhtml+xml",
        "application/xml",
        "; charset=utf-8", // edge: empty mime before semicolon → permissive
    ] {
        assert!(check_content_type(ct).is_ok(), "should accept: {ct}");
    }
}

/// [T-F008]
#[test]
fn rejects_non_textual_content_types() {
    for ct in ["application/pdf", "image/png", "application/json"] {
        assert!(
            matches!(
                check_content_type(ct),
                Err(FetchError::UnsupportedContentType(ref m)) if m == ct
            ),
            "should reject: {ct}"
        );
    }
}
