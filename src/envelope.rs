//! Output envelopes per ADR-0010 (scout-local JSON envelope contract) and
//! ADR-0003 (degraded_reasons typed enum).
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
    BraveSearchFailed,
}

impl DegradedReason {
    /// Human-readable label used by [`crate::tools::errors::unwrap_or_degraded`]
    /// to build the `"Could not fetch {label} ({e})"` message. The four
    /// fetch-style variants (three `*FetchFailed` plus `BraveSearchFailed`)
    /// that flow through that helper get a meaningful label; other variants
    /// build bespoke messages at their callsite.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::IssuesFetchFailed => "issues",
            Self::PullsFetchFailed => "pull requests",
            Self::ReleasesFetchFailed => "releases",
            Self::BraveSearchFailed => "Brave search",
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
///
/// Fields are private to enforce the `(degraded, notes, degraded_reasons)`
/// invariant: a literal `degraded: false` paired with non-empty `notes`
/// cannot be constructed. Use [`Self::ok`] or [`Self::with_degradation`].
#[derive(Debug)]
pub(crate) struct CommandOutput {
    markdown: String,
    data: serde_json::Value,
    notes: Vec<String>,
    degraded_reasons: Vec<DegradedReason>,
    degraded: bool,
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

    /// Consume self and return the rendered Markdown body.
    pub(crate) fn into_markdown(self) -> String {
        self.markdown
    }

    /// Consume self into a [`SuccessEnvelope`]. Moves `data`, `notes`, and
    /// `degraded_reasons` without cloning.
    pub(crate) fn into_envelope(self) -> SuccessEnvelope {
        SuccessEnvelope {
            data: self.data,
            degraded: self.degraded,
            notes: self.notes,
            degraded_reasons: self.degraded_reasons,
        }
    }
}

/// Test-only accessors. Production paths consume via [`CommandOutput::into_markdown`]
/// or [`CommandOutput::into_envelope`]; tests need to assert multiple fields
/// without consuming the value.
#[cfg(test)]
impl CommandOutput {
    pub(crate) fn markdown(&self) -> &str {
        &self.markdown
    }

    pub(crate) fn data(&self) -> &serde_json::Value {
        &self.data
    }

    pub(crate) fn degraded_reasons(&self) -> &[DegradedReason] {
        &self.degraded_reasons
    }
}

/// JSON-serializable error classification per ADR-0010 (9-code policy).
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
    /// governed by ADR-0010 (scout-local). `Timeout` (124) follows GNU coreutils
    /// `timeout` and `Unknown` (104) is the PJ extension for unclassifiable
    /// failures (per ADR-0011 Classification Priority Table 退避 slot).
    pub(crate) fn exit_code(self) -> u8 {
        match self {
            Self::UsageError => 64,  // EX_USAGE
            Self::DataError => 65,   // EX_DATAERR
            Self::NotFound => 66,    // EX_NOINPUT
            Self::Internal => 70,    // EX_SOFTWARE (scout-side invariant)
            Self::IoError => 74,     // EX_IOERR
            Self::TempFailure => 75, // EX_TEMPFAIL
            Self::Timeout => 124,    // GNU coreutils `timeout` convention
            Self::Unknown => 104,    // PJ extension per ADR-0002, retreat slot per ADR-0011
        }
    }

    /// Whether this classification recommends retry. Determined structurally
    /// from `kind` so `ScoutError` cannot drift out of sync with the JSON
    /// `error.retryable` contract.
    pub(crate) fn is_retryable(self) -> bool {
        matches!(self, Self::TempFailure | Self::Timeout)
    }
}

/// Success envelope wrapping command output per ADR-0010. ADR-0003 added
/// `degraded_reasons` as an additive field (omitted from JSON when empty).
#[derive(Debug, Serialize)]
pub(crate) struct SuccessEnvelope {
    pub data: serde_json::Value,
    pub degraded: bool,
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub degraded_reasons: Vec<DegradedReason>,
}

/// Error envelope per ADR-0010. Wraps the payload under an `error` key so
/// JSON output matches `{"error": { "code": ..., "message": ..., ... }}`.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub error: ErrorPayload,
}

/// Serialize an output envelope as its one-line JSON form per ADR-0010. The
/// single serialize point for both `SuccessEnvelope` and `ErrorEnvelope`: these
/// crate-owned types serialize infallibly, so the `expect` is unreachable and
/// callers stay free of a `Result` they could only `.expect()` themselves.
pub(crate) fn to_json_line<T: Serialize>(envelope: &T) -> String {
    serde_json::to_string(envelope).expect("envelope is Serialize")
}

/// Error payload nested under `ErrorEnvelope::error` per ADR-0010.
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
mod tests;
