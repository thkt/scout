//! CLI exit-code and JSON error.code contracts for proxy responses (DR-0003).
//! HTTP-status cases and T-C024/025 require a proxy connection to rule out
//! coincidental SSRF or DNS failures. T-C026 fails during client construction;
//! T-C027 checks timeout wording without asserting a connection count.
//!
//! Fetch has no retry-helper calls, so SCOUT_MAX_RETRIES is irrelevant here.
//! Exit 70 has no construction path through this fetch-only harness.

mod common;

#[path = "common/tests.rs"]
mod common_tests;

use common::parse_envelope;
use std::process::Output;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

fn run_scout_fetch(extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = common::scout_with_clean_env();
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.args(["--json", "fetch", "http://example.com/"])
        .output()
        .expect("scout --json fetch failed to run")
}

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

fn assert_proxy_was_dialed_for_exit_code(connection_count: &AtomicUsize, context: &str) {
    common::assert_proxy_was_dialed(
        connection_count,
        context,
        "the exit code above did not travel through the proxy response path",
    );
}

/// Uses a domain target so the configured proxy handles resolution instead of
/// triggering scout's IP-literal preflight rejection.
fn assert_proxy_status_maps_to(
    proxy_status: u16,
    expected_exit_code: i32,
    expected_error_code: &str,
) {
    let Some((proxy_url, connection_count, _handle)) =
        common::spawn_mock_proxy(proxy_status, Duration::ZERO, b"upstream response body")
    else {
        return;
    };

    let output = run_scout_fetch(&[("HTTP_PROXY", &proxy_url)]);
    let context = format!("proxy status {proxy_status}");

    assert_exits_with(&output, expected_exit_code, expected_error_code, &context);
    assert_proxy_was_dialed_for_exit_code(&connection_count, &context);
}

// T-C020
#[test]
fn proxied_404_exits_66_not_found() {
    assert_proxy_status_maps_to(404, 66, "NOT_FOUND");
}

// T-C021
#[test]
fn proxied_403_exits_64_usage_error() {
    assert_proxy_status_maps_to(403, 64, "USAGE_ERROR");
}

// T-C022
#[test]
fn proxied_400_exits_65_data_error() {
    assert_proxy_status_maps_to(400, 65, "DATA_ERROR");
}

// T-C023
#[test]
fn proxied_500_exits_75_temp_failure() {
    assert_proxy_status_maps_to(500, 75, "TEMP_FAILURE");
}

// T-C024: A 2s proxy delay exceeds the minimum 1s outer fetch timeout.
#[test]
fn proxy_response_slower_than_fetch_timeout_exits_124_timeout() {
    let Some((proxy_url, connection_count, _handle)) =
        common::spawn_mock_proxy(200, Duration::from_secs(2), b"too slow to matter")
    else {
        return;
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
    assert_proxy_was_dialed_for_exit_code(&connection_count, "slow proxy response");
}

// T-C025: Malformed proxy bytes yield 104/UNKNOWN. This fixture classification
// depends on reqwest; dependency upgrades may change the error category.
#[test]
fn non_http_proxy_response_exits_104_unknown() {
    let Some((proxy_url, connection_count, _handle)) = common::spawn_mock_proxy_raw_response(
        b"not an http response at all, just garbage bytes\r\n\r\n",
    ) else {
        return;
    };

    let output = run_scout_fetch(&[("HTTP_PROXY", &proxy_url)]);

    assert_exits_with(&output, 104, "UNKNOWN", "a non-HTTP proxy response");
    assert_proxy_was_dialed_for_exit_code(&connection_count, "non-HTTP proxy response");
}

// T-C026: Invalid proxy configuration yields 74/IO_ERROR before dispatch.
// Acceptance of this literal depends on reqwest.
#[test]
fn unparsable_http_proxy_value_exits_74_io_error() {
    let output = run_scout_fetch(&[("HTTP_PROXY", "not a url with spaces")]);

    assert_exits_with(
        &output,
        74,
        "IO_ERROR",
        "an HTTP_PROXY value reqwest::Proxy::all cannot parse",
    );
}

// T-C027: Actual outer timeout message contains "timed out" exactly once.
#[test]
fn fetch_timeout_message_states_the_timeout_once() {
    let Some((proxy_url, _connection_count, _handle)) =
        common::spawn_mock_proxy(200, Duration::from_secs(2), b"too slow to matter")
    else {
        return;
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

#[cfg(all(unix, feature = "js-rendering"))]
#[path = "exit_code_contract/browser_cleanup.rs"]
mod browser_cleanup;
