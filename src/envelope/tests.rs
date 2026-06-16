use super::*;

/// [T-EN001] ErrorPayload omits next_step when None
#[test]
fn error_payload_omits_optional_next_step() {
    let payload = ErrorPayload {
        code: ErrorCode::UsageError,
        message: String::from("Missing query"),
        next_step: None,
        candidates: vec![],
        retryable: false,
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(
        !json.contains("next_step"),
        "next_step should be omitted when None, got: {json}"
    );
}

/// [T-EN002] ErrorPayload omits candidates when empty
#[test]
fn error_payload_omits_empty_candidates() {
    let payload = ErrorPayload {
        code: ErrorCode::UsageError,
        message: String::from("invalid"),
        next_step: None,
        candidates: vec![],
        retryable: false,
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(
        !json.contains("candidates"),
        "candidates should be omitted when empty, got: {json}"
    );
}

/// [T-EN003] ErrorPayload includes next_step and candidates when present
#[test]
fn error_payload_includes_present_optional_fields() {
    let payload = ErrorPayload {
        code: ErrorCode::UsageError,
        message: String::from("did you mean"),
        next_step: Some(String::from("Pass <QUERY>")),
        candidates: vec![String::from("query"), String::from("queries")],
        retryable: false,
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(
        json.contains(r#""next_step":"Pass <QUERY>""#),
        "got: {json}"
    );
    assert!(
        json.contains(r#""candidates":["query","queries"]"#),
        "got: {json}"
    );
}

/// [T-EN004] ErrorEnvelope wraps payload under `error` key per ADR-0010
#[test]
fn error_envelope_wraps_payload_under_error_key() {
    let env = ErrorEnvelope {
        error: ErrorPayload {
            code: ErrorCode::UsageError,
            message: String::from("Missing query"),
            next_step: None,
            candidates: vec![],
            retryable: false,
        },
    };
    let json = serde_json::to_string(&env).unwrap();
    assert!(
        json.starts_with(r#"{"error":"#),
        "envelope should start with `{{\"error\":` per ADR-0010, got: {json}"
    );
    assert!(
        json.contains(r#""code":"USAGE_ERROR""#),
        "payload should contain code, got: {json}"
    );
}

/// [T-EN016] `ErrorEnvelope::to_json_line` is the single serialize point per
/// ADR-0010 and emits the same one-line JSON as direct `serde_json::to_string`.
#[test]
fn error_envelope_to_json_line_matches_direct_serialize() {
    let env = ErrorEnvelope {
        error: ErrorPayload {
            code: ErrorCode::Internal,
            message: String::from("failed to serialize fetch result"),
            next_step: None,
            candidates: vec![],
            retryable: false,
        },
    };
    let line = env.to_json_line();
    assert_eq!(line, serde_json::to_string(&env).unwrap());
    assert!(line.starts_with(r#"{"error":"#), "got: {line}");
    assert!(line.contains(r#""code":"INTERNAL""#), "got: {line}");
    assert!(
        !line.contains('\n'),
        "envelope must be one line, got: {line}"
    );
}

/// [T-EN005] SuccessEnvelope serializes data + degraded + notes
#[test]
fn success_envelope_serializes_required_fields() {
    let env = SuccessEnvelope {
        data: serde_json::json!({"markdown": "hello"}),
        degraded: false,
        notes: vec![],
        degraded_reasons: vec![],
    };
    let json = serde_json::to_string(&env).unwrap();
    assert!(
        json.contains(r#""data":{"markdown":"hello"}"#),
        "got: {json}"
    );
    assert!(json.contains(r#""degraded":false"#), "got: {json}");
    assert!(json.contains(r#""notes":[]"#), "got: {json}");
    assert!(
        !json.contains("degraded_reasons"),
        "degraded_reasons should be omitted when empty per ADR-0003, got: {json}"
    );
}

/// [T-EN006] SuccessEnvelope surfaces degraded=true with notes
#[test]
fn success_envelope_surfaces_degradation() {
    let env = SuccessEnvelope {
        data: serde_json::json!(null),
        degraded: true,
        notes: vec![String::from("Could not fetch contributors")],
        degraded_reasons: vec![],
    };
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains(r#""degraded":true"#), "got: {json}");
    assert!(
        json.contains(r#""notes":["Could not fetch contributors"]"#),
        "got: {json}"
    );
}

/// [T-EN007] CommandOutput::ok produces non-degraded with empty notes
#[test]
fn command_output_ok_is_not_degraded() {
    let env = CommandOutput::ok(String::from("md"), serde_json::json!({"a": 1})).into_envelope();
    assert_eq!(env.data, serde_json::json!({"a": 1}));
    assert!(env.notes.is_empty());
    assert!(env.degraded_reasons.is_empty());
    assert!(!env.degraded);
}

/// [T-EN015] CommandOutput preserves the markdown body across into_markdown
#[test]
fn command_output_into_markdown_returns_body() {
    let out = CommandOutput::ok(String::from("md body"), serde_json::Value::Null);
    assert_eq!(out.into_markdown(), "md body");
}

/// [T-EN008] CommandOutput::with_degradation sets degraded=true when degradation non-empty
#[test]
fn command_output_with_degradation_is_degraded() {
    let mut deg = Degradation::default();
    deg.push(
        String::from("partial fetch"),
        DegradedReason::IssuesFetchFailed,
    );
    let out = CommandOutput::with_degradation(String::from("md"), serde_json::Value::Null, deg);
    let env = out.into_envelope();
    assert!(env.degraded);
    assert_eq!(env.notes, vec!["partial fetch"]);
    assert_eq!(
        env.degraded_reasons,
        vec![DegradedReason::IssuesFetchFailed]
    );
}

/// [T-EN009] CommandOutput::with_degradation sets degraded=false when degradation empty
#[test]
fn command_output_with_empty_degradation_is_not_degraded() {
    let out = CommandOutput::with_degradation(
        String::from("md"),
        serde_json::Value::Null,
        Degradation::default(),
    );
    let env = out.into_envelope();
    assert!(!env.degraded);
    assert!(env.degraded_reasons.is_empty());
}

/// [T-EN010] ErrorCode serializes per ADR-0010 SCREAMING_SNAKE_CASE
#[test]
fn error_code_serializes_screaming_snake_case() {
    let pairs = [
        (ErrorCode::UsageError, r#""USAGE_ERROR""#),
        (ErrorCode::DataError, r#""DATA_ERROR""#),
        (ErrorCode::NotFound, r#""NOT_FOUND""#),
        (ErrorCode::Internal, r#""INTERNAL""#),
        (ErrorCode::IoError, r#""IO_ERROR""#),
        (ErrorCode::TempFailure, r#""TEMP_FAILURE""#),
        (ErrorCode::Timeout, r#""TIMEOUT""#),
        (ErrorCode::Unknown, r#""UNKNOWN""#),
    ];
    for (code, expected) in pairs {
        let actual = serde_json::to_string(&code).unwrap();
        assert_eq!(
            actual, expected,
            "code {code:?} should serialize as {expected}"
        );
    }
}

/// [T-EN014] ErrorCode → exit-code mapping per ADR-0002 (exit values) +
/// ADR-0010 (9-code policy).
/// Locks the full table so adding a new variant without an `exit_code()`
/// arm fails compile, and drift on any existing variant fails this test.
#[test]
fn error_code_exit_code_table() {
    let pairs = [
        (ErrorCode::UsageError, 64),
        (ErrorCode::DataError, 65),
        (ErrorCode::NotFound, 66),
        (ErrorCode::Internal, 70),
        (ErrorCode::IoError, 74),
        (ErrorCode::TempFailure, 75),
        (ErrorCode::Unknown, 104),
        (ErrorCode::Timeout, 124),
    ];
    for (code, expected) in pairs {
        assert_eq!(
            code.exit_code(),
            expected,
            "{code:?} should exit {expected}"
        );
    }
}

/// [T-EN011] DegradedReason serializes per ADR-0003 SCREAMING_SNAKE_CASE
#[test]
fn degraded_reason_serializes_screaming_snake_case() {
    let pairs = [
        (
            DegradedReason::IssuesFetchFailed,
            r#""ISSUES_FETCH_FAILED""#,
        ),
        (DegradedReason::PullsFetchFailed, r#""PULLS_FETCH_FAILED""#),
        (
            DegradedReason::ReleasesFetchFailed,
            r#""RELEASES_FETCH_FAILED""#,
        ),
        (
            DegradedReason::ReadmeFetchFailed,
            r#""README_FETCH_FAILED""#,
        ),
        (
            DegradedReason::ReadmeBlobFetchFailed,
            r#""README_BLOB_FETCH_FAILED""#,
        ),
        (
            DegradedReason::ReadmeDecodeFailed,
            r#""README_DECODE_FAILED""#,
        ),
        (DegradedReason::UrlFetchFailed, r#""URL_FETCH_FAILED""#),
        (
            DegradedReason::ReadabilityFallback,
            r#""READABILITY_FALLBACK""#,
        ),
        (
            DegradedReason::BraveSearchFailed,
            r#""BRAVE_SEARCH_FAILED""#,
        ),
    ];
    for (reason, expected) in pairs {
        let actual = serde_json::to_string(&reason).unwrap();
        assert_eq!(
            actual, expected,
            "reason {reason:?} should serialize as {expected}"
        );
    }
}

/// [T-EN012] Degradation::push pairs notes and reasons in order
#[test]
fn degradation_push_pairs_notes_and_reasons() {
    let mut deg = Degradation::default();
    deg.push(String::from("first"), DegradedReason::IssuesFetchFailed);
    deg.push(String::from("second"), DegradedReason::PullsFetchFailed);
    assert!(!deg.is_empty());
    let (notes, reasons) = deg.into_parts();
    assert_eq!(notes, vec!["first", "second"]);
    assert_eq!(
        reasons,
        vec![
            DegradedReason::IssuesFetchFailed,
            DegradedReason::PullsFetchFailed,
        ]
    );
}

/// [T-EN013] SuccessEnvelope includes degraded_reasons when non-empty
#[test]
fn success_envelope_surfaces_degraded_reasons() {
    let env = SuccessEnvelope {
        data: serde_json::json!(null),
        degraded: true,
        notes: vec![String::from("Could not fetch issues")],
        degraded_reasons: vec![DegradedReason::IssuesFetchFailed],
    };
    let json = serde_json::to_string(&env).unwrap();
    assert!(
        json.contains(r#""degraded_reasons":["ISSUES_FETCH_FAILED"]"#),
        "got: {json}"
    );
}
