/// [T-SR001]
#[test]
fn stdin_resolver_first_consumes_second_uses_arg() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    let first = r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
    assert_eq!(first, "from_stdin");
    let second = r
        .resolve(Some("test.txt".into()), "path", "<FILE_PATH>")
        .unwrap();
    assert_eq!(second, "test.txt");
}

/// [T-SR002] StdinResolver: arg wins over stdin, stdin preserved for next resolve
#[test]
fn stdin_resolver_arg_wins_stdin_preserved() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    let first = r
        .resolve(Some("owner/repo".into()), "repository", "<OWNER/REPO>")
        .unwrap();
    assert_eq!(first, "owner/repo");
    let second = r.resolve(None, "path", "<FILE_PATH>").unwrap();
    assert_eq!(second, "from_stdin");
}

/// [T-SR003]
#[test]
fn stdin_resolver_consumed_stdin_fails_second() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
    let result = r.resolve(None, "path", "<FILE_PATH>");
    assert!(
        result.is_err(),
        "second positional should fail when stdin consumed"
    );
}

/// [T-SR005] StdinResolver: error message hints stdin was consumed, not missing
#[test]
fn stdin_resolver_consumed_error_hints_stdin_exhausted() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
    let err = r
        .resolve(None, "path", "<FILE_PATH>")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("stdin was already read"),
        "error should hint stdin was consumed, got: {err}"
    );
    assert!(
        !err.contains("pipe it via stdin"),
        "error should not suggest piping when stdin is exhausted, got: {err}"
    );
}

/// [T-SR004]
#[test]
fn stdin_resolver_both_args_stdin_unused() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    let first = r
        .resolve(Some("owner/repo".into()), "repository", "<OWNER/REPO>")
        .unwrap();
    let second = r
        .resolve(Some("test.txt".into()), "path", "<FILE_PATH>")
        .unwrap();
    assert_eq!(first, "owner/repo");
    assert_eq!(second, "test.txt");
}
