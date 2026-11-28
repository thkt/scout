//! Random-number abstraction. `FastrandRng` for production; tests implement
//! `Rng` with a fixed sample to pin the backoff arithmetic.

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
