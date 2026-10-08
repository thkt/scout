use super::*;

/// [T-F007]
#[test]
fn accepts_textual_content_types() {
    for (ct, expected) in [
        ("text/html; charset=utf-8", MediaType::Html),
        ("text/plain", MediaType::PlainText),
        ("application/xhtml+xml", MediaType::Html),
        ("application/xml", MediaType::OtherText),
        ("; charset=utf-8", MediaType::Unknown),
    ] {
        assert_eq!(check_content_type(ct).unwrap(), expected, "{ct}");
    }
}

/// [T-F079] Registered feed types must work as well as generic XML labels.
#[test]
fn accepts_the_xml_structured_syntax_suffix() {
    for ct in [
        "application/rss+xml; charset=UTF-8",
        "application/atom+xml",
        "application/xhtml+xml",
    ] {
        assert!(check_content_type(ct).is_ok(), "should accept: {ct}");
    }
}

/// [T-F080] Reject image serialization even when its suffix is `+xml`.
#[test]
fn rejects_the_xml_suffix_outside_application() {
    assert!(matches!(
        check_content_type("image/svg+xml"),
        Err(FetchError::UnsupportedContentType(_))
    ));
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

/// [T-F086] RFC 9110 section 8.3.1 makes media types case-insensitive.
#[test]
fn accepts_textual_content_types_regardless_of_ascii_case() {
    for ct in [
        "Text/HTML; charset=UTF-8",
        "TEXT/PLAIN",
        "APPLICATION/XML",
        "Application/Rss+Xml",
        "APPLICATION/ATOM+XML",
        "Application/Xhtml+Xml",
    ] {
        assert!(check_content_type(ct).is_ok(), "should accept: {ct}");
    }
}

/// [T-F087] Case folding does not broaden the allowed media-type families.
#[test]
fn rejects_mixed_case_non_textual_types_with_original_spelling() {
    for ct in ["IMAGE/SVG+XML", "Application/Pdf", "APPLICATION/JSON"] {
        assert!(
            matches!(check_content_type(ct), Err(FetchError::UnsupportedContentType(mime)) if mime == ct),
            "should reject and preserve the original type: {ct}"
        );
    }
}
