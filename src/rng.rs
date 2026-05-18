//! Random-number abstraction. `FastrandRng` for production, `SeededRng` for
//! tests that need deterministic backoff arithmetic.

/// Bounded random `u64` source. `Send + Sync` so implementations can sit
/// behind an `Arc<dyn Rng>` shared across async tasks.
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

/// Test RNG seeded with a fixed value so every backoff calculation is
/// reproducible across runs.
#[cfg(test)]
pub(crate) struct SeededRng(pub fastrand::Rng);

#[cfg(test)]
impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self(fastrand::Rng::with_seed(seed))
    }
}

#[cfg(test)]
impl Rng for SeededRng {
    fn u64_below(&self, upper_exclusive: u64) -> u64 {
        self.0.clone().u64(..upper_exclusive)
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

    /// [T-RNG002] Two SeededRng with the same seed produce the same sequence,
    /// proving the deterministic test seam.
    #[test]
    fn seeded_rng_is_reproducible_under_identical_seed() {
        let a = SeededRng::new(42);
        let b = SeededRng::new(42);
        for _ in 0..5 {
            assert_eq!(a.u64_below(1_000_000), b.u64_below(1_000_000));
        }
    }
}
