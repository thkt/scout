use std::time::Duration;

use super::*;
use crate::envelope::ErrorCode;
use crate::test_support::{
    mount_get, mount_users_info_resolving, spawn_mid_stream_drop_server, try_spawn_mock_server,
};
use crate::tools::ScoutError;
use reqwest::Client;
use reqwest::redirect::Policy;
use tracing_test::traced_test;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, ResponseTemplate};

/// A permalink in the fixed test workspace, with `raw_url` derived from `ts` the
/// way a real permalink is — so the two cannot be typed out of agreement.
fn slack_url(ts: &str, thread_ts: Option<&str>) -> SlackUrl {
    let p = ts.replace('.', "");
    SlackUrl {
        workspace: "acme".into(),
        channel: "C1".into(),
        ts: ts.into(),
        thread_ts: thread_ts.map(Into::into),
        raw_url: format!("https://acme.slack.com/archives/C1/p{p}"),
    }
}

/// [T-SK001]
#[tokio::test]
async fn api_get_once_429_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    mount_get(&server, "/test.method", ResponseTemplate::new(429)).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    assert!(matches!(
        result,
        Err(SlackError::RateLimited { retry_after: None })
    ));
}

/// [T-SK068] a 5xx gateway page is a transient server error, not a decode fault
///
/// Only 429 was branched on, so an HTML error page from a proxy or gateway
/// reached the JSON parse and surfaced as `Decode` — Internal (70), never
/// retried. The peer backends both retry the same condition.
#[tokio::test]
async fn api_get_once_502_returns_a_retriable_server_error() {
    let Some(server) = try_spawn_mock_server("slack::http_502").await else {
        return;
    };
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(502).set_body_string("<html><body>502 Bad Gateway</body></html>"),
    )
    .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    let err = result.expect_err("a 502 body is not a Slack API response");
    assert!(
        matches!(err, SlackError::Server(502)),
        "expected Server(502), got: {err:?}"
    );
    assert_eq!(
        err.classify().kind,
        ErrorCode::TempFailure,
        "a gateway failure is transient, not a scout-side bug"
    );
    assert!(
        is_retriable(&err),
        "a transient server error must reach the retry loop"
    );
}

/// [T-SK002] HTTP 429 with Retry-After header preserves header value
#[tokio::test]
async fn api_get_once_429_with_retry_after_header() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(429).append_header("Retry-After", "30"),
    )
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

/// [T-SK003]
#[tokio::test]
async fn api_get_once_body_ratelimited_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"ok": false, "error": "ratelimited"})),
    )
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
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"ok": false, "error": "channel_not_found"})),
    )
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
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": false})),
    )
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
    let body = vec![b'x'; MAX_API_RESPONSE_BYTES + 1];
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
    mount_get(
        &server,
        "/conversations.info",
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"ok": true, "channel": {"id": "C123"}})),
    )
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
    mount_get(
        &server,
        "/users.info",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
    )
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
    let url = slack_url(target_ts, Some(parent_ts));
    let out = client
        .fetch_message(&url)
        .await
        .expect("target message on page 2 is found via pagination")
        .markdown;
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
    mount_get(
        &server,
        "/conversations.history",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"text": mentions, "ts": "1000.000001"}]
        })),
    )
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
    let url = slack_url("1000.000001", None);
    let outcome = client
        .fetch_message(&url)
        .await
        .expect("mass-mention message resolves");
    assert!(
        outcome.users_capped,
        "distinct user IDs exceed the cap, so users_capped must be set"
    );
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
    mount_get(
        &server,
        "/conversations.history",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "UAUTHOR0", "text": "parent", "ts": parent_ts, "reply_count": 2}]
        })),
    )
    .await;
    // Replies carry three distinct authors; the mass mention lives in one reply.
    mount_get(
        &server,
        "/conversations.replies",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "UAUTHOR0", "text": "parent", "ts": parent_ts},
                {"user": "UAUTHOR1", "text": mentions, "ts": "1000.000002"},
                {"user": "UAUTHOR2", "text": "carol reply", "ts": "1000.000003"}
            ],
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })),
    )
    .await;
    // Any looked-up user resolves to a name, so a resolved author cannot leave
    // its raw ID in the output.
    mount_users_info_resolving(&server).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = slack_url(parent_ts, Some(parent_ts));
    let out = client
        .fetch_message(&url)
        .await
        .expect("thread with capped lookups resolves")
        .markdown;
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
    mount_get(
        &server,
        "/conversations.history",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "PARENT_BODY", "ts": parent_ts, "reply_count": 2}]
        })),
    )
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
    mount_users_info_resolving(&server).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = slack_url(parent_ts, Some(parent_ts));
    let outcome = client
        .fetch_message(&url)
        .await
        .expect("thread spanning two pages resolves");
    assert!(
        !outcome.thread_truncated,
        "a thread whose last page advertises no next cursor ends naturally and must not be flagged truncated (page-cap false-positive guard, symmetric to the user-cap boundary in T-SK055)"
    );
    let out = outcome.markdown;
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

/// [T-SK046] A thread whose reply pages never stop (every page returns
/// has_more:true + a cursor) stops paginating at SLACK_MAX_REPLY_PAGES and
/// emits a WARN that the thread was truncated, rather than looping forever
/// (issue #188 claim 2, cap path). Covers the post-loop cap-hit branch in
/// fetch_replies. The parent recurs on every page and is deduped by ts, so the
/// returned thread is the parent alone and fetch_message still succeeds.
#[tokio::test]
#[traced_test]
async fn fetch_replies_stops_at_page_cap_and_warns() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000001";
    // Every page advertises another page, so the loop only ends at the cap.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "PARENT_BODY", "ts": parent_ts}],
            "has_more": true,
            "response_metadata": {"next_cursor": "MORE"}
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = slack_url(parent_ts, Some(parent_ts));
    let outcome = client
        .fetch_message(&url)
        .await
        .expect("a truncated thread still returns the pages fetched so far");
    assert!(
        outcome.thread_truncated,
        "the reply page cap was hit, so thread_truncated must be set"
    );
    let out = outcome.markdown;
    assert!(
        out.contains("PARENT_BODY"),
        "expected the parent body in the truncated output, got: {out}"
    );
    assert!(
        logs_contain("hit the page cap"),
        "expected a WARN that the thread was truncated at the page cap"
    );
    assert!(logs_contain("WARN"));
}

/// [T-SK047] A message-permalink URL (no thread_ts) whose target has replies
/// triggers the conversations.history reply_count probe; when reply_count > 0
/// fetch_thread re-fetches the full thread via conversations.replies and marks
/// the result as a thread, so the replies render (issue #188 claim 2, probe
/// path). Covers the has_replies branch in fetch_thread. Without it a permalink
/// to a thread root would show only the root with no replies.
#[tokio::test]
async fn fetch_message_link_with_replies_fetches_thread() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000001";
    // The probe reports the target has one reply, so fetch_replies runs next.
    mount_get(
        &server,
        "/conversations.history",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "parent", "ts": parent_ts, "reply_count": 1}]
        })),
    )
    .await;
    mount_get(
        &server,
        "/conversations.replies",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "U1", "text": "parent", "ts": parent_ts},
                {"user": "U2", "text": "a reply", "ts": "1000.000002"}
            ],
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })),
    )
    .await;
    mount_users_info_resolving(&server).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = slack_url(parent_ts, None);
    let out = client
        .fetch_message(&url)
        .await
        .expect("a permalink to a thread root resolves the full thread")
        .markdown;
    assert!(
        out.contains("a reply"),
        "expected the probed thread's reply in output, got: {out}"
    );
}

/// [T-SK048] When distinct authors exceed SLACK_MAX_USER_LOOKUPS, the keep set
/// is fixed to first-occurrence (thread chronological) order rather than
/// HashSet iteration order, so which authors resolve to names is reproducible
/// across runs. A thread carrying SLACK_MAX_USER_LOOKUPS + 10 distinct authors
/// keeps exactly the first 50 by message order; authors 50..59 are evicted and
/// render as raw IDs. T-SK044 has only 3 authors « cap, so it never exercises
/// this path (issue #221).
#[tokio::test]
async fn fetch_message_keeps_first_occurrence_authors_when_capping() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000000";
    let total = SLACK_MAX_USER_LOOKUPS + 10; // 60 distinct authors
    // Author i authors message i, in a fixed order. Zero-padded IDs (U000..U059)
    // avoid the substring trap where contains("U5") would match "U50".
    let messages = (0..total)
        .map(|i| {
            serde_json::json!({
                "user": format!("U{i:03}"),
                "text": "reply",
                "ts": format!("1000.{i:06}"),
            })
        })
        .collect::<Vec<_>>();
    mount_get(
        &server,
        "/conversations.replies",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": messages,
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })),
    )
    .await;
    mount_users_info_resolving(&server).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = slack_url(parent_ts, Some(parent_ts));
    let out = client
        .fetch_message(&url)
        .await
        .expect("thread with capped author lookups resolves")
        .markdown;
    for i in 0..SLACK_MAX_USER_LOOKUPS {
        let raw = format!("U{i:03}");
        assert!(
            !out.contains(&raw),
            "author {raw} is within the first 50 by order and must resolve, but its raw ID leaked: {out}"
        );
    }
    for i in SLACK_MAX_USER_LOOKUPS..total {
        let raw = format!("U{i:03}");
        assert!(
            out.contains(&raw),
            "evicted author {raw} must render as a raw ID, but it was absent: {out}"
        );
    }
}

/// [T-SK049] A user mentioned in an early message who authors a later message is
/// kept as an author, not demoted to an evictable mention. The keep set is built
/// in two passes — all authors first, then mentions — sharing one `seen` set, so
/// the author role wins a dual-role ID's single slot. A single interleaved pass
/// would record the early mention first and evict the late author, shrinking
/// effective author coverage (issue #221 implementation hazard).
#[tokio::test]
async fn fetch_message_keeps_dual_role_id_as_author_not_mention() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    let parent_ts = "1000.000000";
    // The parent mentions UMENT01 then UDUAL (in-text order). UDUAL authors a
    // later message, so under two-pass author-first priority it keeps a slot;
    // the pure mention UMENT01 loses the (zero) remaining slots and renders raw.
    let mut messages = vec![serde_json::json!({
        "user": "UA00",
        "text": "<@UMENT01> <@UDUAL>",
        "ts": parent_ts,
    })];
    // UA00 plus UA01.. give SLACK_MAX_USER_LOOKUPS - 1 distinct pure authors;
    // UDUAL as a late author makes exactly the cap, leaving no top-up slot for
    // mentions. Derived from the cap so the boundary holds if the cap changes.
    for i in 1..(SLACK_MAX_USER_LOOKUPS - 1) {
        messages.push(serde_json::json!({
            "user": format!("UA{i:02}"),
            "text": "reply",
            "ts": format!("1000.{i:06}"),
        }));
    }
    messages.push(serde_json::json!({
        "user": "UDUAL",
        "text": "dual-role author",
        "ts": "1000.000049",
    }));
    mount_get(
        &server,
        "/conversations.replies",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": messages,
            "has_more": false,
            "response_metadata": {"next_cursor": ""}
        })),
    )
    .await;
    mount_users_info_resolving(&server).await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let url = slack_url(parent_ts, Some(parent_ts));
    let out = client
        .fetch_message(&url)
        .await
        .expect("thread with a dual-role ID resolves")
        .markdown;
    assert!(
        !out.contains("UDUAL"),
        "the dual-role ID must stay kept as an author, but its raw ID leaked: {out}"
    );
    assert!(
        out.contains("UMENT01"),
        "the pure mention must be evicted past the full cap and render raw: {out}"
    );
}

/// [T-005] api error internal_error は一度再試行され 2 回目の成功レスポンスが返る
///
/// `internal_error` classifies as TempFailure (per `SlackError::classify`'s
/// `Api` string table), so `is_retriable` must derive from `classify().kind`
/// rather than a second, independently-maintained table that never listed the
/// `Api` variant at all.
#[tokio::test]
async fn api_error_internal_error_retries_once_then_succeeds() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/test.method"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": false, "error": "internal_error"})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
    )
    .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());
    let result: Result<DummyBody, _> = client.api_get("test.method", &[]).await;
    assert!(
        result.is_ok(),
        "internal_error should retry once and return the second call's success, got: {result:?}"
    );
}

/// [T-006] timeout でも transient でもない transport error は再試行されず 1 回で返る
///
/// A redirect loop is neither `is_timeout()` nor `is_transient_network()`,
/// so it classifies as Unknown — `Classification::from_reqwest`'s escape
/// hatch — and must not retry. The request count proves the retry loop
/// made exactly one attempt.
#[tokio::test]
async fn transport_error_neither_timeout_nor_transient_is_not_retried() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(302).insert_header("Location", "/test.method"),
    )
    .await;

    let redirect_limit_1 = Client::builder()
        .redirect(Policy::limited(1))
        .build()
        .expect("client builds");
    let client = SlackClient::with_base_url(redirect_limit_1, &server.uri());
    let result: Result<DummyBody, _> = client.api_get("test.method", &[]).await;
    let err = result.expect_err("a redirect loop must not resolve to a body");
    assert!(
        matches!(err, SlackError::Network(ref e) if e.is_redirect()),
        "expected a redirect-loop Network error, got: {err:?}"
    );
    assert_eq!(
        err.classify().kind,
        ErrorCode::Unknown,
        "a redirect loop is neither timeout nor transient"
    );
    let requests = server
        .received_requests()
        .await
        .expect("request recording is on by default");
    assert_eq!(
        requests.len(),
        2,
        "one attempt is initial + 1 followed redirect; a retry would raise this to 4"
    );
}

/// [T-007] 2xx の mid-stream body 切断は Network に落ち TempFailure に分類される
///
/// Mirrors `api_get_once_2xx_mid_stream_drop_returns_network` (T-SK030), and
/// additionally pins the classification: a transport-IO drop must land in
/// TempFailure (retriable), not Decode (schema drift, terminal).
#[tokio::test]
async fn mid_stream_body_drop_classifies_as_temp_failure() {
    let Some((url, _counter, handle)) = spawn_mid_stream_drop_server(1) else {
        return;
    };
    let client = SlackClient::with_base_url(Client::new(), &url);
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    let err = result.expect_err("a mid-stream drop must not resolve to a body");
    assert!(
        matches!(err, SlackError::Network(_)),
        "expected SlackError::Network for mid-stream drop, got: {err:?}"
    );
    assert_eq!(
        err.classify().kind,
        ErrorCode::TempFailure,
        "a transport-IO drop is retriable, not a scout-side schema bug"
    );
    let _ = handle.join();
}

/// [T-009] SlackClient の read timeout は ScoutError 経由で exit code 124 になる
///
/// The seam from a real HTTP timeout through to the process exit code:
/// a `SlackClient::api_get_once` call whose request timeout fires must reach
/// `ScoutError` as exit 124 (GNU coreutils `timeout` convention, ADR-0002),
/// not the pre-fix TempFailure(75).
#[tokio::test]
async fn slack_client_read_timeout_reaches_exit_code_124_via_scout_error() {
    let Some(server) = try_spawn_mock_server("slack::http::read_timeout").await else {
        return;
    };
    mount_get(
        &server,
        "/test.method",
        ResponseTemplate::new(200).set_delay(Duration::from_secs(2)),
    )
    .await;

    let http = Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .expect("client build");
    let client = SlackClient::with_base_url(http, &server.uri());
    let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
    let err = result.expect_err("a response slower than the client timeout must fail");
    assert!(
        matches!(err, SlackError::Network(ref e) if e.is_timeout()),
        "expected a timeout Network error, got: {err:?}"
    );

    let scout_err = ScoutError::from(err);
    assert_eq!(
        scout_err.exit_code(),
        124,
        "expected 124 (GNU timeout): {scout_err}"
    );
    assert!(
        scout_err.retryable(),
        "Timeout must be retryable: {scout_err}"
    );
}
