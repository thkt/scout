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
        stdout.contains("Exit codes:"),
        "help output should contain Exit codes: section, got:\n{stdout}"
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
fn t_c003_search_without_api_key_exits_1() {
    let output = scout()
        .args(["search", "test query"])
        .env_remove("GEMINI_API_KEY")
        .output()
        .expect("scout search failed to run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "missing API key should be exit code 1 (user error)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C004
#[test]
fn t_c004_fetch_invalid_url_exits_1() {
    let output = scout()
        .args(["fetch", "not-a-valid-url"])
        .output()
        .expect("scout fetch failed to run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "invalid URL should be exit code 1 (user error)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}

// T-C005
#[test]
fn t_c005_repo_tree_bad_format_exits_1() {
    let output = scout()
        .args(["repo-tree", "no-slash-here"])
        .output()
        .expect("scout repo-tree failed to run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "malformed owner/repo should be exit code 1 (user error)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error:"),
        "stderr should contain error: prefix, got:\n{stderr}"
    );
}
