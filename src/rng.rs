//! Random-number abstraction. `FastrandRng` for production, `SeededRng` for
//! tests that need deterministic backoff arithmetic.

#[cfg(test)]
use std::sync::Mutex;

/// Bounded random `u64` source. `Send + Sync` so `&dyn Rng` can cross
/// `.await` points in the retry loop.
pub(crate) trait Rng: Send + Sync {
    /// Returns a uniform sample in `[0, upper_exclusive)`. Callers ensure
    /// `upper_exclusive > 0`; passing 0 will panic to match `fastrand`.
    fn u64_below(&self, upper_exclusive: u64) -> u64;
}

/// Production RNG backed by the process-global `fastrand` state.
pub(crate) struct FastrandRng;

impl Rng for FastrandRng {
    fn u64_below(&self, upper_exclusive: u64) -> u64 {
        fastrand::u64(..upper_exclusive)
    }
}

/// Test RNG seeded with a fixed value. `Mutex` is required because `Rng` is
/// `&self` but `fastrand::Rng::u64` mutates internal state; the bare value
/// would have to be `.clone()`d each call, which discards sequence progress
/// and produces the same sample every time.
#[cfg(test)]
pub(crate) struct SeededRng(Mutex<fastrand::Rng>);

#[cfg(test)]
impl SeededRng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(Mutex::new(fastrand::Rng::with_seed(seed)))
    }
}

#[cfg(test)]
impl Rng for SeededRng {
    fn u64_below(&self, upper_exclusive: u64) -> u64 {
        self.0
            .lock()
            .expect("SeededRng mutex poisoned")
            .u64(..upper_exclusive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-RNG001] FastrandRng draws within the requested half-open range.
    /// 100 samples is enough to catch an off-by-one in the upper bound.
    #[test]
    fn fastrand_rng_stays_below_upper_bound() {
        let rng = FastrandRng;
        for _ in 0..100 {
            let v = rng.u64_below(10);
            assert!(v < 10, "fastrand returned {v} for upper_exclusive=10");
        }
    }

    /// [T-RNG003] Repeated calls on the same SeededRng advance the internal
    /// state — guards against the earlier `self.0.clone()` bug where every
    /// call returned the seed's first sample.
    #[test]
    fn seeded_rng_advances_state_across_calls() {
        use std::collections::HashSet;
        let rng = SeededRng::new(7);
        let mut samples = Vec::with_capacity(5);
        for _ in 0..5 {
            samples.push(rng.u64_below(u64::MAX));
        }
        let unique: HashSet<_> = samples.iter().collect();
        assert!(
            unique.len() > 1,
            "SeededRng must advance state; got constant sequence: {samples:?}"
        );
    }
}
