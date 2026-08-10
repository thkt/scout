//! Shared error classification used by every backend's `classify()` method.

use crate::envelope::ErrorCode;
use crate::retry::is_transient_network;

// Named so every backend reporting the same class of failure hands the caller
// the same next_step string.
const HINT_RETRY_DELAY: &str = "Retry after a short delay";
const HINT_CHECK_NETWORK: &str = "Check your network connection";

/// Per-variant error classification produced by each backend error type's
/// `classify()` method. `From<XxxError> for ScoutError` composes it with the
/// variant's `Display` message into a `ScoutError`.
///
/// Centralising classification on the variant (instead of inside `From`) keeps
/// the ADR-0011 priority decision next to the variant definition and lets
/// `classify()` be unit-tested directly.
pub(crate) struct Classification {
    pub(crate) kind: ErrorCode,
    pub(crate) next_step: Option<String>,
}

impl Classification {
    pub(crate) fn new(kind: ErrorCode) -> Self {
        Self {
            kind,
            next_step: None,
        }
    }

    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.next_step = Some(hint.into());
        self
    }

    /// For 5xx, rate-limit, and other timing-recoverable failures.
    pub(crate) fn transient_retry() -> Self {
        Self::new(ErrorCode::TempFailure).with_hint(HINT_RETRY_DELAY)
    }

    /// For connect-level network failures where retry alone will not help.
    fn transient_network() -> Self {
        Self::new(ErrorCode::TempFailure).with_hint(HINT_CHECK_NETWORK)
    }

    /// Split from `TempFailure` per ADR-0002 so caller scripts can apply a
    /// longer backoff than for rate-limit / 5xx failures.
    pub(crate) fn timeout_retry() -> Self {
        Self::new(ErrorCode::Timeout).with_hint(HINT_RETRY_DELAY)
    }

    /// The ADR-0003 HTTP-status table, in one place.
    ///
    /// Three backends used to re-derive it from raw status integers, and they
    /// had drifted: a GitHub 408 answered DataError instead of TempFailure, a
    /// Brave 404 answered DataError instead of NotFound. A backend that needs a
    /// different code for a status — not just a different hint — adds its own arm
    /// ahead of the delegating one, which makes the deviation visible; ADR-0003
    /// requires such a reclassification to say so in a doc comment.
    ///
    /// Hints stay with the caller: the table decides the code, and only the
    /// backend knows what to tell the user about its own service.
    pub(crate) fn from_http_status(status: u16) -> Self {
        match status {
            500..=599 | 408 | 429 => Self::transient_retry(),
            404 => Self::new(ErrorCode::NotFound),
            401 | 403 => Self::new(ErrorCode::UsageError),
            400..=499 => Self::new(ErrorCode::DataError),
            // Retreat: 1xx/3xx reaching an error path is not a status this table
            // describes, so it becomes the signal rather than a guess.
            _ => Self::new(ErrorCode::Unknown),
        }
    }

    /// Where a `reqwest::Error` lands in the ADR-0011 priority table.
    ///
    /// One rule about one foreign type, so it lives once: every backend that
    /// wraps a `reqwest::Error` asks the same two questions in the same order.
    /// The order is load-bearing — `is_transient_network` also answers true for
    /// timeouts, and ADR-0002 splits those into their own code, so the timeout
    /// check has to come first.
    ///
    /// Anything neither timeout nor transient is `Unknown`, not `TempFailure`:
    /// a rising `Unknown` rate is the signal ADR-0011 wants when the
    /// classification misses a case, and calling an unrecognized transport
    /// failure retryable buries it instead.
    pub(crate) fn from_reqwest(e: &reqwest::Error) -> Self {
        // Priority 4: TIMEOUT
        if e.is_timeout() {
            return Self::timeout_retry();
        }
        // Priority 4: TEMP_FAILURE
        if is_transient_network(e) {
            return Self::transient_network();
        }
        // Retreat: unclassifiable transport failure
        Self::new(ErrorCode::Unknown)
    }
}
