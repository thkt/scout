use super::*;

/// [T-F052] parse_ws_url_extracts_first_matching_line
#[tokio::test]
async fn parse_ws_url_extracts_first_matching_line() {
    let stderr = b"[chromium] starting up\n\
                       DevTools listening on ws://127.0.0.1:54321/devtools/browser/abc-123\n\
                       DevTools listening on ws://127.0.0.1:54321/devtools/browser/def-456\n";
    let url = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
        .await
        .expect("first match should win");
    assert_eq!(url, "ws://127.0.0.1:54321/devtools/browser/abc-123");
}

/// [T-F053] parse_ws_url_skips_unrelated_lines_until_match
#[tokio::test]
async fn parse_ws_url_skips_unrelated_lines_until_match() {
    let stderr = b"[8765:0x110000000] preference manifest unparseable\n\
                       [warn] hardware acceleration unavailable\n\
                       random listening on something else\n\
                       DevTools listening on ws://localhost:1234/devtools/browser/xyz\n";
    let url = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
        .await
        .expect("should match after unrelated prefix");
    assert_eq!(url, "ws://localhost:1234/devtools/browser/xyz");
}

/// [T-F054] parse_ws_url_eof_before_match_errors
#[tokio::test]
async fn parse_ws_url_eof_before_match_errors() {
    let stderr = b"chromium crashed before opening port\n";
    let err = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
        .await
        .expect_err("EOF without match must surface as error");
    let msg = err.to_string();
    assert!(
        msg.contains("chromium exited before announcing DevTools URL"),
        "expected EOF message, got: {msg}"
    );
}

/// [T-F055] parse_ws_url_rejects_non_browser_devtools_url
///
/// chromium also prints `DevTools listening on ws://.../page/<id>` for
/// per-page debuggers — we must only accept the browser-level URL.
#[tokio::test]
async fn parse_ws_url_rejects_non_browser_devtools_url() {
    let stderr = b"DevTools listening on ws://127.0.0.1:9999/devtools/page/something\n";
    let err = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
        .await
        .expect_err("page-level URL must not match");
    assert!(
        err.to_string()
            .contains("chromium exited before announcing DevTools URL")
    );
}
