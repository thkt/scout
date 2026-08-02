use super::*;

/// [T-SLC001]
#[test]
fn token_not_set_is_usage_error_with_token_hint() {
    let c = SlackError::TokenNotSet.classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("SLACK_TOKEN")),
        "expected SLACK_TOKEN hint, got: {:?}",
        c.next_step
    );
}

/// [T-SLC011] Same caller-facing treatment as `TokenNotSet`: both are a
/// misconfigured credential the user must fix before retrying (issue #261).
#[test]
fn token_wrong_type_is_usage_error_with_token_hint() {
    let c = SlackError::TokenWrongType.classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("SLACK_TOKEN")),
        "expected SLACK_TOKEN hint, got: {:?}",
        c.next_step
    );
}

/// [T-SLC002] InsecureUrl classifies as DataError (peer to other backends' InsecureUrl).
#[test]
fn insecure_url_is_data_error() {
    let c = SlackError::InsecureUrl.classify();
    assert_eq!(c.kind, ErrorCode::DataError);
}

/// [T-SLC003] scout's internal "message not found" (space form) must classify the same
/// as Slack's `message_not_found` (underscore) — both should land on
/// EX_NOINPUT(66) per issue #114.
#[test]
fn api_not_found_codes_classify_as_not_found() {
    for code in [
        "channel_not_found",
        "message_not_found",
        "thread_not_found",
        "message not found",
    ] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(
            c.kind,
            ErrorCode::NotFound,
            "{code} must classify as NotFound"
        );
    }
}

/// [T-SLC010] scout's ts-bearing "message {ts} not found in thread" string
/// (built at client.rs when `extract_target` misses — target absent or in a
/// truncated page) classifies as NotFound, same as the bare "message not found"
/// form. Guards issue #224: the interpolated `{ts}` made the old exact-match arm
/// miss this string, dropping it to UsageError (exit 64) instead of NotFound
/// (exit 66).
#[test]
fn api_not_found_in_thread_with_ts_classifies_as_not_found() {
    let c = SlackError::Api {
        error: "message 1700000000.123456 not found in thread".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::NotFound);
}

/// [T-SLC004] ADR-0003 — internal_error must not be misclassified as UsageError.
#[test]
fn api_temp_failure_codes_classify_as_temp_failure() {
    for code in ["internal_error", "service_unavailable", "fatal_error"] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::TempFailure, "{code}");
    }
}

/// [T-SLC005]
#[test]
fn api_other_codes_classify_as_usage_error() {
    for code in ["invalid_auth", "missing_scope", "not_authed"] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::UsageError, "{code}");
    }
}

/// [T-SLC006] RateLimited classifies as TempFailure.
#[test]
fn rate_limited_is_temp_failure() {
    let c = SlackError::RateLimited { retry_after: None }.classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
}

/// [T-SLC007] Network classifies as TempFailure (network-class hint).
#[test]
fn network_is_temp_failure() {
    let c = SlackError::Network("connection reset".into()).classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
}

/// [T-SLC008] Timeout classifies as Timeout (exit 124 split from TempFailure).
#[test]
fn timeout_is_timeout_kind() {
    let c = SlackError::Timeout("timed out".into()).classify();
    assert_eq!(c.kind, ErrorCode::Timeout);
}

/// [T-SLC009] Decode (schema drift) classifies as Internal per ADR-0011 priority 5.
#[test]
fn decode_is_internal() {
    let c = SlackError::Decode("schema mismatch".into()).classify();
    assert_eq!(c.kind, ErrorCode::Internal);
}
