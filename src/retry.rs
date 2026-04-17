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
