use super::*;

/// [T-BRC001] ApiKeyNotSet classifies as UsageError with BRAVE_SEARCH_API_KEY hint.
#[test]
fn api_key_not_set_is_usage_error_with_key_hint() {
    let c = BraveError::ApiKeyNotSet.classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("BRAVE_SEARCH_API_KEY")),
        "expected BRAVE_SEARCH_API_KEY hint, got: {:?}",
        c.next_step
    );
}

/// [T-BRC002] Unauthorized classifies as UsageError with a Brave dashboard hint.
#[test]
fn unauthorized_is_usage_error_with_dashboard_hint() {
    let c = BraveError::Unauthorized.classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("api-dashboard.search.brave.com")),
        "expected Brave dashboard hint, got: {:?}",
        c.next_step
    );
}

/// [T-BRC003] Priority-2 DataError variants classify as DataError.
#[test]
fn data_error_variants_classify_as_data_error() {
    let cases: Vec<BraveError> = vec![
        BraveError::InsecureBaseUrl,
        BraveError::Api {
            code: 400,
            message: "bad".into(),
        },
        BraveError::Api {
            code: 422,
            message: "unprocessable".into(),
        },
    ];
    for case in &cases {
        assert_eq!(case.classify().kind, ErrorCode::DataError, "{case:?}");
    }
}

/// [T-BRC004] Server (5xx) and RateLimited classify as TempFailure.
#[test]
fn server_and_rate_limited_are_temp_failure() {
    let cases: Vec<BraveError> = vec![
        BraveError::Server(503),
        BraveError::RateLimited { retry_after: None },
        BraveError::Api {
            code: 502,
            message: "bad gateway".into(),
        },
    ];
    for case in &cases {
        assert_eq!(case.classify().kind, ErrorCode::TempFailure, "{case:?}");
    }
}

/// [T-BRC005] Schema drift variants classify as Internal per ADR-0011 priority 5.
/// `ResponseTooLarge` and `ParseJson` both signal an upstream invariant violation
/// that retry will not recover from.
#[test]
fn schema_drift_is_internal() {
    let serde_err =
        serde_json::from_str::<serde_json::Value>("{not valid").expect_err("malformed json");
    let cases: Vec<BraveError> = vec![
        BraveError::ResponseTooLarge,
        BraveError::ParseJson(serde_err),
    ];
    for case in &cases {
        assert_eq!(case.classify().kind, ErrorCode::Internal, "{case:?}");
    }
}

/// [T-BRC006] Api codes outside 4xx/5xx classify as Unknown (escape hatch).
#[test]
fn api_non_4xx_5xx_is_unknown() {
    let c = BraveError::Api {
        code: 304,
        message: "not modified".into(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::Unknown);
}

/// [T-BRC007] is_degradable is true exactly for TempFailure/Timeout infra faults.
#[test]
fn temp_failure_variants_are_degradable() {
    let cases: Vec<BraveError> = vec![
        BraveError::Server(503),
        BraveError::RateLimited { retry_after: None },
        BraveError::Api {
            code: 502,
            message: "bad gateway".into(),
        },
    ];
    for case in &cases {
        assert!(case.is_degradable(), "{case:?}");
    }
}

/// [T-BRC008] is_degradable is false for config, data, internal, and Unknown
/// variants. The `Api { code: 304 }` case pins the classify-derived behavior
/// against T-BRC006: an Unknown status propagates rather than degrading.
#[test]
fn non_temp_failure_variants_are_not_degradable() {
    let cases: Vec<BraveError> = vec![
        BraveError::ApiKeyNotSet,
        BraveError::Unauthorized,
        BraveError::InsecureBaseUrl,
        BraveError::ResponseTooLarge,
        BraveError::Api {
            code: 400,
            message: "bad".into(),
        },
        BraveError::Api {
            code: 304,
            message: "not modified".into(),
        },
    ];
    for case in &cases {
        assert!(!case.is_degradable(), "{case:?}");
    }
}
