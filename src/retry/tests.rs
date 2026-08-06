use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::time::Instant;
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

use super::*;
use crate::clock::FixedClock;
use crate::rng::{FastrandRng, SeededRng};
use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};

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

/// [T-R001] FR-001 + FR-002: first call fails with RateLimited{retry_after: Some(1)},
/// second call succeeds. Helper must retry exactly once, sleep ~1s
/// (server-supplied, no jitter on the RateLimited path), and return Ok.
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
        2,
        mock_is_retriable,
        mock_extract_retry_after,
        &FastrandRng,
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

/// [T-R002] FR-002: non-RateLimited transient error always returns None from the
/// extractor. Helper exhausts 1 + max_retries (=3 attempts when
/// max_retries=2, 2 sleeps) using jittered exponential backoff. Total
/// elapsed must fall within the backoff envelope: half + jitter for
/// attempt 0 + attempt 1.
#[tokio::test(start_paused = true)]
async fn applies_jittered_backoff_when_extractor_returns_none() {
    let attempts = AtomicUsize::new(0);
    let start = Instant::now();

    let result: Result<(), MockErr> = retry_with_rate_limit(
        || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(MockErr::Other)
        },
        2,
        mock_is_retriable,
        mock_extract_retry_after,
        &FastrandRng,
    )
    .await;

    let elapsed = start.elapsed();

    assert_eq!(result, Err(MockErr::Other));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        3usize,
        "expected 1 + max_retries (=3) invocations when every attempt fails"
    );
    // jittered_backoff(0) ∈ [500ms, 1000ms), jittered_backoff(1) ∈ [1000ms, 2000ms).
    // Sum of the two sleeps therefore ∈ [1500ms, 3000ms). Upper bound
    // padded to absorb scheduler slack under start_paused.
    assert!(
        elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_millis(3200),
        "expected jittered backoff total in [1500ms, 3200ms], got {elapsed:?}"
    );
}

/// [T-R003] FR-002 + cap validation rule: RateLimited{retry_after: Some(500)} is
/// above MAX_RETRY_AFTER_SECS (300). is_retriable returns false, the
/// helper must propagate the error after exactly one invocation.
#[tokio::test(start_paused = true)]
async fn does_not_retry_when_retry_after_exceeds_cap() {
    let attempts = AtomicUsize::new(0);

    let result: Result<(), MockErr> = retry_with_rate_limit(
        || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(MockErr::RateLimited(Some(500)))
        },
        2,
        mock_is_retriable,
        mock_extract_retry_after,
        &FastrandRng,
    )
    .await;

    assert_eq!(result, Err(MockErr::RateLimited(Some(500))));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "expected no retry when retry_after exceeds MAX_RETRY_AFTER_SECS"
    );
}

/// [T-R006] Issue #120: `SCOUT_MAX_RETRIES=0` must disable retries entirely.
/// The contract is "N retries on top of the original attempt", so 0
/// means a single attempt with no sleep — the user-visible inverse of
/// the default (=2 → 3 attempts).
#[tokio::test(start_paused = true)]
async fn max_retries_zero_runs_once_without_retry() {
    let attempts = AtomicUsize::new(0);
    let start = Instant::now();

    let result: Result<(), MockErr> = retry_with_rate_limit(
        || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(MockErr::Other)
        },
        0,
        mock_is_retriable,
        mock_extract_retry_after,
        &FastrandRng,
    )
    .await;

    assert_eq!(result, Err(MockErr::Other));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "max_retries=0 must run exactly 1 attempt"
    );
    assert!(
        start.elapsed() < Duration::from_millis(50),
        "no backoff sleep should occur when max_retries=0"
    );
}

/// [T-R007] Same seed → identical sample, proving the Rng seam threads through to
/// jittered_backoff. The envelope check guards the half + jitter formula
/// (attempt=0 → half=500, jitter ∈ [0,500), result ∈ [500,1000)).
#[test]
fn jittered_backoff_is_deterministic_with_seeded_rng() {
    let first = jittered_backoff(0, &SeededRng::new(42));
    let second = jittered_backoff(0, &SeededRng::new(42));
    assert_eq!(first, second, "same seed must reproduce identical backoff");
    assert!(
        (500..1000).contains(&first),
        "jittered_backoff(0) should fall in [500, 1000), got {first}"
    );
}

/// [T-R014] Issue #185: the `None` arm of `retry_after_or_backoff` (exponential
/// backoff) had no ceiling, so a high `SCOUT_MAX_RETRIES` could produce a
/// single multi-minute sleep (attempt 9 → up to ~512s) that overruns the
/// surrounding tool timeout. The cap must match the `Some` arm's
/// `MAX_RETRY_AFTER_SECS`. attempt=10 has `half = 512s >= 300s`, so the
/// floor of the jitter envelope already exceeds the cap — the result is
/// exactly the cap regardless of the rng sample.
#[test]
fn backoff_is_capped_at_max_retry_after() {
    let cap = Duration::from_secs(MAX_RETRY_AFTER_SECS);
    // Invariant across (and beyond) the attempt range the helper can reach
    // (`SCOUT_MAX_RETRIES` is capped at 10): the uncapped exponential growth
    // never escapes the ceiling.
    for attempt in 0..=12u32 {
        let delay = retry_after_or_backoff(None, attempt, &FastrandRng);
        assert!(
            delay <= cap,
            "backoff for attempt {attempt} ({delay:?}) must not exceed {cap:?}"
        );
    }
    // Rng-independent equality: at attempt 10 half (512s) already exceeds the cap.
    assert_eq!(
        retry_after_or_backoff(None, 10, &FastrandRng),
        cap,
        "backoff past the cap boundary must clamp exactly to the cap"
    );
}

/// [T-R004] Issue #113: reqwest 0.13 surfaces a mid-stream body drop as
/// `is_decode() == true` with an `io::Error` (UnexpectedEof) in the
/// source chain. is_transient_network must classify this as transient
/// so the retry loop attempts recovery; left untreated it falls into
/// GitHubError::Decode → Internal(70), retryable=false.
#[tokio::test]
async fn is_transient_network_recognizes_mid_stream_body_drop() {
    let Some((url, _counter, handle)) = spawn_mid_stream_drop_server(1) else {
        return; // loopback bind unavailable — skip
    };
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.expect("send");
    let result: Result<serde_json::Value, _> = resp.json().await;
    let err = result.expect_err("mid-stream drop must fail body decode");

    assert!(
        is_transient_network(&err),
        "mid-stream drop must classify as transient (is_body={}, is_decode={}, is_connect={}, is_timeout={}): {err}",
        err.is_body(),
        err.is_decode(),
        err.is_connect(),
        err.is_timeout()
    );

    handle
        .join()
        .expect("server thread should not panic")
        .expect("server thread should not fail while writing the response");
}

/// [T-R005] Counterpart to T-R004. A 2xx with malformed JSON also returns
/// `is_decode() == true` but the source chain is a serde_json::Error,
/// not an io::Error. is_transient_network must keep returning false
/// so the error stays on the Decode → Internal(70) non-retry path.
#[tokio::test]
async fn is_transient_network_rejects_schema_fail() {
    let Some(server) = try_spawn_mock_server("retry::is_transient_network_schema").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{not valid json"))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let resp = client.get(server.uri()).send().await.expect("send");
    let result: Result<serde_json::Value, _> = resp.json().await;
    let err = result.expect_err("malformed JSON must fail body decode");

    assert!(
        !is_transient_network(&err),
        "schema fail must not classify as transient (is_decode={}): {err}",
        err.is_decode()
    );
}

fn headers_with_retry_after(value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(RETRY_AFTER, value.parse().expect("static literal is valid"));
    h
}

/// [T-R008] RFC 9110 §10.2.4 form 1: delay-seconds. Clock is irrelevant here;
/// FixedClock(0) proves no clock arithmetic sneaks into the integer branch.
#[test]
fn parse_retry_after_accepts_integer_seconds() {
    let headers = headers_with_retry_after("120");
    assert_eq!(parse_retry_after(&headers, &FixedClock(0)), Some(120));
}

/// [T-R009] RFC 9110 §10.2.4 form 2: HTTP-date. "Wed, 21 Oct 2015 07:28:00 GMT" =
/// 1_445_412_480 unix seconds. FixedClock(1_445_412_180) is 300s earlier,
/// so the returned delay must be 300 (target_secs - clock.now_secs()).
#[test]
fn parse_retry_after_accepts_http_date() {
    let headers = headers_with_retry_after("Wed, 21 Oct 2015 07:28:00 GMT");
    assert_eq!(
        parse_retry_after(&headers, &FixedClock(1_445_412_180)),
        Some(300)
    );
}

/// [T-R010] Returns Some(0) — caller retries now — rather than None, which
/// would fall back to jittered backoff (T-R011, T-R012).
#[test]
fn parse_retry_after_clamps_past_http_date_to_zero() {
    let headers = headers_with_retry_after("Wed, 21 Oct 2015 07:28:00 GMT");
    assert_eq!(
        parse_retry_after(&headers, &FixedClock(2_000_000_000)),
        Some(0)
    );
}

/// [T-R011] Neither integer nor RFC-822/850/asctime date: drop and let caller fall
/// back to jittered backoff.
#[test]
fn parse_retry_after_returns_none_for_garbage() {
    let headers = headers_with_retry_after("definitely not a date");
    assert_eq!(parse_retry_after(&headers, &FixedClock(0)), None);
}

/// [T-R012] Same None as an unparseable header, not the Some(0) a past date yields.
#[test]
fn parse_retry_after_returns_none_when_header_absent() {
    let headers = HeaderMap::new();
    assert_eq!(parse_retry_after(&headers, &FixedClock(0)), None);
}
