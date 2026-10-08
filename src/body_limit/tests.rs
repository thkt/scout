use super::*;
use crate::test_support::no_redirect_client;
use crate::test_support::{
    join_server_thread, spawn_close_delimited_body_server, spawn_declared_length_no_body_server,
    try_spawn_mock_server,
};
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

/// [T-R013] Reject an oversized close-delimited body through the chunk loop.
/// No Content-Length means the header pre-check cannot satisfy the assertion.
/// A tiny cap exercises the same guard without a large transfer.
#[tokio::test]
async fn read_body_capped_rejects_close_delimited_oversized_body() {
    const CAP: usize = 16;
    let Some((url, handle)) = spawn_close_delimited_body_server(CAP + 1) else {
        return; // loopback bind unavailable — skip
    };
    let resp = reqwest::Client::new().get(&url).send().await.expect("send");

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

    join_server_thread(handle);
}

/// [T-BL001] Reject an oversized Content-Length before reading body bytes.
/// The header-only server closes early: reading would yield a network error,
/// not too_large. This distinguishes the pre-check from the chunk-loop guard.
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

    join_server_thread(handle);
}

/// [T-BL002] A body of exactly cap bytes returns in full
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

/// [T-R016] A close-delimited response returns only the diagnostic prefix.
/// This checks returned length, not bytes consumed; the uncapped-read lint
/// separately prevents replacing the bounded reader with Response::text().
#[tokio::test]
async fn read_body_snippet_stops_at_the_limit() {
    const LIMIT: usize = 32;
    let Some((url, handle)) = spawn_close_delimited_body_server(LIMIT * 100) else {
        return; // loopback bind unavailable — skip
    };

    let response = no_redirect_client()
        .get(&url)
        .send()
        .await
        .expect("request to the local server");
    let body = read_body_snippet(response, LIMIT)
        .await
        .expect("snippet read");

    assert_eq!(
        body.len(),
        LIMIT,
        "must return exactly the limit, not the whole body"
    );
    join_server_thread(handle);
}

/// [T-R017] a body shorter than the limit comes back whole
#[tokio::test]
async fn read_body_snippet_returns_a_short_body_intact() {
    let Some(server) = try_spawn_mock_server("body_limit::snippet_short").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400).set_body_string("nope"))
        .mount(&server)
        .await;

    let response = reqwest::get(server.uri()).await.expect("request");
    let body = read_body_snippet(response, 1024)
        .await
        .expect("snippet read");

    assert_eq!(body, b"nope");
}
