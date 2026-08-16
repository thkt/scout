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
//! `T-C027` reuses `T-C024`'s slow-proxy setup to pin a different contract: the
//! wording of the `error.message` a timeout produces, rather than its exit code.
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

use common::parse_envelope;
use std::process::Output;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

/// Runs `scout --json fetch http://example.com/` with `extra_env` layered on
/// top of `common::scout_with_clean_env`. Shared by every scenario below,
/// which differ only in `extra_env`.
fn run_scout_fetch(extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = common::scout_with_clean_env();
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.args(["--json", "fetch", "http://example.com/"])
        .output()
        .expect("scout --json fetch failed to run")
}

/// Assert the exit code and `error.code` one run produced. The two travel
/// together — `ErrorCode` (src/envelope.rs) is what decides both — so a
/// scenario that pinned only one of them would leave the other free to drift.
/// `context` names the scenario in the failure output.
fn assert_exits_with(
    output: &Output,
    expected_exit_code: i32,
    expected_error_code: &str,
    context: &str,
) {
    assert_eq!(
        output.status.code(),
        Some(expected_exit_code),
        "{context} should exit {expected_exit_code}, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value = parse_envelope(output, context);
    assert_eq!(
        value["error"]["code"], expected_error_code,
        "{context} should classify as {expected_error_code}, got: {value}"
    );
}

/// Supplies this file's `consequence` wording once instead of at each of the
/// three call sites below.
fn assert_proxy_was_dialed_for_exit_code(connection_count: &AtomicUsize, context: &str) {
    common::assert_proxy_was_dialed(
        connection_count,
        context,
        "the exit code above did not travel through the proxy response path",
    );
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
        return; // bind_loopback ruled this a skip, not a failure
    };

    let output = run_scout_fetch(&[("HTTP_PROXY", &proxy_url)]);
    let context = format!("proxy status {proxy_status}");

    assert_exits_with(&output, expected_exit_code, expected_error_code, &context);
    assert_proxy_was_dialed_for_exit_code(&connection_count, &context);
}

// T-C020: proxied_404_exits_66_not_found
#[test]
fn proxied_404_exits_66_not_found() {
    assert_proxy_status_maps_to(404, 66, "NOT_FOUND");
}

// T-C021: proxied_403_exits_64_usage_error
#[test]
fn proxied_403_exits_64_usage_error() {
    assert_proxy_status_maps_to(403, 64, "USAGE_ERROR");
}

// T-C022: proxied_400_exits_65_data_error
#[test]
fn proxied_400_exits_65_data_error() {
    assert_proxy_status_maps_to(400, 65, "DATA_ERROR");
}

// T-C023: proxied_500_exits_75_temp_failure
#[test]
fn proxied_500_exits_75_temp_failure() {
    assert_proxy_status_maps_to(500, 75, "TEMP_FAILURE");
}

// T-C024: proxy_response_slower_than_fetch_timeout_exits_124_timeout
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
fn proxy_response_slower_than_fetch_timeout_exits_124_timeout() {
    let Some((proxy_url, connection_count, _handle)) =
        common::spawn_mock_proxy(200, Duration::from_secs(2), b"too slow to matter")
    else {
        return; // bind_loopback ruled this a skip, not a failure
    };

    let output = run_scout_fetch(&[
        ("HTTP_PROXY", &proxy_url),
        ("SCOUT_FETCH_TIMEOUT_SECS", "1"),
    ]);

    assert_exits_with(
        &output,
        124,
        "TIMEOUT",
        "a proxy response slower than the fetch timeout",
    );
    // A 0 count here would mean the timeout fired before the request reached
    // the proxy, which is a different path from the one this test targets.
    assert_proxy_was_dialed_for_exit_code(&connection_count, "slow proxy response");
}

// T-C025: non_http_proxy_response_exits_104_unknown
//
// Reached through `FetchError::Http(re) => Classification::from_reqwest(re)`
// (src/fetch.rs), not the ADR-0003 status table `T-C020`–`T-C023` exercise: a
// response with no status line has no `Status(u16)` to classify.
//
// reqwest-version-bound: on reqwest 0.13.4 (pinned in Cargo.lock) this
// malformed proxy response surfaces as a `SendRequest` / "invalid HTTP
// version parsed" `reqwest::Error` with
// `is_decode() == false`, `is_connect() == false`, `is_timeout() == false`, so
// none of `retry::is_transient_network`'s checks fire and
// `Classification::from_reqwest` falls to its `Unknown` retreat slot. A
// reqwest upgrade that reclassifies this exact byte sequence so
// `is_decode()`/`is_connect()`/`is_timeout()` turns true would move the exit
// code to 75 or 124 (`TempFailure`/`Timeout`); that is a test-update event for
// this test's fixture, not a regression in `from_reqwest` itself.
#[test]
fn non_http_proxy_response_exits_104_unknown() {
    let Some((proxy_url, connection_count, _handle)) = common::spawn_mock_proxy_raw_response(
        b"not an http response at all, just garbage bytes\r\n\r\n",
    ) else {
        return; // bind_loopback ruled this a skip, not a failure
    };

    let output = run_scout_fetch(&[("HTTP_PROXY", &proxy_url)]);

    // 104 is the PJ extension ADR-0002 gives an unclassifiable failure.
    assert_exits_with(&output, 104, "UNKNOWN", "a non-HTTP proxy response");
    assert_proxy_was_dialed_for_exit_code(&connection_count, "non-HTTP proxy response");
}

// T-C026: unparsable_http_proxy_value_exits_74_io_error
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
// document which URL forms it accepts or rejects, so the literal value below
// was confirmed empirically rather than read off that page. A reqwest upgrade
// that starts accepting this exact literal is a test-update event for this
// test's fixture, not a builder-path regression.
#[test]
fn unparsable_http_proxy_value_exits_74_io_error() {
    let output = run_scout_fetch(&[("HTTP_PROXY", "not a url with spaces")]);

    // 74 is EX_IOERR, reached by client construction failing rather than by a
    // request failing.
    assert_exits_with(
        &output,
        74,
        "IO_ERROR",
        "an HTTP_PROXY value reqwest::Proxy::all cannot parse",
    );
}

// T-C027: fetch_timeout_message_states_the_timeout_once
//
// Pins the payload rule stated on `FetchError::Timeout` (src/fetch.rs) for the
// `Scout::fetch` call site, where the wrapper can double the payload into
// "fetch timed out: fetch timed out after 1s".
//
// Repeating `T-C024`'s scenario rather than asserting on that run keeps each ID
// pinning one contract. Driving a real timeout is what makes the assertion
// non-tautological: a `FetchError::Timeout` built in-process would assert on a
// payload this test wrote itself.
#[test]
fn fetch_timeout_message_states_the_timeout_once() {
    let Some((proxy_url, _connection_count, _handle)) =
        common::spawn_mock_proxy(200, Duration::from_secs(2), b"too slow to matter")
    else {
        return; // bind_loopback ruled this a skip, not a failure
    };

    let output = run_scout_fetch(&[
        ("HTTP_PROXY", &proxy_url),
        ("SCOUT_FETCH_TIMEOUT_SECS", "1"),
    ]);

    let envelope = parse_envelope(&output, "a proxy response slower than the fetch timeout");
    let message = envelope["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("error.message should be a string, got: {envelope}"));
    assert_eq!(
        message.matches("timed out").count(),
        1,
        "error.message should state the timeout once, got: {message}"
    );
}
