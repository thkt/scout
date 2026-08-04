use super::*;
use crate::test_support::spawn_close_delimited_body_server;

/// [T-R013] Issue #219: every existing `too_large` test drives wiremock's
/// `set_body_bytes`, which always emits an honest Content-Length, so they
/// exercise only the pre-check path (body_limit.rs:40-45). The chunk loop
/// (body_limit.rs:51-55) is the defense-in-depth guard for upstreams that omit
/// Content-Length (compression → `content_length() == None`, chunked or
/// close-delimited transfer); before this test it had zero coverage. A
/// close-delimited response (no Content-Length, EOF-terminated body) forces
/// `content_length() == None`, so the chunk loop — not the pre-check — must
/// be what rejects the oversized body. CAP is tiny to keep the transfer
/// cheap; the branch under test is identical regardless of cap size.
#[tokio::test]
async fn read_body_capped_rejects_close_delimited_oversized_body() {
    const CAP: usize = 16;
    let Some((url, handle)) = spawn_close_delimited_body_server(CAP + 1) else {
        return; // loopback bind unavailable — skip
    };
    let resp = reqwest::Client::new().get(&url).send().await.expect("send");

    // Pin the chunk-loop path: a present Content-Length would route through
    // the pre-check instead, silently re-covering the existing gap.
    assert!(
        resp.content_length().is_none(),
        "close-delimited response must have no Content-Length so the chunk \
         loop, not the pre-check, is the cap guard under test"
    );

    // Distinct sentinels so a transport/framing fault surfaces as `network`
    // rather than masquerading as the `too_large` we want to assert.
    let result: Result<Vec<u8>, &str> =
        read_body_capped(resp, CAP, || "too_large", |_e| "network").await;

    assert_eq!(
        result,
        Err("too_large"),
        "body exceeding cap with absent Content-Length must be rejected by \
         the chunk loop"
    );

    let _ = handle.join();
}
