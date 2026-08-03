use std::error::Error;
use std::fmt;

use tracing::warn;

use crate::brave::client::BraveError;
use crate::envelope::{Degradation, DegradedReason, ErrorCode};
use crate::fetch::FetchError;
use crate::github;
use crate::slack::SlackError;

use crate::classify::Classification;

#[derive(Debug)]
pub struct ScoutError {
    message: String,
    retryable: bool,
    kind: ErrorCode,
    next_step: Option<String>,
    candidates: Vec<String>,
}

impl fmt::Display for ScoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.next_step {
            write!(f, " — {hint}")?;
        }
        if self.retryable {
            write!(f, " (temporary failure; retry may succeed)")?;
        }
        Ok(())
    }
}

impl Error for ScoutError {}

impl ScoutError {
    /// Shared construction path. `retryable` is derived from `kind` so the
    /// public exit-code/JSON contract cannot drift between callers.
    fn new(kind: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            retryable: kind.is_retryable(),
            kind,
            next_step: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn user_error(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::UsageError, msg)
    }

    /// External tool / IO failure outside scout's invariants (e.g. headless
    /// browser CDP error). Maps to `ErrorCode::IoError` (exit 74 EX_IOERR).
    /// scout-side schema bugs route through `Classification::new(Internal)`
    /// in each backend's `classify()` instead.
    pub(super) fn io_error(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::IoError, msg)
    }

    /// scout-side invariant violation (e.g. a `serde_json::to_value` failure on
    /// a type scout itself controls). Maps to `ErrorCode::Internal` (exit 70
    /// EX_SOFTWARE), the sibling of `io_error`/`unknown` named per ADR-0003 §104.
    /// Lets handlers propagate serialize failures via `?` through the JSON error
    /// envelope instead of `.expect()` panicking and bypassing it.
    pub(super) fn internal_bug(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, msg)
    }

    /// Timeout (request-level or transport-level). Maps to `ErrorCode::Timeout`
    /// (exit 124, GNU coreutils `timeout`) per ADR-0002. Retryable like
    /// `transient`, but separated so caller scripts/agents can apply a longer
    /// backoff than for rate-limit / 5xx temp failures.
    pub(super) fn timeout(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, msg)
    }

    #[cfg(test)]
    pub(super) fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, msg)
    }

    /// Build a `ScoutError` from a backend-emitted [`Classification`] paired
    /// with the variant's `Display` message. Used by `From<XxxError>` impls
    /// so the classification logic stays co-located with each error variant.
    fn from_classification(c: Classification, msg: impl Into<String>) -> Self {
        let mut err = Self::new(c.kind, msg);
        err.next_step = c.next_step;
        err
    }

    /// Test-only builder for fixtures that construct `ScoutError` directly
    /// without going through `From<XxxError>`; production paths attach
    /// `next_step` via [`Classification`].
    #[cfg(test)]
    pub(super) fn with_next_step(mut self, hint: impl Into<String>) -> Self {
        self.next_step = Some(hint.into());
        self
    }

    /// Attach correction candidates per ADR-0002 `error.candidates` (e.g., typo suggestions).
    pub(super) fn with_candidates(mut self, candidates: Vec<String>) -> Self {
        self.candidates = candidates;
        self
    }

    /// sysexits.h exit code derived from `kind` per ADR-0002.
    pub fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    /// JSON-serializable error classification per ADR-0002.
    pub fn error_kind(&self) -> ErrorCode {
        self.kind
    }

    /// Plain message without next_step / retry hints. Use for JSON `error.message`
    /// where `error.next_step` and `error.retryable` are surfaced separately.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Recovery hint per ADR-0002 `error.next_step`.
    pub fn next_step(&self) -> Option<&str> {
        self.next_step.as_deref()
    }

    /// Correction candidates per ADR-0002 `error.candidates` (e.g., similar paths after typo).
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }
}

// `From<XxxError>` impls delegate to each backend's `classify()` so the
// ADR-0011 priority decision stays exhaustiveness-checked next to the variant.
impl From<github::GitHubError> for ScoutError {
    fn from(e: github::GitHubError) -> Self {
        let msg = e.to_string();
        Self::from_classification(e.classify(), msg)
    }
}

impl From<FetchError> for ScoutError {
    fn from(e: FetchError) -> Self {
        let msg = e.to_string();
        Self::from_classification(e.classify(), msg)
    }
}

impl From<SlackError> for ScoutError {
    fn from(e: SlackError) -> Self {
        let msg = e.to_string();
        Self::from_classification(e.classify(), msg)
    }
}

impl From<BraveError> for ScoutError {
    fn from(e: BraveError) -> Self {
        let msg = e.to_string();
        Self::from_classification(e.classify(), msg)
    }
}

/// Unwrap a `Result<Vec<T>, GitHubError>` returning the value on success, or
/// push a degradation entry (paired `notes` message + typed `reason`) on
/// failure and return an empty vec. Per ADR-0003, callers supply only the
/// typed `reason`; the human-readable label is derived from the variant via
/// [`DegradedReason::label`] so the `(label, reason)` pair stays in sync.
pub(super) fn unwrap_or_degraded<T>(
    result: Result<Vec<T>, github::GitHubError>,
    reason: DegradedReason,
    degradation: &mut Degradation,
) -> Vec<T> {
    match result {
        Ok(v) => v,
        Err(e) => {
            let label = reason.label();
            warn!(%e, "failed to fetch {}", label);
            degradation.push(format!("Could not fetch {label} ({e})"), reason);
            vec![]
        }
    }
}

#[cfg(test)]
mod classification_tests;
#[cfg(test)]
mod exit_code_tests;
