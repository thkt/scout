//! Time abstraction for testability.
//!
//! Issue #103 / H-12: `secs_until_ratelimit_reset` previously called
//! `SystemTime::now()` directly, making it impossible to verify the
//! `x-ratelimit-reset - now` arithmetic without time-of-day flakiness.

use std::time::{SystemTime, UNIX_EPOCH};

/// Source of unix-epoch wall-clock seconds. `Send + Sync` so implementations
/// can live behind an `Arc<dyn Clock>` shared across async tasks.
pub(crate) trait Clock: Send + Sync {
    fn now_secs(&self) -> u64;
}

/// Production clock. Returns 0 if the system clock is set before 1970-01-01,
/// which is treated as "unknown now" — the only callers that read this value
/// (rate-limit reset arithmetic) saturate to 0 anyway.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Test clock that always returns the configured second.
#[cfg(test)]
pub(crate) struct FixedClock(pub u64);

#[cfg(test)]
impl Clock for FixedClock {
    fn now_secs(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-CLOCK001] FixedClock echoes its constructor argument so callers can
    /// pin `now` for deterministic retry-after arithmetic tests.
    #[test]
    fn fixed_clock_returns_constructor_value() {
        let c = FixedClock(1_700_000_000);
        assert_eq!(c.now_secs(), 1_700_000_000);
    }

    /// [T-CLOCK002] SystemClock returns a unix epoch second that is plausibly
    /// "now" (after 2020-01-01). Guards against an accidental `Duration::ZERO`
    /// regression in the unwrap_or branch.
    #[test]
    fn system_clock_returns_post_2020_epoch_seconds() {
        let c = SystemClock;
        let now = c.now_secs();
        // 2020-01-01T00:00:00Z = 1_577_836_800 unix seconds.
        assert!(
            now > 1_577_836_800,
            "system clock should be after 2020-01-01, got {now}"
        );
    }
}
