//! Output envelopes per ADR-0065 (scout JSON output schema) and ADR-0003
//! (degraded_reasons typed enum).
//!
//! `CommandOutput` is the internal shape produced by each command handler;
//! `lib::run` then serializes it as Markdown (default) or as a `SuccessEnvelope`
//! JSON line (when `--json` is set).

use serde::Serialize;

/// Typed reason for a degraded command output (partial failure) per ADR-0003.
/// Exposed under `degraded_reasons` in JSON output so callers can detect
/// specific failure modes programmatically rather than parsing free-form notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum DegradedReason {
    IssuesFetchFailed,
    PullsFetchFailed,
    ReleasesFetchFailed,
    ReadmeFetchFailed,
    ReadmeBlobFetchFailed,
    ReadmeDecodeFailed,
    UrlFetchFailed,
    ReadabilityFallback,
}

impl DegradedReason {
    /// Human-readable label used by [`crate::tools::errors::unwrap_or_degraded`]
    /// to build the `"Could not fetch {label} ({e})"` message. Only the three
    /// `*FetchFailed` variants that flow through that helper get a meaningful
    /// label; other variants build bespoke messages at their callsite.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::IssuesFetchFailed => "issues",
            Self::PullsFetchFailed => "pull requests",
            Self::ReleasesFetchFailed => "releases",
            Self::ReadmeFetchFailed
            | Self::ReadmeBlobFetchFailed
            | Self::ReadmeDecodeFailed
            | Self::UrlFetchFailed
            | Self::ReadabilityFallback => "resource",
        }
    }
}

/// Bundle of human-readable notes and typed reasons collected during a
/// degraded command path. The `(notes[i], reasons[i])` pairing invariant is
/// enforced by making the fields private and exposing [`Degradation::push`]
/// as the sole mutator.
#[derive(Debug, Default)]
pub(crate) struct Degradation {
    notes: Vec<String>,
    reasons: Vec<DegradedReason>,
}

impl Degradation {
    /// Push a human-readable message paired with its typed reason.
    pub fn push(&mut self, message: String, reason: DegradedReason) {
        self.notes.push(message);
        self.reasons.push(reason);
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.reasons.is_empty()
    }

    /// Read access to the human-readable notes for Markdown rendering.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Consume and return the underlying vectors.
    pub fn into_parts(self) -> (Vec<String>, Vec<DegradedReason>) {
        (self.notes, self.reasons)
    }
}

/// Internal command output: holds both the Markdown rendering and the
/// structured `data` payload, plus degradation signals. Each handler builds
/// one of these; `lib::run` picks the path (Markdown or JSON) at the boundary.
#[derive(Debug)]
pub(crate) struct CommandOutput {
    pub markdown: String,
    pub data: serde_json::Value,
    pub notes: Vec<String>,
    pub degraded_reasons: Vec<DegradedReason>,
    pub degraded: bool,
}

impl CommandOutput {
    /// Construct an output with no degradation signal.
    pub fn ok(markdown: String, data: serde_json::Value) -> Self {
        Self {
            markdown,
            data,
            notes: Vec::new(),
            degraded_reasons: Vec::new(),
            degraded: false,
        }
    }

    /// Construct an output from a [`Degradation`] bundle. `degraded` is set
    /// when either `notes` or `reasons` is non-empty.
    pub fn with_degradation(
        markdown: String,
        data: serde_json::Value,
        degradation: Degradation,
    ) -> Self {
        let degraded = !degradation.is_empty();
        let (notes, degraded_reasons) = degradation.into_parts();
        Self {
            markdown,
            data,
            notes,
            degraded_reasons,
            degraded,
        }
    }
}

/// JSON-serializable error classification per ADR-0065 (9-code policy).
///
/// `Internal` is reserved for scout-side invariant violations (e.g. unexpected
/// API schema during deserialize). `Timeout` splits from `TempFailure` so
/// callers can apply a longer retry backoff than for rate limits / 5xx.
/// `Unknown` is the explicit escape hatch for inputs that no priority rule
/// classified; a rising rate of `Unknown` signals the classification design
/// needs revisiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ErrorCode {
    UsageError,
    DataError,
    NotFound,
    Internal,
    IoError,
    TempFailure,
    Timeout,
    Unknown,
}

impl ErrorCode {
    /// sysexits.h exit code mapped 1:1 from `error.code`. Exit-code values are
    /// governed by ADR-0002 (scout-local). The `error.code` JSON tag itself is
    /// governed by ADR-0065 (dotclaude) until a scout-local ADR captures it.
    /// `Timeout` (124) follows GNU coreutils `timeout` and `Unknown` (104) is
    /// the PJ extension for unclassifiable failures.
    pub(crate) fn exit_code(self) -> u8 {
        match self {
            Self::UsageError => 64,  // EX_USAGE
            Self::DataError => 65,   // EX_DATAERR
            Self::NotFound => 66,    // EX_NOINPUT
            Self::Internal => 70,    // EX_SOFTWARE (scout-side invariant)
            Self::IoError => 74,     // EX_IOERR
            Self::TempFailure => 75, // EX_TEMPFAIL
            Self::Timeout => 124,    // GNU coreutils `timeout` convention
            Self::Unknown => 104,    // PJ extension, ADR-0065 §Classification Priority
        }
    }
}

/// Success envelope wrapping command output per ADR-0065. ADR-0003 added
/// `degraded_reasons` as an additive field (omitted from JSON when empty).
#[derive(Debug, Serialize)]
pub(crate) struct SuccessEnvelope {
    pub data: serde_json::Value,
    pub degraded: bool,
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub degraded_reasons: Vec<DegradedReason>,
}

/// Error envelope per ADR-0065. Wraps the payload under an `error` key so
/// JSON output matches `{"error": { "code": ..., "message": ..., ... }}`.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub error: ErrorPayload,
}

/// Error payload nested under `ErrorEnvelope::error` per ADR-0065.
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

    /// [T-EN004] ErrorEnvelope wraps payload under `error` key per ADR-0065
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
            "envelope should start with `{{\"error\":` per ADR-0065, got: {json}"
        );
        assert!(
            json.contains(r#""code":"USAGE_ERROR""#),
            "payload should contain code, got: {json}"
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
        let out = CommandOutput::ok(String::from("md"), serde_json::json!({"a": 1}));
        assert_eq!(out.markdown, "md");
        assert_eq!(out.data, serde_json::json!({"a": 1}));
        assert!(out.notes.is_empty());
        assert!(out.degraded_reasons.is_empty());
        assert!(!out.degraded);
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
        assert!(out.degraded);
        assert_eq!(out.notes, vec!["partial fetch"]);
        assert_eq!(
            out.degraded_reasons,
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
        assert!(!out.degraded);
        assert!(out.degraded_reasons.is_empty());
    }

    /// [T-EN010] ErrorCode serializes per ADR-0065 SCREAMING_SNAKE_CASE
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

    /// [T-EN014] ErrorCode::Internal maps to exit 70 (EX_SOFTWARE) per ADR-0065
    #[test]
    fn error_code_internal_exits_70() {
        assert_eq!(ErrorCode::Internal.exit_code(), 70);
    }

    /// [T-EN015] ErrorCode::Unknown maps to exit 104 (PJ extension) per ADR-0065
    #[test]
    fn error_code_unknown_exits_104() {
        assert_eq!(ErrorCode::Unknown.exit_code(), 104);
    }

    /// [T-EN016] ErrorCode::Timeout maps to exit 124 (GNU coreutils `timeout`)
    /// per ADR-0065. Independent from TempFailure(75) so callers can apply a
    /// longer retry backoff than for rate limits / 5xx.
    #[test]
    fn error_code_timeout_exits_124() {
        assert_eq!(ErrorCode::Timeout.exit_code(), 124);
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
}
