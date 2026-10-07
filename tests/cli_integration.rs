mod common;

use common::{parse_envelope, scout};
use std::process::Output;

// T-C001: help_exits_zero_and_contains_app_name
#[test]
fn help_exits_zero_and_contains_app_name() {
    let output = scout().arg("--help").output().expect("scout --help failed");
    assert_eq!(output.status.code(), Some(0), "help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scout"),
        "help output should mention app name, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Exit codes"),
        "help output should contain Exit codes section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("sysexits.h"),
        "help should reference sysexits.h, got:\n{stdout}"
    );
    // ADR-0002 — every documented non-zero exit code must surface in --help so
    // agent/script callers can discover the contract without reading source.
    // Match the `  CODE  ` table layout to avoid substring collisions (e.g.,
    // "75" inside a future "175 RPM" mention).
    for code in ["64", "65", "66", "70", "74", "75", "104", "124"] {
        let needle = format!("  {code}  ");
        assert!(
            stdout.contains(&needle),
            "help should advertise exit code {code} as `{needle}` per ADR-0002, got:\n{stdout}"
        );
    }
}

// T-C002: version_exits_zero
#[test]
fn version_exits_zero() {
    let output = scout()
        .arg("--version")
        .output()
        .expect("scout --version failed");
    assert_eq!(output.status.code(), Some(0), "version should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scout"),
        "version output should contain app name, got:\n{stdout}"
    );
}

// T-C013: version_points_coding_agents_at_help
#[test]
fn version_points_coding_agents_at_help() {
    let output = scout()
        .arg("--version")
        .output()
        .expect("scout --version failed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("coding agent") && stderr.contains("--help"),
        "version should point a coding agent at help on stderr, got:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("coding agent"),
        "the hint must stay off stdout so the version line remains parseable, got:\n{stdout}"
    );
}

// T-C014: non_utf8_argument_is_a_usage_error_not_a_panic
#[cfg(unix)]
#[test]
fn non_utf8_argument_is_a_usage_error_not_a_panic() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let output = scout()
        .arg("fetch")
        .arg(OsStr::from_bytes(b"\xff"))
        .output()
        .expect("scout fetch failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "a non-UTF-8 argument is caller input, so it belongs on the usage-error \
         path, not an abort; got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// T-C003: search_without_api_key_exits_64
#[test]
fn search_without_api_key_exits_64() {
    let output = scout()
        .args(["search", "test query"])
        .env_remove("BRAVE_SEARCH_API_KEY")
        .output()
        .expect("scout search failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "missing API key should be exit 64 (EX_USAGE per ADR-0002)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// [T-C038]
// Setup: env `BRAVE_SEARCH_API_KEY="   "` (whitespace only).
// Action: run `scout search "test query"`.
// Expected: stderr contains `BRAVE_SEARCH_API_KEY`, exit code 64 (EX_USAGE);
// whitespace-only key is treated as missing because
// brave/client.rs::from_env applies `trim().is_empty()`.
#[test]
fn search_with_whitespace_only_api_key_exits_64() {
    let output = scout()
        .args(["search", "test query"])
        .env("BRAVE_SEARCH_API_KEY", "   ")
        .output()
        .expect("scout search failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "whitespace-only API key should be treated as missing (exit 64)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BRAVE_SEARCH_API_KEY"),
        "stderr should mention BRAVE_SEARCH_API_KEY, got:\n{stderr}"
    );
}

// T-C004: fetch_invalid_url_exits_65
#[test]
fn fetch_invalid_url_exits_65() {
    let output = scout()
        .args(["fetch", "not-a-valid-url"])
        .output()
        .expect("scout fetch failed to run");
    assert_eq!(
        output.status.code(),
        Some(65),
        "invalid URL should be exit 65 (EX_DATAERR per ADR-0002)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C005: repo_tree_bad_format_exits_65
#[test]
fn repo_tree_bad_format_exits_65() {
    let output = scout()
        .args(["repo-tree", "no-slash-here"])
        .output()
        .expect("scout repo-tree failed to run");
    assert_eq!(
        output.status.code(),
        Some(65),
        "malformed owner/repo should be exit 65 (EX_DATAERR per ADR-0002)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C006: help_advertises_json_flag — --json appears in --help under Options
#[test]
fn help_advertises_json_flag() {
    let output = scout().arg("--help").output().expect("scout --help failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--json"),
        "help output should advertise --json flag, got:\n{stdout}"
    );
}

// T-C007: json_emits_envelope_on_error — --json with malformed repo emits a JSON envelope on stderr
#[test]
fn json_emits_envelope_on_error() {
    let output = scout()
        .args(["--json", "repo-tree", "no-slash-here"])
        .output()
        .expect("scout --json repo-tree failed to run");
    assert_eq!(
        output.status.code(),
        Some(65),
        "malformed owner/repo should still exit 65 (EX_DATAERR) in --json mode"
    );
    let value = parse_envelope(&output, "malformed owner/repo");
    assert_eq!(
        value["error"]["code"], "DATA_ERROR",
        "code should be DATA_ERROR per ADR-0010, got: {value}"
    );
    assert_eq!(
        value["error"]["retryable"], false,
        "retryable should be false for DATA_ERROR, got: {value}"
    );
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Invalid repository format")),
        "message should describe the input failure, got: {value}"
    );
    assert!(
        value["error"]["next_step"]
            .as_str()
            .is_some_and(|s| s.contains("owner/repo")),
        "next_step should hint at owner/repo format, got: {value}"
    );
    assert!(
        value["error"].get("candidates").is_none(),
        "candidates should be omitted when empty, got: {value}"
    );
}

// T-C008: json_missing_api_key_emits_usage_error_with_next_step — --json missing API key surfaces a USAGE_ERROR envelope with next_step on stderr
#[test]
fn json_missing_api_key_emits_usage_error_with_next_step() {
    let output = scout()
        .args(["--json", "search", "test query"])
        .env_remove("BRAVE_SEARCH_API_KEY")
        .output()
        .expect("scout --json search failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "missing API key → exit 64 (EX_USAGE)"
    );
    let value = parse_envelope(&output, "missing API key");
    assert_eq!(
        value["error"]["code"], "USAGE_ERROR",
        "missing API key is a usage problem per ADR-0011 priority 1, got: {value}"
    );
    assert!(
        value["error"]["next_step"]
            .as_str()
            .is_some_and(|s| s.contains("BRAVE_SEARCH_API_KEY")),
        "next_step should point user to BRAVE_SEARCH_API_KEY, got: {value}"
    );
}

// T-C010: json_clap_parse_error_emits_envelope — --json with a clap parse error (unknown flag) routes through JSON envelope
#[test]
fn json_clap_parse_error_emits_envelope() {
    let output = scout()
        .args(["--json", "--definitely-not-a-flag"])
        .output()
        .expect("scout failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "clap parse error should exit 64 (EX_USAGE per ADR-0002)"
    );
    let value = parse_envelope(&output, "clap parse error");
    assert_eq!(
        value["error"]["code"], "USAGE_ERROR",
        "clap parse error should be USAGE_ERROR per ADR-0011 priority 1, got: {value}"
    );
}

/// Exit 65 alone cannot distinguish the SSRF rejection from any other
/// `DataError` variant reachable on these paths, so the code and next_step
/// asserts below are what pin the contract.
fn assert_reject_envelope(output: &Output, form_name: &str) -> serde_json::Value {
    assert_eq!(
        output.status.code(),
        Some(65),
        "{form_name} literal loopback fetch should exit 65 (EX_DATAERR), got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = parse_envelope(output, form_name);
    assert_eq!(
        value["error"]["code"], "DATA_ERROR",
        "{form_name} literal loopback should classify as DATA_ERROR, got: {value}"
    );
    assert!(
        value["error"]["next_step"]
            .as_str()
            .is_some_and(|s| s.contains("private IPs are blocked")),
        "{form_name} next_step should state private IPs are blocked, got: {value}"
    );
    value
}

// T-C017: localhost_hostname_fetch_exits_65_data_error — Host::Domain arm, not the IP-literal one
#[test]
fn localhost_hostname_fetch_exits_65_data_error() {
    let mut cmd = scout();
    cmd.env_clear();
    common::forward_coverage_profile(&mut cmd);
    let output = cmd
        .args(["--json", "fetch", "http://localhost/"])
        .output()
        .expect("scout --json fetch failed to run");
    assert_reject_envelope(&output, "localhost hostname");
}

// T-C019: direct_proxied_and_js_launch_forms_return_same_error_code_and_next_step_for_the_same_url
//
// Each launch form must reject literal loopback with exit 65, DATA_ERROR and
// private-IP guidance; their next_step values must also agree. The --js row
// requires js-rendering: without it BrowserNotFound (USAGE_ERROR) precedes
// ssrf_check. With it rejection runs before fetch_with_cdp launches Chromium.
// cfg! keeps the push visible to the default build's unused-mut analysis.
struct LaunchForm {
    name: &'static str,
    args: Vec<&'static str>,
    env: Vec<(&'static str, &'static str)>,
}

#[test]
fn direct_proxied_and_js_launch_forms_return_same_error_code_and_next_step_for_the_same_url() {
    let mut forms = vec![
        LaunchForm {
            name: "Direct",
            args: vec![],
            env: vec![],
        },
        LaunchForm {
            name: "Proxied",
            args: vec![],
            // A missed rejection fails fast at port 1 instead of hanging.
            // Rejection precedes dialing in both modes, so this does not prove
            // proxy selection; detect_egress_mode's tests cover that contract.
            env: vec![("HTTP_PROXY", "http://127.0.0.1:1")],
        },
    ];
    if cfg!(feature = "js-rendering") {
        forms.push(LaunchForm {
            name: "--js",
            args: vec!["--js"],
            env: vec![],
        });
    }

    let mut envelopes: Vec<(&str, serde_json::Value)> = Vec::new();
    for form in &forms {
        let mut cmd = scout();
        cmd.env_clear();
        common::forward_coverage_profile(&mut cmd);
        for (key, val) in &form.env {
            cmd.env(key, val);
        }
        cmd.arg("--json").arg("fetch");
        for arg in &form.args {
            cmd.arg(arg);
        }
        cmd.arg("http://127.0.0.1/");
        let output = cmd
            .output()
            .unwrap_or_else(|e| panic!("scout --json fetch ({}) failed to run: {e}", form.name));
        let envelope = assert_reject_envelope(&output, form.name);
        envelopes.push((form.name, envelope));
    }

    let (baseline_name, baseline) = &envelopes[0];
    for (name, envelope) in &envelopes[1..] {
        assert_eq!(
            envelope["error"]["next_step"], baseline["error"]["next_step"],
            "{name} should return the same next_step as {baseline_name}, got: {envelope}"
        );
    }
}

// T-C009: json_error_envelope_is_single_line — --json error envelope is exactly one line (single-line JSON contract)
#[test]
fn json_error_envelope_is_single_line() {
    let output = scout()
        .args(["--json", "repo-tree", "no-slash-here"])
        .output()
        .expect("scout --json repo-tree failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with('{')).collect();
    assert_eq!(
        json_lines.len(),
        1,
        "expected exactly one JSON envelope line, got: {json_lines:?}"
    );
}
