//! Pins the ADR-0003 HTTP-status table end to end: an upstream status
//! reaching `scout fetch` through a mock `HTTP_PROXY` must travel through
//! `Classification::from_http_status` (src/tools/errors.rs) and
//! `ErrorCode::exit_code` (src/envelope.rs) to become the exact process exit
//! code and JSON `error.code` that table names.
//!
//! Every scenario also asserts the mock proxy's connection counter is >= 1,
//! so a test that happened to exit with the right code via a different path
//! (e.g. the SSRF pre-check or a DNS failure short-circuiting before the
//! proxy is ever dialed) cannot pass as a false positive for this contract.
//!
//! `T-C024`/`T-C025`/`T-C026` extend the same end-to-end shape to the three exit
//! codes an HTTP status can never produce: a proxy response slower than
//! `src/tools/config.rs`'s `SCOUT_FETCH_TIMEOUT_SECS` (124, `T-C024`), a proxy
//! response that never parses as HTTP at all (104, `T-C025`), and an
//! `HTTP_PROXY` value `src/fetch/ssrf.rs`'s `detect_egress_mode` reads but
//! `reqwest::Proxy::all` cannot parse (74, `T-C026`). None of the three sets
//! `SCOUT_MAX_RETRIES`: the `fetch` path these tests exercise calls nothing
//! from `src/retry.rs` (`retry_with_rate_limit` is called from the Brave,
//! GitHub, and Slack clients, none of which this path enters), so there is
//! nothing for that env var to change here.
//!
//! Exit 70 (`ErrorCode::Internal`, EX_SOFTWARE) is out of scope for this file
//! on purpose, not by oversight: every constructor of it (`SlackError::Decode`
//! / `ParseUrl`, `BraveError::ParseJson` / `ResponseTooLarge`,
//! `GitHubError::Decode` / `ResponseTooLarge`) sits in a non-`fetch` backend's
//! JSON-deserialize path, and `FetchError::classify` (src/fetch.rs) has no arm
//! that reaches `Internal` at all. None of those backends expose an env var
//! this file's proxy/timeout harness can turn into a deterministic
//! malformed-JSON response the way `HTTP_PROXY` and
//! `SCOUT_FETCH_TIMEOUT_SECS` do here, so 70 has no external construction path
//! through this contract's fetch-only surface.

mod common;

use common::{parse_envelope, scout};
use std::env;
use std::process::Output;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Runs `scout --json fetch http://example.com/` with a from-scratch
/// environment (`PATH`/`HOME` restored so the OS proxy lookup and any config
/// file it reads still resolve, everything else cleared so a var set in the
/// invoking shell can't leak into the contract) plus `extra_env` layered on
/// top. Shared by every scenario below, which differ only in `extra_env`.
fn run_scout_fetch(extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = scout();
    cmd.env_clear()
        .env("PATH", env::var("PATH").unwrap_or_default())
        .env("HOME", env::var("HOME").unwrap_or_default());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.args(["--json", "fetch", "http://example.com/"])
        .output()
        .expect("scout --json fetch failed to run")
}

/// Drive `scout --json fetch` through `common::spawn_mock_proxy` answering
/// every request with `proxy_status`, then assert the exit code and
/// `error.code` the ADR-0003 table demands for it, plus the proxy having
/// actually been dialed at least once.
///
/// `HTTP_PROXY` routes `fetch` through the mock proxy (`EgressMode::Proxied`,
/// src/fetch/ssrf.rs::detect_egress_mode); the target URL is a domain name
/// (not an IP literal) so `ssrf_check` does not reject it before the proxy is
/// ever dialed, and its DNS is never consulted because the proxy — not
/// scout — would be the one resolving and dialing it.
fn assert_proxy_status_maps_to(
    proxy_status: u16,
    expected_exit_code: i32,
    expected_error_code: &str,
) {
    let Some((proxy_url, connection_count, _handle)) =
        common::spawn_mock_proxy(proxy_status, Duration::ZERO, b"upstream response body")
    else {
        return; // loopback bind unavailable in this environment
    };

    let output = run_scout_fetch(&[("HTTP_PROXY", &proxy_url)]);

    assert_eq!(
        output.status.code(),
        Some(expected_exit_code),
        "proxy status {proxy_status} should exit {expected_exit_code}, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value = parse_envelope(&output, &format!("proxy status {proxy_status}"));
    assert_eq!(
        value["error"]["code"], expected_error_code,
        "proxy status {proxy_status} should classify as {expected_error_code}, got: {value}"
    );

    assert!(
        connection_count.load(Ordering::SeqCst) >= 1,
        "expected at least one connection to reach the mock proxy for status {proxy_status}, \
         got 0 — a 0 count means the exit code above did not travel through the proxy response \
         path (e.g. a DNS/SSRF short-circuit produced the same code by coincidence), which would \
         be a false positive for this contract"
    );
}

// [T-C020] proxy 経由の 404 応答は exit code 66 と error.code NOT_FOUND になる
#[test]
fn proxy_経由の_404_応答は_exit_code_66_と_error_code_not_found_になる() {
    assert_proxy_status_maps_to(404, 66, "NOT_FOUND");
}

// [T-C021] proxy 経由の 403 応答は exit code 64 と error.code USAGE_ERROR になる
#[test]
fn proxy_経由の_403_応答は_exit_code_64_と_error_code_usage_error_になる() {
    assert_proxy_status_maps_to(403, 64, "USAGE_ERROR");
}

// [T-C022] proxy 経由の 400 応答は exit code 65 と error.code DATA_ERROR になる
#[test]
fn proxy_経由の_400_応答は_exit_code_65_と_error_code_data_error_になる() {
    assert_proxy_status_maps_to(400, 65, "DATA_ERROR");
}

// [T-C023] proxy 経由の 500 応答は exit code 75 と error.code TEMP_FAILURE になる
#[test]
fn proxy_経由の_500_応答は_exit_code_75_と_error_code_temp_failure_になる() {
    assert_proxy_status_maps_to(500, 75, "TEMP_FAILURE");
}

// [T-C024] proxy の応答遅延が SCOUT_FETCH_TIMEOUT_SECS を超えると exit code 124 と error.code TIMEOUT になる
//
// `Scout::fetch` (src/tools/query.rs) wraps `fetch_page` in
// `tokio::time::timeout(self.config.fetch_timeout, ..)`; a slower response
// fires that call's own `Err(FetchError::Timeout(..))` fallback, which
// `FetchError::classify`'s `Self::Timeout(_) => Classification::timeout_retry()`
// arm (src/fetch.rs) turns into exit 124. `SCOUT_FETCH_TIMEOUT_SECS=1` is the
// lowest value `src/tools/config.rs`'s `parse_timeout` accepts
// (`TIMEOUT_MIN_SECS`), so a 2s mock-proxy delay clears it with margin without
// slowing the suite more than necessary.
#[test]
fn proxy_の応答遅延が_scout_fetch_timeout_secsを超えると_exit_code_124_と_error_code_timeout_になる()
 {
    let Some((proxy_url, connection_count, _handle)) =
        common::spawn_mock_proxy(200, Duration::from_secs(2), b"too slow to matter")
    else {
        return; // loopback bind unavailable in this environment
    };

    let output = run_scout_fetch(&[
        ("HTTP_PROXY", &proxy_url),
        ("SCOUT_FETCH_TIMEOUT_SECS", "1"),
    ]);

    assert_eq!(
        output.status.code(),
        Some(124),
        "a proxy response slower than SCOUT_FETCH_TIMEOUT_SECS should exit 124 (GNU coreutils \
         timeout convention), got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value = parse_envelope(&output, "slow proxy response");
    assert_eq!(
        value["error"]["code"], "TIMEOUT",
        "a fetch exceeding SCOUT_FETCH_TIMEOUT_SECS should classify as TIMEOUT, got: {value}"
    );

    assert!(
        connection_count.load(Ordering::SeqCst) >= 1,
        "expected the proxy to have been dialed at least once — a 0 count would mean the \
         timeout fired before the request ever reached the proxy, which would not prove the \
         SCOUT_FETCH_TIMEOUT_SECS path this test targets"
    );
}

// [T-C025] proxy が非 HTTP バイト列を返すと exit code 104 と error.code UNKNOWN になる
//
// Reached through `FetchError::Http(re) => Classification::from_reqwest(re)`
// (src/fetch.rs), not the ADR-0003 status table `T-C020`–`T-C023` exercise: a
// response with no status line has no `Status(u16)` to classify.
//
// reqwest-version-bound: on reqwest 0.13.4 (pinned in Cargo.lock, verified
// this session — see notes) this malformed proxy response surfaces as a
// `SendRequest` / "invalid HTTP version parsed" `reqwest::Error` with
// `is_decode() == false`, `is_connect() == false`, `is_timeout() == false`, so
// none of `retry::is_transient_network`'s checks fire and
// `Classification::from_reqwest` falls to its `Unknown` retreat slot. A
// reqwest upgrade that reclassifies this exact byte sequence so
// `is_decode()`/`is_connect()`/`is_timeout()` turns true would move the exit
// code to 75 or 124 (`TempFailure`/`Timeout`); that is a test-update event for
// this test's fixture, not a regression in `from_reqwest` itself.
#[test]
fn proxy_が非_http_バイト列を返すと_exit_code_104_と_error_code_unknown_になる() {
    let Some((proxy_url, connection_count, _handle)) = common::spawn_mock_proxy_raw_response(
        b"not an http response at all, just garbage bytes\r\n\r\n",
    ) else {
        return; // loopback bind unavailable in this environment
    };

    let output = run_scout_fetch(&[("HTTP_PROXY", &proxy_url)]);

    assert_eq!(
        output.status.code(),
        Some(104),
        "a non-HTTP proxy response should exit 104 (PJ extension, unclassifiable failure per \
         ADR-0002), got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value = parse_envelope(&output, "non-HTTP proxy response");
    assert_eq!(
        value["error"]["code"], "UNKNOWN",
        "a non-HTTP proxy response should classify as UNKNOWN, got: {value}"
    );

    assert!(
        connection_count.load(Ordering::SeqCst) >= 1,
        "expected at least one connection to reach the mock proxy — a 0 count means the exit \
         code above did not travel through the proxy response path"
    );
}

// [T-C026] 不正な HTTP_PROXY 値での起動は exit code 74 と error.code IO_ERROR になる
//
// Proven through `build_default_clients`'s own
// `Proxy::all(url).map_err(|e| ScoutError::io_error(..))` arm
// (src/tools/builder.rs), not through `FetchError::classify` /
// `Classification::from_reqwest` (the reqwest-error priority table `T-C025`
// exercises): `ScoutBuilder::from_env` runs inside `Scout::new()` before
// `cli.command` ever dispatches to a handler (src/lib.rs), so this failure
// happens before any command handler or proxy connection — no mock proxy is
// spawned for this test, unlike every other scenario in this file.
//
// reqwest-version-bound: on reqwest 0.13.4 (pinned in Cargo.lock),
// <https://docs.rs/reqwest/0.13/reqwest/struct.Proxy.html#method.all> does not
// document which URL forms it accepts or rejects (checked this session, nothing
// on the page states it — see notes), so the literal value below was confirmed
// empirically this session, not from that page. A reqwest upgrade that starts
// accepting this exact literal is a test-update event for this test's fixture,
// not a builder-path regression.
#[test]
fn 不正な_http_proxy_値での起動は_exit_code_74_と_error_code_io_error_になる() {
    let output = run_scout_fetch(&[("HTTP_PROXY", "not a url with spaces")]);

    assert_eq!(
        output.status.code(),
        Some(74),
        "an HTTP_PROXY value reqwest::Proxy::all cannot parse should fail client construction \
         (exit 74 EX_IOERR), got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value = parse_envelope(&output, "invalid HTTP_PROXY value");
    assert_eq!(
        value["error"]["code"], "IO_ERROR",
        "invalid HTTP_PROXY should classify as IO_ERROR, got: {value}"
    );
}
