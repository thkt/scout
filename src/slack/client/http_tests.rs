use super::*;
use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};
use reqwest::Client;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-SK001] HTTP 429 response maps to SlackError::RateLimited
#[tokio::test]
async fn api_get_once_429_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(matches!(
        result,
        Err(SlackError::RateLimited { retry_after: None })
    ));
}

/// [T-SK002] HTTP 429 with Retry-After header preserves header value
#[tokio::test]
async fn api_get_once_429_with_retry_after_header() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "30"))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(matches!(
        result,
        Err(SlackError::RateLimited {
            retry_after: Some(30)
        })
    ));
}

/// [T-SK003] Body-level ratelimited error maps to SlackError::RateLimited
#[tokio::test]
async fn api_get_once_body_ratelimited_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": false, "error": "ratelimited"})),
        )
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(matches!(result, Err(SlackError::RateLimited { .. })));
}

/// [T-SK004] Non-ratelimited Slack API error maps to SlackError::Api
#[tokio::test]
async fn api_get_once_api_error_returns_api_variant() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": false, "error": "channel_not_found"})),
        )
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(matches!(
        result,
        Err(SlackError::Api { error }) if error == "channel_not_found"
    ));
}

/// [T-SK031] ok:false without an `error` field surfaces as SlackError::Decode
/// (issue #114 condition 5). The previous code substituted the literal
/// "unknown" string and mapped to UsageError; a missing `error` is a
/// Slack API contract violation, not a user-fixable failure.
#[tokio::test]
async fn api_get_once_ok_false_without_error_field_returns_decode() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": false})))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(
        matches!(result, Err(SlackError::Decode(_))),
        "expected SlackError::Decode for ok:false without error field, got: {result:?}"
    );
}

/// [T-SK032] (issue #165 / CHX-009)
/// Setup: wiremock returns a 2xx whose body exceeds
/// `MAX_API_RESPONSE_BYTES` (1 MiB), simulating a runaway Slack
/// thread/channel response.
/// Action: `api_get_once::<DummyBody>("test.method", &[])` is invoked.
/// Expected: returns `SlackError::Decode` (terminal — Slack contract
/// violation, retry will not recover). Body message contains
/// "too large" to surface the cap in the user-facing error.
#[tokio::test]
async fn api_get_once_oversized_body_returns_decode() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let body = vec![b'x'; (1024 * 1024) + 1];
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    match result {
        Err(SlackError::Decode(msg)) => assert!(
            msg.contains("too large"),
            "expected size-cap message, got: {msg}"
        ),
        other => panic!("expected SlackError::Decode for oversized body, got: {other:?}"),
    }
}

/// [T-SK030] Mid-stream body drop on 2xx routes through SlackError::Network
/// (transient, retry path) rather than SlackError::Decode (terminal). reqwest
/// 0.13 reports the drop as `is_decode() == true`; `is_transient_decode`
/// distinguishes it from a schema fail via the io::Error source chain (issue #113).
#[tokio::test]
async fn api_get_once_2xx_mid_stream_drop_returns_network() {
    let Some((url, _counter, handle)) = spawn_mid_stream_drop_server(1) else {
        return;
    };
    let client = SlackClient::with_base_url(Client::new(), &url);
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(
        matches!(result, Err(SlackError::Network(_))),
        "expected SlackError::Network for mid-stream drop, got: {result:?}"
    );
    let _ = handle.join();
}
