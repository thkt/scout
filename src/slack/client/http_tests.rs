use super::*;
use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};
use reqwest::Client;
use tracing_test::traced_test;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
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

/// [T-SK040] conversations.info 200 with a null channel.name emits a WARN
/// before falling back to the raw channel ID. Without it the label-resolution
/// degradation is silent to operators (issue #188 claim 1). The Err branch
/// already warns; this covers the Ok-but-null path.
#[tokio::test]
#[traced_test]
async fn resolve_channel_null_name_warns_then_falls_back() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.info"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": true, "channel": {"id": "C123"}})),
        )
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let name = client.resolve_channel("C123").await;
    assert_eq!(name, "C123", "falls back to the raw channel ID");
    assert!(
        logs_contain("channel name missing"),
        "expected a warn for the null channel name"
    );
    assert!(logs_contain("WARN"));
}

/// [T-SK041] users.info 200 with a null user emits a WARN before falling back
/// to the raw user ID (issue #188 claim 1). Mirrors T-SK040 for the user path.
#[tokio::test]
#[traced_test]
async fn fetch_user_name_null_user_warns_then_falls_back() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let name = client.fetch_user_name("U123").await;
    assert_eq!(name, "U123", "falls back to the raw user ID");
    assert!(
        logs_contain("user name missing"),
        "expected a warn for the null user name"
    );
    assert!(logs_contain("WARN"));
}

/// [T-SK042] conversations.replies pagination: a thread whose target message
/// lands on the second page (page 1 returns has_more:true + next_cursor) is
/// still found. Before the pagination loop, serde dropped has_more/next_cursor
/// and only page 1 was fetched, so a >200-reply thread lost the target and
/// fetch_message returned "not found" (issue #188 claim 2).
#[tokio::test]
async fn fetch_replies_paginates_to_find_target_on_page_two() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000001";
    let target_ts = "1000.000500";
    // Page 1: parent + filler, has_more with a cursor. Target is NOT here.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .and(query_param_is_missing("cursor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "U1", "text": "parent", "ts": parent_ts},
                {"user": "U1", "text": "filler", "ts": "1000.000002"}
            ],
            "has_more": true,
            "response_metadata": {"next_cursor": "PAGE2"}
        })))
        .mount(&server)
        .await;
    // Page 2: contains the target message, no further pages.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .and(query_param("cursor", "PAGE2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "U1", "text": "the target reply", "ts": target_ts}
            ],
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = SlackUrl {
        workspace: "acme".into(),
        channel: "C1".into(),
        ts: target_ts.into(),
        thread_ts: Some(parent_ts.into()),
        raw_url: "https://acme.slack.com/archives/C1/p1000000500".into(),
    };
    let out = client
        .fetch_message(&url)
        .await
        .expect("target message on page 2 is found via pagination");
    assert!(
        out.contains("the target reply"),
        "expected the page-2 target in output, got: {out}"
    );
}
