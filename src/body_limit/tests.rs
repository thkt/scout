use super::*;
use crate::test_support::{
    spawn_close_delimited_body_server, spawn_declared_length_no_body_server, try_spawn_mock_server,
};
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

/// [T-R013] Issue #219: every existing `too_large` test drives wiremock's
/// `set_body_bytes`, which always emits an honest Content-Length, so they
/// exercise only `read_body_capped`'s Content-Length pre-check. Its chunk
/// loop, the `body.len() > cap` arm, is the defense-in-depth guard for upstreams that omit
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

/// [T-001] Content-Length が cap を超えるヘッダのみのレスポンスは body を読まずに too_large になる
///
/// (TC-006 gap) The server writes only a `Content-Length: cap+1` header,
/// then closes without a single body byte. If `read_body_capped` reached the
/// chunk loop before rejecting, the premature close would surface as a
/// `network` error (declared length vs. zero actual bytes mismatch), not
/// `too_large`. Observing `too_large` is therefore direct evidence the
/// pre-check rejected before any `chunk()` call — the body was never read.
#[tokio::test]
async fn content_length_over_cap_with_no_body_rejects_too_large_without_reading_body() {
    const CAP: usize = 16;
    let Some((url, handle)) = spawn_declared_length_no_body_server(CAP + 1) else {
        return; // loopback bind unavailable — skip
    };
    let resp = reqwest::Client::new().get(&url).send().await.expect("send");

    assert_eq!(
        resp.content_length(),
        Some((CAP + 1) as u64),
        "server must declare the oversized Content-Length for the pre-check \
         to see"
    );

    let result: Result<Vec<u8>, &str> =
        read_body_capped(resp, CAP, || "too_large", |_e| "network").await;

    assert_eq!(
        result,
        Err("too_large"),
        "an oversized declared Content-Length must be rejected before any \
         body byte is read"
    );

    let _ = handle.join();
}

/// [T-002] ちょうど cap バイトの body は全量が返る
#[tokio::test]
async fn body_of_exactly_cap_bytes_returns_in_full() {
    const CAP: usize = 16;
    let Some(server) = try_spawn_mock_server("body_limit::exact_cap").await else {
        return; // loopback bind unavailable — skip
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; CAP]))
        .mount(&server)
        .await;

    let resp = reqwest::Client::new()
        .get(server.uri())
        .send()
        .await
        .expect("send");

    let result: Result<Vec<u8>, &str> =
        read_body_capped(resp, CAP, || "too_large", |_e| "network").await;

    assert_eq!(
        result,
        Ok(vec![b'x'; CAP]),
        "a body exactly at cap must be returned in full, not rejected"
    );
}
