use std::error::Error as _;
use std::fmt;
use std::future::Future;
use std::io;
use std::time::{Duration, UNIX_EPOCH};

use reqwest::Error;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use tokio::time::sleep;
use tracing::warn;

use crate::clock::Clock;
use crate::rng::Rng;

const INITIAL_BACKOFF_MS: u64 = 1000;
/// 5-min cap matches interactive CLI patience (a user waiting at a terminal
/// stops trusting the tool past this point). Server hints beyond the cap
/// fail fast rather than block.
const MAX_RETRY_AFTER_SECS: u64 = 300;

/// Default retry count used by every backend client when no override is
/// supplied. The helper performs `1 + DEFAULT_MAX_RETRIES` total attempts,
/// so `2` yields the 3-attempt budget that backends are tuned against.
pub(crate) const DEFAULT_MAX_RETRIES: u32 = 2;

/// Temporary re-export: `read_body_capped` and `MAX_API_RESPONSE_BYTES` moved
/// to `body_limit.rs` (a shared leaf in `charset.rs`'s `//!`-doc format); this
/// keeps existing `crate::retry::` call sites (Brave/Slack clients, tests)
/// resolving without an update.
pub(crate) use crate::body_limit::{MAX_API_RESPONSE_BYTES, read_body_capped};

/// Upper bound on JSON response body bytes accepted from the GitHub backend
/// (issue #186). GitHub's payloads are an order of magnitude larger than
/// Brave/Slack: `git/trees?recursive=1` is served up to GitHub's own ~7 MB
/// truncation ceiling, and `git/blobs` returns base64-inflated file content.
/// 10 MB matches `fetch.rs`'s `MAX_RESPONSE_BYTES` (the largest content scout
/// already returns) so legitimate large-repo trees and files are not rejected,
/// while still bounding the memory a hostile or runaway response can consume.
pub(crate) const MAX_GITHUB_RESPONSE_BYTES: usize = 10_000_000;

pub(crate) fn jittered_backoff(attempt: u32, rng: &dyn Rng) -> u64 {
    let base = INITIAL_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt));
    let half = base / 2;
    half + rng.u64_below(half.max(1))
}

pub(crate) fn is_transient_network(e: &Error) -> bool {
    e.is_connect() || e.is_timeout() || is_transient_decode(e)
}

/// True when a `Decode`-classified reqwest error originates in a transport
/// IO failure (mid-stream body drop, connection reset). Issue #113: reqwest
/// 0.13 surfaces an `UnexpectedEof` from hyper as `is_decode() == true`,
/// indistinguishable from a serde schema mismatch by boolean alone. Walking
/// the source chain for any `io::Error` separates transport (retryable) from
/// schema (terminal).
fn is_transient_decode(e: &Error) -> bool {
    if !e.is_decode() {
        return false;
    }
    let mut src = e.source();
    while let Some(cur) = src {
        if cur.downcast_ref::<io::Error>().is_some() {
            return true;
        }
        src = cur.source();
    }
    false
}

/// Run `operation` up to `max_retries + 1` times: one initial attempt
/// plus `max_retries` retries on retriable failures. Names follow the
/// user-facing contract — `SCOUT_MAX_RETRIES=N` means "N retries on top
/// of the original attempt", so `=0` runs once (no retry), `=2` runs at
/// most three times.
async fn retry_with<T, E, F, Fut>(
    operation: F,
    max_retries: u32,
    is_retriable: impl Fn(&E) -> bool,
    delay_for: impl Fn(&E, u32) -> Duration,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    // The last attempt returns its own error rather than stashing it, so there
    // is no path out of the loop that has to invent one.
    for attempt in 0..max_retries {
        match operation().await {
            Ok(v) => return Ok(v),
            Err(e) if is_retriable(&e) => {
                let delay = delay_for(&e, attempt);
                // warn (not debug) because retries are recoverable anomalies;
                // the error field surfaces the underlying failure without
                // requiring RUST_LOG=debug.
                warn!(
                    attempt = attempt + 1,
                    delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    error = %e,
                    "retrying after transient error"
                );
                sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
    operation().await
}

/// Parse `Retry-After`. RFC 9110 §10.2.4 allows two forms: an integer delay
/// in seconds, or an HTTP-date. The HTTP-date branch converts to "seconds
/// from now" via `clock` so retry scheduling stays in the same units.
pub(crate) fn parse_retry_after(headers: &HeaderMap, clock: &dyn Clock) -> Option<u64> {
    let raw = headers.get(RETRY_AFTER).and_then(|v| v.to_str().ok())?;
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    match httpdate::parse_http_date(raw) {
        Ok(target) => match target.duration_since(UNIX_EPOCH) {
            Ok(d) => Some(d.as_secs().saturating_sub(clock.now_secs())),
            Err(e) => {
                warn!(value = %raw, error = %e, "Retry-After HTTP-date is before Unix epoch");
                None
            }
        },
        Err(e) => {
            warn!(value = %raw, error = %e, "unparseable Retry-After header");
            None
        }
    }
}

/// Returns true when it makes sense to retry: the server-supplied delay fits
/// within the cap, so we will actually sleep the full duration and succeed.
/// When retry_after exceeds the cap the delay would be truncated, meaning
/// we would sleep, retry, and still hit the limit — fail fast instead.
pub(crate) fn retry_after_within_cap(retry_after: Option<u64>) -> bool {
    retry_after.is_none_or(|s| s <= MAX_RETRY_AFTER_SECS)
}

pub(crate) fn retry_after_or_backoff(
    retry_after: Option<u64>,
    attempt: u32,
    rng: &dyn Rng,
) -> Duration {
    match retry_after {
        Some(secs) => Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS)),
        // Cap the exponential backoff at the same ceiling as the server-supplied
        // `Retry-After`. Without this, a high `SCOUT_MAX_RETRIES` lets the
        // `2^attempt` growth produce a single multi-minute sleep (e.g. attempt 9
        // → 512s) that overruns the surrounding tool timeout (issue #185).
        None => Duration::from_millis(jittered_backoff(attempt, rng))
            .min(Duration::from_secs(MAX_RETRY_AFTER_SECS)),
    }
}

/// Retry with the rate-limit-aware delay formula. The cap policy (refuse
/// retry when retry-after exceeds `MAX_RETRY_AFTER_SECS`) is the caller's
/// `is_retriable` responsibility, typically via `retry_after_within_cap`.
pub(crate) async fn retry_with_rate_limit<T, E, F, Fut>(
    operation: F,
    max_retries: u32,
    is_retriable: impl Fn(&E) -> bool,
    extract_retry_after: impl Fn(&E) -> Option<u64>,
    rng: &dyn Rng,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    retry_with(operation, max_retries, is_retriable, |e, attempt| {
        retry_after_or_backoff(extract_retry_after(e), attempt, rng)
    })
    .await
}

#[cfg(test)]
mod tests;
