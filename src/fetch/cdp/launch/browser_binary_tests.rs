use super::*;
use std::env;

/// [T-F041]
#[test]
fn t001_returns_error_when_chrome_not_found() {
    let result = resolve_browser_binary_from(&[], &[]);
    assert!(
        matches!(result, Err(BrowserError::NotFound)),
        "expected NotFound, got: {result:?}"
    );
}

/// [T-F042]
#[test]
fn finds_binary_at_known_path() {
    let existing = env::current_exe().unwrap();
    let result = resolve_browser_binary_from(&[], &[existing.as_path()]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), existing);
}
