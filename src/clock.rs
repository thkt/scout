//! Wall-clock abstraction. `SystemClock` for production, `FixedClock` for tests
//! that need deterministic `now`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Source of unix-epoch wall-clock seconds. `Send + Sync` so implementations
/// can live behind an `Arc<dyn Clock>` shared across async tasks.
pub(crate) trait Clock: Send + Sync {
    fn now_secs(&self) -> u64;
}

/// Returns 0 when the wall clock is pre-epoch (treat as unknown).
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

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
