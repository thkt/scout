//! Output envelopes per ADR-0065 (scout JSON output schema).
//!
//! Production wiring lands in Phase 2.2 (`--json` global flag); Phase 2.1
//! exposes the types so `ScoutError::error_kind()` can classify against them.

use serde::Serialize;

/// JSON-serializable error classification per ADR-0065.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ErrorCode {
    UsageError,
    DataError,
    NotFound,
    IoError,
    TempFailure,
}

/// Success envelope wrapping command output per ADR-0065.
#[allow(dead_code)] // consumed in Phase 2.2 (--json output path)
#[derive(Debug, Serialize)]
pub(crate) struct SuccessEnvelope {
    pub data: serde_json::Value,
    pub degraded: bool,
    pub notes: Vec<String>,
}

/// Error envelope per ADR-0065. Wraps the payload under an `error` key so
/// JSON output matches `{"error": { "code": ..., "message": ..., ... }}`.
#[allow(dead_code)] // consumed in Phase 2.2 (--json output path)
#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub error: ErrorPayload,
}

/// Error payload nested under `ErrorEnvelope::error` per ADR-0065.
#[allow(dead_code)] // consumed in Phase 2.2 (--json output path)
#[derive(Debug, Serialize)]
pub(crate) struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
    pub retryable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-EN002] ErrorPayload omits next_step when None
    #[test]
    fn t_en002_error_payload_omits_optional_next_step() {
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

    /// [T-EN003] ErrorPayload omits candidates when empty
    #[test]
    fn t_en003_error_payload_omits_empty_candidates() {
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

    /// [T-EN004] ErrorPayload includes next_step and candidates when present
    #[test]
    fn t_en004_error_payload_includes_present_optional_fields() {
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

    /// [T-EN007] ErrorEnvelope wraps payload under `error` key per ADR-0065
    #[test]
    fn t_en007_error_envelope_wraps_payload_under_error_key() {
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
            "envelope should start with `{{\"error\":` per ADR-0065, got: {json}"
        );
        assert!(
            json.contains(r#""code":"USAGE_ERROR""#),
            "payload should contain code, got: {json}"
        );
    }

    /// [T-EN005] SuccessEnvelope serializes data + degraded + notes
    #[test]
    fn t_en005_success_envelope_serializes_required_fields() {
        let env = SuccessEnvelope {
            data: serde_json::json!({"markdown": "hello"}),
            degraded: false,
            notes: vec![],
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            json.contains(r#""data":{"markdown":"hello"}"#),
            "got: {json}"
        );
        assert!(json.contains(r#""degraded":false"#), "got: {json}");
        assert!(json.contains(r#""notes":[]"#), "got: {json}");
    }

    /// [T-EN006] SuccessEnvelope surfaces degraded=true with notes
    #[test]
    fn t_en006_success_envelope_surfaces_degradation() {
        let env = SuccessEnvelope {
            data: serde_json::json!(null),
            degraded: true,
            notes: vec![String::from("Could not fetch contributors")],
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""degraded":true"#), "got: {json}");
        assert!(
            json.contains(r#""notes":["Could not fetch contributors"]"#),
            "got: {json}"
        );
    }

    /// [T-EN001] ErrorCode serializes per ADR-0065 SCREAMING_SNAKE_CASE
    #[test]
    fn t_en001_error_code_serializes_screaming_snake_case() {
        let pairs = [
            (ErrorCode::UsageError, r#""USAGE_ERROR""#),
            (ErrorCode::DataError, r#""DATA_ERROR""#),
            (ErrorCode::NotFound, r#""NOT_FOUND""#),
            (ErrorCode::IoError, r#""IO_ERROR""#),
            (ErrorCode::TempFailure, r#""TEMP_FAILURE""#),
        ];
        for (code, expected) in pairs {
            let actual = serde_json::to_string(&code).unwrap();
            assert_eq!(
                actual, expected,
                "code {code:?} should serialize as {expected}"
            );
        }
    }
}
