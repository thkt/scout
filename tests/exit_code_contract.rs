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

mod common;

use std::env;
use std::process::{Command, Output};
use std::sync::atomic::Ordering;
use std::time::Duration;

fn scout() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scout"))
}

/// Same rule as `tests/cli_integration.rs`'s `parse_envelope`: scan stderr
/// line by line for the first line that parses as JSON, because
/// `init_tracing`'s WARN/INFO lines share stderr with the envelope.
fn parse_envelope(output: &Output, context: &str) -> serde_json::Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| {
            panic!("{context} stderr should contain a JSON envelope line, got:\n{stderr}")
        });
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("{context} envelope must be valid JSON ({e}): {line}"))
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

    let output = scout()
        .env_clear()
        .env("PATH", env::var("PATH").unwrap_or_default())
        .env("HOME", env::var("HOME").unwrap_or_default())
        .env("HTTP_PROXY", &proxy_url)
        .args(["--json", "fetch", "http://example.com/"])
        .output()
        .expect("scout --json fetch failed to run");

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

// [T-001] proxy 経由の 404 応答は exit code 66 と error.code NOT_FOUND になる
#[test]
fn proxy_経由の_404_応答は_exit_code_66_と_error_code_not_found_になる() {
    assert_proxy_status_maps_to(404, 66, "NOT_FOUND");
}

// [T-002] proxy 経由の 403 応答は exit code 64 と error.code USAGE_ERROR になる
#[test]
fn proxy_経由の_403_応答は_exit_code_64_と_error_code_usage_error_になる() {
    assert_proxy_status_maps_to(403, 64, "USAGE_ERROR");
}

// [T-003] proxy 経由の 400 応答は exit code 65 と error.code DATA_ERROR になる
#[test]
fn proxy_経由の_400_応答は_exit_code_65_と_error_code_data_error_になる() {
    assert_proxy_status_maps_to(400, 65, "DATA_ERROR");
}

// [T-004] proxy 経由の 500 応答は exit code 75 と error.code TEMP_FAILURE になる
#[test]
fn proxy_経由の_500_応答は_exit_code_75_と_error_code_temp_failure_になる() {
    assert_proxy_status_maps_to(500, 75, "TEMP_FAILURE");
}
