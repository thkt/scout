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

/// [T-SK043] A message mentioning far more distinct users than
/// SLACK_MAX_USER_LOOKUPS caps the number of users.info requests at the limit,
/// so a mass-mention message cannot exhaust Slack's per-minute rate budget.
/// Excess mentions degrade to raw IDs. The `.expect(cap)` on the users.info
/// mock fails on server drop if the cap is not enforced (issue #188 claim 3).
#[tokio::test]
async fn fetch_message_caps_users_info_lookups_on_mass_mentions() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let total = SLACK_MAX_USER_LOOKUPS + 50;
    let mentions = (0..total)
        .map(|i| format!("<@U{i}>"))
        .collect::<Vec<_>>()
        .join(" ");
    // Single message (no thread): conversations.history returns it directly.
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"text": mentions, "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;
    // users.info must be hit exactly SLACK_MAX_USER_LOOKUPS times, not `total`.
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .expect(SLACK_MAX_USER_LOOKUPS as u64)
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = SlackUrl {
        workspace: "acme".into(),
        channel: "C1".into(),
        ts: "1000.000001".into(),
        thread_ts: None,
        raw_url: "https://acme.slack.com/archives/C1/p1000000001".into(),
    };
    client
        .fetch_message(&url)
        .await
        .expect("mass-mention message resolves");
    // The `.expect(cap)` is verified when `server` drops at end of scope.
}

/// [T-SK044] When distinct IDs exceed SLACK_MAX_USER_LOOKUPS, message authors
/// are kept in the lookup set ahead of mentions. Authors render on every
/// message, so dropping one degrades visible output more than dropping a
/// mention. The earlier cap took an arbitrary HashSet slice, so authors could
/// be evicted nondeterministically (issue #188 audit RU7/SF-1). Here three
/// distinct authors compete with thousands of mentions for a 50-slot cap; with
/// author-first priority every author resolves to a name, so no raw "UAUTHOR"
/// ID leaks into the rendered output.
#[tokio::test]
async fn fetch_message_prioritizes_authors_over_mentions_when_capping() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000001";
    // Far more mentions than the cap, so an arbitrary slice would almost
    // certainly evict at least one of the three authors.
    let mentions = (0..3000)
        .map(|i| format!("<@U{i}>"))
        .collect::<Vec<_>>()
        .join(" ");
    // Thread probe: the root has replies, so fetch_replies is used.
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "UAUTHOR0", "text": "parent", "ts": parent_ts, "reply_count": 2}]
        })))
        .mount(&server)
        .await;
    // Replies carry three distinct authors; the mass mention lives in one reply.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "UAUTHOR0", "text": "parent", "ts": parent_ts},
                {"user": "UAUTHOR1", "text": mentions, "ts": "1000.000002"},
                {"user": "UAUTHOR2", "text": "carol reply", "ts": "1000.000003"}
            ],
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })))
        .mount(&server)
        .await;
    // Any looked-up user resolves to a name, so a resolved author cannot leave
    // its raw ID in the output.
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = SlackUrl {
        workspace: "acme".into(),
        channel: "C1".into(),
        ts: parent_ts.into(),
        thread_ts: Some(parent_ts.into()),
        raw_url: "https://acme.slack.com/archives/C1/p1000000001".into(),
    };
    let out = client
        .fetch_message(&url)
        .await
        .expect("thread with capped lookups resolves");
    assert!(
        !out.contains("UAUTHOR"),
        "every author should resolve to a name, but a raw author ID leaked: {out}"
    );
}

/// [T-SK045] conversations.replies repeats the thread parent as messages[0] on
/// every page. The pagination loop flat-extends pages, so a parent that recurs
/// across pages was counted once per page: extract_target removes only the
/// first copy, leaving the rest as duplicate replies that inflate
/// context_messages and re-render the parent body. Dedup by ts so the parent
/// appears once regardless of page count (issue #188 audit RU1/RU2).
#[tokio::test]
async fn fetch_replies_dedups_parent_repeated_across_pages() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000001";
    // Page 1: parent + first reply, more pages follow.
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "PARENT_BODY", "ts": parent_ts, "reply_count": 2}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .and(query_param_is_missing("cursor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "U1", "text": "PARENT_BODY", "ts": parent_ts},
                {"user": "U2", "text": "first reply", "ts": "1000.000002"}
            ],
            "has_more": true,
            "response_metadata": {"next_cursor": "PAGE2"}
        })))
        .mount(&server)
        .await;
    // Page 2: parent repeats as messages[0], plus the second reply.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .and(query_param("cursor", "PAGE2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "U1", "text": "PARENT_BODY", "ts": parent_ts},
                {"user": "U3", "text": "second reply", "ts": "1000.000003"}
            ],
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = SlackUrl {
        workspace: "acme".into(),
        channel: "C1".into(),
        ts: parent_ts.into(),
        thread_ts: Some(parent_ts.into()),
        raw_url: "https://acme.slack.com/archives/C1/p1000000001".into(),
    };
    let out = client
        .fetch_message(&url)
        .await
        .expect("thread spanning two pages resolves");
    assert!(
        out.contains("context_messages: 2"),
        "two distinct replies expected, parent duplicate must not inflate the count: {out}"
    );
    assert_eq!(
        out.matches("PARENT_BODY").count(),
        1,
        "the parent body should render once, not once per page: {out}"
    );
}
