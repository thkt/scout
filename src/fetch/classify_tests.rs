use super::*;

/// [T-FEC001]
#[test]
fn browser_not_found_is_usage_error() {
    let c = FetchError::BrowserNotFound("not installed".into()).classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
}

/// [T-FEC002] priority 1 over the priority-2 4xx fallback
#[test]
fn status_401_403_is_usage_error_not_data_error() {
    for code in [401u16, 403] {
        let c = FetchError::Status(code).classify();
        assert_eq!(
            c.kind,
            ErrorCode::UsageError,
            "code {code} must precede 4xx arm"
        );
    }
}

/// [T-FEC003] priority 3 over the priority-2 4xx fallback
#[test]
fn status_404_is_not_found_not_data_error() {
    let c = FetchError::Status(404).classify();
    assert_eq!(c.kind, ErrorCode::NotFound);
    assert!(
        c.next_step.as_deref().is_some_and(|h| h.contains("URL")),
        "expected URL hint, got: {:?}",
        c.next_step
    );
}

/// [T-FEC004] priority 4 ahead of the priority-2 4xx fallback.
#[test]
fn status_408_429_is_temp_failure_not_data_error() {
    for code in [408u16, 429] {
        let c = FetchError::Status(code).classify();
        assert_eq!(c.kind, ErrorCode::TempFailure, "code {code}");
    }
}

/// [T-FEC005]
#[test]
fn status_other_4xx_is_data_error() {
    for code in [400u16, 410, 422, 499] {
        let c = FetchError::Status(code).classify();
        assert_eq!(c.kind, ErrorCode::DataError, "code {code}");
    }
}

/// [T-FEC006]
#[test]
fn status_5xx_is_temp_failure() {
    for code in [500u16, 502, 503, 599] {
        let c = FetchError::Status(code).classify();
        assert_eq!(c.kind, ErrorCode::TempFailure, "code {code}");
    }
}

/// [T-FEC007] Priority-2 DataError variants (non-Status) classify as DataError.
#[test]
fn data_error_variants_classify_as_data_error() {
    let cases: Vec<FetchError> = vec![
        FetchError::InvalidScheme,
        FetchError::InternalHost,
        FetchError::UnsupportedContentType("image/png".into()),
        FetchError::RedirectMissingLocation,
        FetchError::TooLarge,
        FetchError::TooManyRedirects(10),
    ];
    for case in &cases {
        assert_eq!(
            case.classify().kind,
            ErrorCode::DataError,
            "{case:?} must classify as DataError"
        );
    }
}

/// [T-FEC008] Timeout classifies as Timeout (exit 124 split from TempFailure).
#[test]
fn timeout_is_timeout_kind() {
    let c = FetchError::Timeout("timed out".into()).classify();
    assert_eq!(c.kind, ErrorCode::Timeout);
}

/// [T-FEC009]
#[test]
fn dns_resolution_is_temp_failure_with_dns_hint() {
    let c = FetchError::DnsResolution("dns failed".into()).classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert!(
        c.next_step.as_deref().is_some_and(|h| h.contains("DNS")),
        "expected DNS hint, got: {:?}",
        c.next_step
    );
}

/// [T-FEC010] BrowserFailed classifies as IoError (priority 5 sibling).
#[test]
fn browser_failed_is_io_error() {
    let c = FetchError::BrowserFailed("CDP error".into()).classify();
    assert_eq!(c.kind, ErrorCode::IoError);
}

/// [T-FC018] 変換が失敗したとき exit code 65 で終わる
///
/// `MarkdownConversion` carries the htmd conversion failure (fail-close) and
/// must classify the same way `UnsupportedContentType`
/// does: `Classification::DataError`, which `ErrorCode::exit_code`
/// (src/envelope.rs) maps to process exit code 65 (EX_DATAERR).
#[test]
fn markdown_conversion_failure_is_data_error() {
    let c = FetchError::MarkdownConversion("unexpected end of input".into()).classify();
    assert_eq!(c.kind, ErrorCode::DataError);
}
