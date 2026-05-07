use std::process::Command;

fn scout() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scout"))
}

// T-C001
#[test]
fn t_c001_help_exits_zero_and_contains_app_name() {
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
}

// T-C002
#[test]
fn t_c002_version_exits_zero() {
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

// T-C003
#[test]
fn t_c003_search_without_api_key_exits_64() {
    let output = scout()
        .args(["search", "test query"])
        .env_remove("GEMINI_API_KEY")
        .output()
        .expect("scout search failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "missing API key should be exit 64 (EX_USAGE per ADR-0065)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C004
#[test]
fn t_c004_fetch_invalid_url_exits_65() {
    let output = scout()
        .args(["fetch", "not-a-valid-url"])
        .output()
        .expect("scout fetch failed to run");
    assert_eq!(
        output.status.code(),
        Some(65),
        "invalid URL should be exit 65 (EX_DATAERR per ADR-0065)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C005
#[test]
fn t_c005_repo_tree_bad_format_exits_65() {
    let output = scout()
        .args(["repo-tree", "no-slash-here"])
        .output()
        .expect("scout repo-tree failed to run");
    assert_eq!(
        output.status.code(),
        Some(65),
        "malformed owner/repo should be exit 65 (EX_DATAERR per ADR-0065)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C006: --json appears in --help under Options
#[test]
fn t_c006_help_advertises_json_flag() {
    let output = scout().arg("--help").output().expect("scout --help failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--json"),
        "help output should advertise --json flag, got:\n{stdout}"
    );
}

// T-C007: --json with malformed repo emits a JSON envelope on stderr
#[test]
fn t_c007_json_emits_envelope_on_error() {
    let output = scout()
        .args(["--json", "repo-tree", "no-slash-here"])
        .output()
        .expect("scout --json repo-tree failed to run");
    assert_eq!(
        output.status.code(),
        Some(65),
        "malformed owner/repo should still exit 65 (EX_DATAERR) in --json mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.starts_with('{'))
        .expect("stderr should contain a JSON envelope line");
    let value: serde_json::Value = serde_json::from_str(line).expect("envelope must be valid JSON");
    assert_eq!(
        value["error"]["code"], "DATA_ERROR",
        "code should be DATA_ERROR per ADR-0065, got: {value}"
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

// T-C008: --json missing API key surfaces a USAGE_ERROR envelope with next_step on stderr
#[test]
fn t_c008_json_missing_api_key_emits_usage_error_with_next_step() {
    let output = scout()
        .args(["--json", "search", "test query"])
        .env_remove("GEMINI_API_KEY")
        .output()
        .expect("scout --json search failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "missing API key → exit 64 (EX_USAGE)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.starts_with('{'))
        .expect("stderr should contain a JSON envelope line");
    let value: serde_json::Value = serde_json::from_str(line).expect("envelope must be valid JSON");
    assert_eq!(
        value["error"]["code"], "USAGE_ERROR",
        "missing API key is a usage problem per ADR-0065, got: {value}"
    );
    assert!(
        value["error"]["next_step"]
            .as_str()
            .is_some_and(|s| s.contains("GEMINI_API_KEY")),
        "next_step should point user to GEMINI_API_KEY, got: {value}"
    );
}

// T-C010: --json with a clap parse error (unknown flag) routes through JSON envelope
#[test]
fn t_c010_json_clap_parse_error_emits_envelope() {
    let output = scout()
        .args(["--json", "--definitely-not-a-flag"])
        .output()
        .expect("scout failed to run");
    assert_eq!(
        output.status.code(),
        Some(64),
        "clap parse error should exit 64 (EX_USAGE per ADR-0065)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.starts_with('{'))
        .expect("stderr should contain a JSON envelope line for clap parse errors");
    let value: serde_json::Value = serde_json::from_str(line).expect("envelope must be valid JSON");
    assert_eq!(
        value["error"]["code"], "USAGE_ERROR",
        "clap parse error should be USAGE_ERROR per ADR-0065, got: {value}"
    );
}

// T-C009: --json error envelope is exactly one line (single-line JSON contract)
#[test]
fn t_c009_json_error_envelope_is_single_line() {
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
