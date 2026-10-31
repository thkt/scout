use super::*;

/// [T-FEC001]
#[test]
fn browser_not_found_is_usage_error() {
    let c = FetchError::BrowserNotFound("not installed".into()).classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
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
