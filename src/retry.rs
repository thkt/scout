use std::future::Future;
use std::time::Duration;

use reqwest::Error;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use tokio::time::sleep;
use tracing::debug;

pub(crate) const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_RETRY_AFTER_SECS: u64 = 300;

pub(crate) fn jittered_backoff(attempt: u32) -> u64 {
    let base = INITIAL_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt));
    let half = base / 2;
    half + fastrand::u64(..half.max(1))
}

pub(crate) fn is_transient_network(e: &Error) -> bool {
    e.is_connect() || e.is_timeout()
}

pub(crate) async fn retry_with<T, E, F, Fut>(
    operation: F,
    is_retriable: impl Fn(&E) -> bool,
    delay_for: impl Fn(&E, u32) -> Duration,
    fallback_err: impl FnOnce() -> E,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match operation().await {
            Ok(v) => return Ok(v),
            Err(e) if is_retriable(&e) => {
                if attempt + 1 < MAX_RETRIES {
                    let delay = delay_for(&e, attempt);
                    debug!(
                        attempt = attempt + 1,
                        delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "retrying after transient error"
                    );
                    sleep(delay).await;
                }
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(fallback_err))
}

pub(crate) fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
}

/// Returns true when it makes sense to retry: the server-supplied delay fits
/// within the cap, so we will actually sleep the full duration and succeed.
/// When retry_after exceeds the cap the delay would be truncated, meaning
/// we would sleep, retry, and still hit the limit — fail fast instead.
pub(crate) fn retry_after_within_cap(retry_after: Option<u64>) -> bool {
    retry_after.is_none_or(|s| s <= MAX_RETRY_AFTER_SECS)
}

pub(crate) fn retry_after_or_backoff(retry_after: Option<u64>, attempt: u32) -> Duration {
    match retry_after {
        Some(secs) => Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS)),
        None => Duration::from_millis(jittered_backoff(attempt)),
    }
}

/// Retry with the standard rate-limit-aware delay formula.
///
/// Caller supplies `extract_retry_after` to pull `Option<u64>` from the
/// error's `RateLimited` variant. The delay per attempt is then
/// `retry_after_or_backoff(extract_retry_after(&e), attempt)`.
///
/// The cap policy (refuse retry when retry-after exceeds
/// `MAX_RETRY_AFTER_SECS`) is the caller's `is_retriable` responsibility,
/// typically via `retry_after_within_cap`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn retry_with_rate_limit<T, E, F, Fut>(
    operation: F,
    is_retriable: impl Fn(&E) -> bool,
    extract_retry_after: impl Fn(&E) -> Option<u64>,
    fallback_err: impl FnOnce() -> E,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    retry_with(
        operation,
        is_retriable,
        |e, attempt| retry_after_or_backoff(extract_retry_after(e), attempt),
        fallback_err,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::time::Instant;

    use super::*;

    /// Test-only error type. `RateLimited` carries the server-supplied
    /// `Retry-After` (seconds); `Other` represents a non-rate-limit transient
    /// error whose extractor yields `None` so the helper falls back to
    /// `jittered_backoff`.
    #[derive(Debug, thiserror::Error, PartialEq, Eq)]
    enum MockErr {
        #[error("rate limited (retry_after={0:?})")]
        RateLimited(Option<u64>),
        #[error("other transient error")]
        Other,
    }

    /// Mirrors a real backend's `is_retriable`: rate-limited responses are
    /// retriable only when the server-supplied delay fits within
    /// `MAX_RETRY_AFTER_SECS`; other transient errors are always retriable.
    fn mock_is_retriable(e: &MockErr) -> bool {
        match e {
            MockErr::RateLimited(retry_after) => retry_after_within_cap(*retry_after),
            MockErr::Other => true,
        }
    }

    /// Mirrors a real backend's `extract_retry_after` closure: only
    /// `RateLimited` carries a delay hint; everything else returns `None`
    /// so the helper applies jittered exponential backoff.
    fn mock_extract_retry_after(e: &MockErr) -> Option<u64> {
        match e {
            MockErr::RateLimited(retry_after) => *retry_after,
            MockErr::Other => None,
        }
    }

    // T-R001: retries_once_then_succeeds_with_retry_after_delay
    // FR-001 + FR-002: first call fails with RateLimited{retry_after: Some(1)},
    // second call succeeds. Helper must retry exactly once, sleep ~1s
    // (server-supplied, no jitter on the RateLimited path), and return Ok.
    #[tokio::test(start_paused = true)]
    async fn retries_once_then_succeeds_with_retry_after_delay() {
        let attempts = AtomicUsize::new(0);
        let start = Instant::now();

        let result: Result<&'static str, MockErr> = retry_with_rate_limit(
            || async {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(MockErr::RateLimited(Some(1)))
                } else {
                    Ok("ok")
                }
            },
            mock_is_retriable,
            mock_extract_retry_after,
            || MockErr::Other,
        )
        .await;

        let elapsed = start.elapsed();

        assert_eq!(result, Ok("ok"));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "expected 2 invocations (1 failure + 1 success), got {}",
            attempts.load(Ordering::SeqCst)
        );
        // RateLimited path uses retry_after directly (no jitter); allow
        // generous tolerance for scheduler slack under start_paused.
        assert!(
            elapsed >= Duration::from_millis(800) && elapsed <= Duration::from_millis(1500),
            "expected ~1s delay (RateLimited retry_after=1), got {elapsed:?}"
        );
    }

    // T-R002: applies_jittered_backoff_when_extractor_returns_none
    // FR-002: non-RateLimited transient error always returns None from the
    // extractor. Helper exhausts MAX_RETRIES (=3 attempts, 2 sleeps) using
    // jittered exponential backoff. Total elapsed must fall within the
    // backoff envelope: half + jitter for attempt 0 + attempt 1.
    #[tokio::test(start_paused = true)]
    async fn applies_jittered_backoff_when_extractor_returns_none() {
        let attempts = AtomicUsize::new(0);
        let start = Instant::now();

        let result: Result<(), MockErr> = retry_with_rate_limit(
            || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(MockErr::Other)
            },
            mock_is_retriable,
            mock_extract_retry_after,
            || MockErr::Other,
        )
        .await;

        let elapsed = start.elapsed();

        assert_eq!(result, Err(MockErr::Other));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            usize::try_from(MAX_RETRIES).expect("MAX_RETRIES fits usize"),
            "expected MAX_RETRIES invocations when every attempt fails"
        );
        // jittered_backoff(0) ∈ [500ms, 1000ms), jittered_backoff(1) ∈ [1000ms, 2000ms).
        // Sum of the two sleeps therefore ∈ [1500ms, 3000ms). Upper bound
        // padded to absorb scheduler slack under start_paused.
        assert!(
            elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_millis(3200),
            "expected jittered backoff total in [1500ms, 3200ms], got {elapsed:?}"
        );
    }

    // T-R003: does_not_retry_when_retry_after_exceeds_cap
    // FR-002 + cap validation rule: RateLimited{retry_after: Some(500)} is
    // above MAX_RETRY_AFTER_SECS (300). is_retriable returns false, the
    // helper must propagate the error after exactly one invocation.
    #[tokio::test(start_paused = true)]
    async fn does_not_retry_when_retry_after_exceeds_cap() {
        let attempts = AtomicUsize::new(0);

        let result: Result<(), MockErr> = retry_with_rate_limit(
            || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(MockErr::RateLimited(Some(500)))
            },
            mock_is_retriable,
            mock_extract_retry_after,
            || MockErr::Other,
        )
        .await;

        assert_eq!(result, Err(MockErr::RateLimited(Some(500))));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "expected no retry when retry_after exceeds MAX_RETRY_AFTER_SECS"
        );
    }
}
