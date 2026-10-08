//! Output envelopes governed by ADR-0010 and typed degradation reasons by ADR-0003.

use serde::Serialize;

/// Typed JSON degradation reasons; callers need not parse free-form notes.
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
    SlackThreadTruncated,
    SlackUsersCapped,
    SlackOutputTruncated,
    SlackLookupFailed,
    DecodeUncertain,
}

impl DegradedReason {
    /// Labels for [`crate::tools::errors::unwrap_or_degraded`]. Only the three
    /// GitHub list-fetch failures use this helper; other reasons build notes at
    /// call sites. The generic arm keeps the match exhaustive.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::IssuesFetchFailed => "issues",
            Self::PullsFetchFailed => "pull requests",
            Self::ReleasesFetchFailed => "releases",
            Self::BraveSearchFailed
            | Self::ReadmeFetchFailed
            | Self::ReadmeBlobFetchFailed
            | Self::ReadmeDecodeFailed
            | Self::UrlFetchFailed
            | Self::ReadabilityFallback
            | Self::SlackThreadTruncated
            | Self::SlackUsersCapped
            | Self::SlackOutputTruncated
            | Self::SlackLookupFailed
            | Self::DecodeUncertain => "resource",
        }
    }
}

/// Private fields and [`Degradation::push`] keep notes paired with reasons.
#[derive(Debug, Default)]
pub(crate) struct Degradation {
    notes: Vec<String>,
    reasons: Vec<DegradedReason>,
}

impl Degradation {
    pub(crate) fn push(&mut self, message: String, reason: DegradedReason) {
        self.notes.push(message);
        self.reasons.push(reason);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.reasons.is_empty()
    }

    pub(crate) fn notes(&self) -> &[String] {
        &self.notes
    }

    pub(crate) fn into_parts(self) -> (Vec<String>, Vec<DegradedReason>) {
        (self.notes, self.reasons)
    }
}

/// Handler output for Markdown or JSON. Private fields keep the degraded flag
/// consistent with notes and reasons through [`Self::with_degradation`].
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub(crate) struct CommandOutput {
    markdown: String,
    data: serde_json::Value,
    notes: Vec<String>,
    degraded_reasons: Vec<DegradedReason>,
    degraded: bool,
}

impl CommandOutput {
    pub(crate) fn ok(markdown: String, data: serde_json::Value) -> Self {
        Self {
            markdown,
            data,
            notes: Vec::new(),
            degraded_reasons: Vec::new(),
            degraded: false,
        }
    }

    pub(crate) fn with_degradation(
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

    pub(crate) fn into_markdown(self) -> String {
        self.markdown
    }

    pub(crate) fn into_envelope(self) -> SuccessEnvelope {
        SuccessEnvelope {
            data: self.data,
            degraded: self.degraded,
            notes: self.notes,
            degraded_reasons: self.degraded_reasons,
        }
    }
}

/// Borrowing accessors for tests; production consumes the output.
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

/// JSON error classification (ADR-0010). `Internal` denotes scout invariants;
/// `Unknown` denotes unclassified failures (ADR-0011). `Timeout` is separate
/// from temporary failures so callers can choose a longer backoff.
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
    InterruptedSigint,
    InterruptedSigterm,
}

impl ErrorCode {
    /// Exit codes governed by ADR-0002 and ADR-0017; JSON tags by ADR-0010.
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
            // Kept separate from the platform-gated InterruptSignal variants:
            // JSON codes must be platform-independent. T-W009 checks agreement.
            Self::InterruptedSigint => 130,
            Self::InterruptedSigterm => 143,
        }
    }

    /// Derive retryability from the code to keep ScoutError and JSON in sync.
    pub(crate) fn is_retryable(self) -> bool {
        matches!(self, Self::TempFailure | Self::Timeout)
    }
}

/// Success envelope wrapping command output per ADR-0010. `degraded_reasons`
/// (ADR-0003) is additive, so it is omitted from JSON when empty.
#[derive(Debug, Serialize)]
pub(crate) struct SuccessEnvelope {
    data: serde_json::Value,
    degraded: bool,
    notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    degraded_reasons: Vec<DegradedReason>,
}

/// Error envelope per ADR-0010.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub(crate) error: ErrorPayload,
}

/// Envelope serialization is infallible for these crate-owned payloads;
/// an error would be an invariant violation.
pub(crate) fn to_json_line<T: Serialize>(envelope: &T) -> String {
    serde_json::to_string(envelope).expect("envelope is Serialize")
}

/// Error payload nested under `ErrorEnvelope::error` per ADR-0010.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorPayload {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_step: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) candidates: Vec<String>,
    pub(crate) retryable: bool,
}

#[cfg(test)]
mod tests;
