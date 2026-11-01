use super::*;
use crate::envelope::{DegradedReason, ErrorCode};
use crate::test_support::{mount_users_info_resolving, try_spawn_mock_server};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Serves `conversations.info` so `resolve_channel` succeeds. Every test that
/// asserts on `degraded_reasons` or on the preamble needs it: without the
/// mount wiremock answers 404, the channel renders raw, and
/// `SLACK_LOOKUP_FAILED` adds a reason and a note the test is not about.
async fn mount_channel_info(server: &wiremock::MockServer) {
    Mock::given(method("GET"))
        .and(path("/conversations.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "channel": {"id": "C1", "name": "general"}
        })))
        .mount(server)
        .await;
}

/// [T-SK050] A thread whose reply pages never stop hits the page cap, so
/// `fetch_slack` wires the truncation into the ADR-0003 channel: `degraded` is
/// set, `degraded_reasons` carries `SLACK_THREAD_TRUNCATED`, and the Markdown
/// body gains a cap note in the frontmatter preamble. Without all three, a
/// truncated thread is indistinguishable from a complete one. The target ts is
/// on page 1 so the message resolves; only the reply tail past the cap is lost.
#[tokio::test]
async fn fetch_slack_thread_page_cap_sets_degraded_reason_and_preamble() {
    let Some(server) = try_spawn_mock_server("tools::slack_thread_cap").await else {
        return;
    };
    let parent_ts = "1000.000001";
    // Every page advertises another, so the loop only ends at the page cap.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "parent body", "ts": parent_ts}],
            "has_more": true,
            "response_metadata": {"next_cursor": "MORE"}
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001?thread_ts=1000.000001",
        ))
        .await
        .expect("a truncated thread still resolves the target on page 1");

    assert!(
        output
            .degraded_reasons()
            .contains(&DegradedReason::SlackThreadTruncated),
        "page-cap truncation must surface SLACK_THREAD_TRUNCATED, got: {:?}",
        output.degraded_reasons()
    );
    assert!(
        output.markdown().contains("> Note:") && output.markdown().contains("Thread truncated"),
        "the cap note must appear in the Markdown preamble, got: {}",
        output.markdown()
    );
}

/// [T-SK051] A message mentioning more distinct users than the lookup cap (50)
/// surfaces `SLACK_USERS_CAPPED` in `degraded_reasons` and a preamble note, so a
/// caller can tell that some `<@UID>` mentions render raw rather than resolved.
#[tokio::test]
async fn fetch_slack_users_cap_sets_degraded_reason_and_preamble() {
    let Some(server) = try_spawn_mock_server("tools::slack_users_cap").await else {
        return;
    };
    // 60 distinct mentions > SLACK_MAX_USER_LOOKUPS (50), so the lookup is capped.
    let mentions = (0..60)
        .map(|i| format!("<@U{i}>"))
        .collect::<Vec<_>>()
        .join(" ");
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"text": mentions, "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;

    mount_channel_info(&server).await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect("a mass-mention message resolves");

    assert!(
        output
            .degraded_reasons()
            .contains(&DegradedReason::SlackUsersCapped),
        "exceeding the user-lookup cap must surface SLACK_USERS_CAPPED, got: {:?}",
        output.degraded_reasons()
    );
    assert!(
        output.markdown().contains("> Note:") && output.markdown().contains("User lookups"),
        "the user-cap note must appear in the Markdown preamble, got: {}",
        output.markdown()
    );
}

/// [T-SK075] A fetch whose users.info lookup failed carries SlackLookupFailed in degraded_reasons
///
/// A single distinct author ID triggers exactly one `users.info` lookup; that
/// count stays far under `SLACK_MAX_USER_LOOKUPS`, so `SLACK_USERS_CAPPED`
/// cannot fire here. When the lookup itself fails (a transport/API error, not
/// the cap), the AI caller needs a distinct signal to tell "resolution
/// failed" apart from "capped by volume" — `DegradedReason::SlackLookupFailed`
/// must appear in `degraded_reasons`. `mount_channel_info` keeps the channel
/// lookup succeeding, so the reason can only come from `users.info`.
#[tokio::test]
async fn fetch_slack_users_info_failure_sets_lookup_failed_reason() {
    let Some(server) = try_spawn_mock_server("tools::slack_users_lookup_failed").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "hello", "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    mount_channel_info(&server).await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect("a failing users.info lookup still falls back to the raw author ID");

    assert!(
        output
            .degraded_reasons()
            .contains(&DegradedReason::SlackLookupFailed),
        "a failed users.info lookup must surface SLACK_LOOKUP_FAILED, got: {:?}",
        output.degraded_reasons()
    );
}

/// [T-SK076] A fetch whose users.info lookup succeeded carries no SlackLookupFailed in
/// degraded_reasons
///
/// Without this negative case, T-SK075 alone cannot rule out
/// `SlackLookupFailed` firing on every fetch regardless of lookup outcome.
#[tokio::test]
async fn fetch_slack_users_info_success_omits_lookup_failed_reason() {
    let Some(server) = try_spawn_mock_server("tools::slack_users_lookup_ok").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": "hello", "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;
    mount_channel_info(&server).await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect("a successful users.info lookup resolves fetch");

    assert!(
        !output
            .degraded_reasons()
            .contains(&DegradedReason::SlackLookupFailed),
        "a successful users.info lookup must not surface SLACK_LOOKUP_FAILED, got: {:?}",
        output.degraded_reasons()
    );
}

/// [T-SK052] A message body over `MAX_FETCH_OUTPUT_BYTES` (100KB) is truncated,
/// and `fetch_slack` reports it via `SLACK_OUTPUT_TRUNCATED` plus the inline
/// byte-count note appended at the body end. A cut
/// without the note is a silent one.
#[tokio::test]
async fn fetch_slack_output_truncation_sets_degraded_reason() {
    let Some(server) = try_spawn_mock_server("tools::slack_output_trunc").await else {
        return;
    };
    let huge = "x".repeat(150_000);
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"text": huge, "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;

    mount_channel_info(&server).await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect("an oversized message still resolves, truncated");

    assert!(
        output
            .degraded_reasons()
            .contains(&DegradedReason::SlackOutputTruncated),
        "exceeding the output cap must surface SLACK_OUTPUT_TRUNCATED, got: {:?}",
        output.degraded_reasons()
    );
    assert!(
        output.markdown().contains("(truncated: showing"),
        "the inline truncation note must remain at the body end, got len {}",
        output.markdown().len()
    );
}

/// [T-SK053] When a thread is page-capped AND its body exceeds the output cap,
/// the thread note must still reach the agent. The preamble is inserted after
/// truncation, so the 100KB cut cannot drop it. Both
/// `SLACK_THREAD_TRUNCATED` and `SLACK_OUTPUT_TRUNCATED` are reported, and the
/// thread note survives in the Markdown alongside the inline truncation note.
#[tokio::test]
async fn fetch_slack_thread_cap_note_survives_output_truncation() {
    let Some(server) = try_spawn_mock_server("tools::slack_combo_cap").await else {
        return;
    };
    let parent_ts = "1000.000001";
    let huge = format!("```rust\n{}\n```", "let x = 1;\n".repeat(15_000));
    // Every page advertises another (page cap) and the parent body is oversized
    // (output cap), so both degradations fire on one fetch.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"user": "U1", "text": huge, "ts": parent_ts}],
            "has_more": true,
            "response_metadata": {"next_cursor": "MORE"}
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;

    mount_channel_info(&server).await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001?thread_ts=1000.000001",
        ))
        .await
        .expect("an oversized truncated thread still resolves");

    let mut in_code = false;
    for event in Parser::new(output.markdown()) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(text)
                if text.contains("Thread truncated") || text.contains("(truncated:") =>
            {
                assert!(!in_code, "generated notes must remain outside code");
            }
            _ => {}
        }
    }
    assert!(output.markdown().contains("(truncated: showing"));
    assert!(output.markdown().contains("let x = 1;"));
    assert_eq!(output.data()["markdown"], output.markdown());
    let reasons = output.degraded_reasons();
    assert!(
        reasons.contains(&DegradedReason::SlackThreadTruncated)
            && reasons.contains(&DegradedReason::SlackOutputTruncated),
        "both the thread and output caps must be reported, got: {reasons:?}"
    );
    assert!(
        output.markdown().contains("Thread truncated"),
        "the thread note must survive the output truncation in the preamble, got len {}",
        output.markdown().len()
    );
}

/// [T-SK054] A normal thread with no caps keeps `degraded: false`: no reason is
/// emitted and no `> Note:` preamble is injected, so the wiring does not flag
/// healthy fetches.
#[tokio::test]
async fn fetch_slack_without_caps_stays_undegraded() {
    let Some(server) = try_spawn_mock_server("tools::slack_no_cap").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"text": "a short healthy message", "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;
    mount_channel_info(&server).await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect("a healthy message resolves");

    assert!(
        output.degraded_reasons().is_empty(),
        "a fetch with no caps must carry no degraded_reasons, got: {:?}",
        output.degraded_reasons()
    );
    assert!(
        !output.markdown().contains("> Note:"),
        "no cap note preamble should be injected, got: {}",
        output.markdown()
    );
}

/// [T-SK055] Distinct user IDs exactly equal to the lookup cap (50) do NOT
/// trigger `SLACK_USERS_CAPPED`: the condition is `> SLACK_MAX_USER_LOOKUPS`, so
/// the boundary value resolves fully and stays undegraded.
#[tokio::test]
async fn fetch_slack_users_at_cap_boundary_stays_undegraded() {
    let Some(server) = try_spawn_mock_server("tools::slack_users_boundary").await else {
        return;
    };
    // Exactly SLACK_MAX_USER_LOOKUPS (50) distinct mentions: == cap, not > cap.
    let mentions = (0..50)
        .map(|i| format!("<@U{i}>"))
        .collect::<Vec<_>>()
        .join(" ");
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [{"text": mentions, "ts": "1000.000001"}]
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;

    mount_channel_info(&server).await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect("a message mentioning exactly the cap resolves");

    assert!(
        !output
            .degraded_reasons()
            .contains(&DegradedReason::SlackUsersCapped),
        "distinct users == cap is within budget and must not degrade, got: {:?}",
        output.degraded_reasons()
    );
}

/// [T-SK056] A reply-bearing thread that hits both the page cap and the
/// user-lookup cap pins where the preamble lands and that multiple notes join
/// into one blockquote. `insert_preamble_notes` must insert after the FIRST
/// `---\n\n` (the frontmatter terminator) and never at a reply separator
/// (`\n\n---\n\n`); the other cap tests dedup replies to the parent alone, so no
/// reply separator ever appears and that placement rule goes unverified. Here a
/// distinct reply survives, so the note must sit after the frontmatter and
/// before the first reply separator, and the join must carry both the thread and
/// users notes (the only path that produces a 2-note preamble).
#[tokio::test]
async fn fetch_slack_thread_and_users_caps_place_joined_preamble_after_frontmatter() {
    let Some(server) = try_spawn_mock_server("tools::slack_thread_users_cap").await else {
        return;
    };
    let parent_ts = "1000.000001";
    // 60 distinct mentions (> SLACK_MAX_USER_LOOKUPS 50) in the parent body fire
    // the user cap; U100.. avoids colliding with the U1/U2 authors.
    let mentions = (100..160)
        .map(|i| format!("<@U{i}>"))
        .collect::<Vec<_>>()
        .join(" ");
    // Every page advertises another (page cap) and carries a distinct reply that
    // dedups to one, so the rendered thread has a reply separator to place
    // against.
    Mock::given(method("GET"))
        .and(path("/conversations.replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"user": "U1", "text": mentions, "ts": parent_ts},
                {"user": "U2", "text": "a surviving reply", "ts": "1000.000002"}
            ],
            "has_more": true,
            "response_metadata": {"next_cursor": "MORE"}
        })))
        .mount(&server)
        .await;
    mount_users_info_resolving(&server).await;

    mount_channel_info(&server).await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001?thread_ts=1000.000001",
        ))
        .await
        .expect("a capped reply-bearing thread resolves the target on page 1");

    let reasons = output.degraded_reasons();
    assert!(
        reasons.contains(&DegradedReason::SlackThreadTruncated)
            && reasons.contains(&DegradedReason::SlackUsersCapped),
        "both the page cap and the user cap must be reported, got: {reasons:?}"
    );

    let md = output.markdown();
    let note_pos = md.find("> Note:").expect("a preamble note is present");
    let frontmatter_end = md
        .find("---\n\n")
        .expect("the frontmatter terminator is present");
    let reply_sep = md
        .find("\n\n---\n\n")
        .expect("a reply separator is present for the surviving reply");
    assert!(
        note_pos > frontmatter_end,
        "the note must sit after the frontmatter terminator, not before it: {md}"
    );
    assert!(
        note_pos < reply_sep,
        "the note must sit before the first reply separator, not at one: {md}"
    );
    let preamble = &md[note_pos..reply_sep];
    assert!(
        preamble.contains("Thread truncated") && preamble.contains("User lookups"),
        "both cap notes must join into one preamble blockquote: {preamble}"
    );
}

/// [T-SK072] Pins the payload rule stated on `SlackError::Timeout`
/// (src/slack.rs) for the `fetch_slack` call site, where the wrapper can double
/// the payload. The `fetch` side is pinned by `T-C024`
/// (tests/exit_code_contract.rs).
///
/// Driving a real timed-out call is what makes the assertion non-tautological:
/// a `SlackError::Timeout` built in-process would assert on a payload this test
/// wrote itself. `with_slack_timeout` cuts the wait to 1s from the production
/// 60s, and the mock delay only has to outlast it.
#[tokio::test]
async fn fetch_slack_timeout_message_states_the_timeout_once() {
    let Some(server) = try_spawn_mock_server("tools::slack_timeout").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(serde_json::json!({
                    "ok": true,
                    "messages": [{"text": "too slow to matter", "ts": "1000.000001"}]
                })),
        )
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .with_slack_timeout(Duration::from_secs(1))
        .build();

    let err = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001",
        ))
        .await
        .expect_err("a Slack response slower than the timeout must fail");

    let message = err.message();
    assert_eq!(
        err.error_kind(),
        ErrorCode::Timeout,
        "a slow Slack response must classify as Timeout, got: {message}"
    );
    assert_eq!(
        message.matches("timed out").count(),
        1,
        "error.message should state the timeout once, got: {message}"
    );
}
