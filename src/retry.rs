use std::time::Duration;

use tracing::debug;

pub(crate) const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

pub(crate) fn jittered_backoff(attempt: u32) -> u64 {
    let base = INITIAL_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt));
    let half = base / 2;
    half + fastrand::u64(..half.max(1))
}

pub(crate) fn is_transient_network(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout()
}

pub(crate) async fn retry_with<T, E, F, Fut>(
    operation: F,
    is_retriable: impl Fn(&E) -> bool,
    fallback_err: impl FnOnce() -> E,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match operation().await {
            Ok(v) => return Ok(v),
            Err(e) if is_retriable(&e) => {
                last_err = Some(e);
                if attempt + 1 < MAX_RETRIES {
                    let delay_ms = jittered_backoff(attempt);
                    debug!(attempt = attempt + 1, delay_ms, "retrying after transient error");
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(fallback_err))
}
