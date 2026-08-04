use super::*;
use crate::clock::FixedClock;
use crate::envelope::{DegradedReason, ErrorCode};
use crate::fetch::{EgressMode, FailingDnsResolver, StaticDnsResolver};
use crate::rng::SeededRng;
use crate::test_support::{spawn_forward_proxy, try_spawn_mock_server};
use crate::token_source::StaticTokenSource;
use reqwest::Proxy;
use reqwest::redirect::Policy;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-SB001] `ScoutBuilder::with_clock` で渡した `Arc` が `Scout.clock` まで
/// 届く injection slot の最小証明。end-to-end な plumbing 確認は T-SB004。
#[test]
fn scout_builder_with_clock_routes_arc_into_scout() {
    let injected: Arc<dyn Clock> = Arc::new(FixedClock(42));
    let scout = ScoutBuilder::for_test()
        .with_clock(injected.clone())
        .build();
    assert!(
        Arc::ptr_eq(&scout.clock, &injected),
        "with_clock must install the supplied Arc into Scout.clock"
    );
}

/// [T-SB002] `ScoutBuilder::with_rng` で渡した `Arc` が `Scout.rng` まで
/// 届く injection slot の最小証明。
#[test]
fn scout_builder_with_rng_routes_arc_into_scout() {
    let injected: Arc<dyn Rng> = Arc::new(SeededRng::new(7));
    let scout = ScoutBuilder::for_test().with_rng(injected.clone()).build();
    assert!(
        Arc::ptr_eq(&scout.rng, &injected),
        "with_rng must install the supplied Arc into Scout.rng"
    );
}

/// [T-SB003] `ScoutBuilder::with_token_source` で渡した `Arc` が
/// `Scout.token_source` まで届く injection slot の最小証明。
#[test]
fn scout_builder_with_token_source_routes_arc_into_scout() {
    let injected: Arc<dyn TokenSource> = Arc::new(StaticTokenSource(None));
    let scout = ScoutBuilder::for_test()
        .with_token_source(injected.clone())
        .build();
    assert!(
        Arc::ptr_eq(&scout.token_source, &injected),
        "with_token_source must install the supplied Arc into Scout.token_source"
    );
}

/// [T-DNS001] `ScoutBuilder::with_dns` で渡した `Arc<dyn DnsResolver>` が
/// `Scout.dns` slot に届き、かつ `Scout::fetch` の SSRF 経路で実際に
/// consult されることを end-to-end で確認する。
///
/// 注入した `StaticDnsResolver(10.0.0.1)` が `https://example.com` の
/// DNS lookup を override すれば、`ssrf_check` の private-IP 判定が
/// `FetchError::InternalHost` を即座に返す。default の `TokioDnsResolver`
/// なら `example.com` は public IP を返すため、この assert は
/// injection が wire できていない場合に必ず落ちる。
#[tokio::test]
async fn scout_builder_with_dns_blocks_fetch_via_injected_private_ip() {
    let injected: Arc<dyn DnsResolver> = Arc::new(StaticDnsResolver::single("10.0.0.1"));
    let scout = ScoutBuilder::for_test().with_dns(injected.clone()).build();

    assert!(
        Arc::ptr_eq(&scout.dns, &injected),
        "with_dns must install the supplied Arc into Scout.dns"
    );

    let result = scout
        .fetch(FetchParams::for_test("https://example.com/page"))
        .await;
    let err = result.expect_err("injected private IP must trip SSRF check");
    assert_eq!(
        err.error_kind(),
        ErrorCode::DataError,
        "SSRF InternalHost maps to DataError (sysexits EX_DATAERR)"
    );
    assert!(
        err.message().contains("internal/private"),
        "error message must surface the SSRF cause, got: {}",
        err.message()
    );
}

/// [T-DNS002] `FailingDnsResolver` を inject すると `Scout::fetch` が
/// `FetchError::DnsResolution` 由来の `ScoutError` を返すことを確認する。
/// resolver の失敗パスが SSRF 経路に正しく伝播することを保証する。
#[tokio::test]
async fn scout_builder_with_dns_propagates_resolver_failure() {
    let injected: Arc<dyn DnsResolver> =
        Arc::new(FailingDnsResolver("simulated DNS failure".into()));
    let scout = ScoutBuilder::for_test().with_dns(injected).build();

    let result = scout
        .fetch(FetchParams::for_test("https://example.com/page"))
        .await;
    let err = result.expect_err("injected resolver failure must surface as error");
    assert!(
        err.message().contains("DNS resolution failed"),
        "error message must surface the DNS failure cause, got: {}",
        err.message()
    );
}

/// [T-SB004] `with_clock` で inject した `FixedClock` が `Scout::github()`
/// 経由で初期化される `GitHubClient` まで届くことを end-to-end で確認する。
/// `Arc::ptr_eq` 単体テスト (T-SB001) では `github()` の plumbing バグ
/// (例: clone 忘れ、async move への束縛漏れ) を catch できないので、
/// wiremock 越しに `secs_until_ratelimit_reset` の算出値を assert する。
///
/// reset = 1600, clock = 1000 → retry_after = 600 が `MAX_RETRY_AFTER_SECS`
/// (300) を超えるため `is_retriable = false` で retry loop はスキップ。
/// `start_paused` を併用すると wiremock の TCP listener も止まり connect が
/// timeout するので、retry を走らせない算術にする方が安定する。
#[tokio::test]
async fn scout_builder_clock_reaches_github_client_via_seam() {
    let Some(server) = try_spawn_mock_server("tools::scout_builder_seam").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(
            ResponseTemplate::new(403)
                .append_header("x-ratelimit-remaining", "0")
                .append_header("x-ratelimit-reset", "1600")
                .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
        )
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_clock(Arc::new(FixedClock(1000)))
        .with_github_endpoint(&server.uri())
        .build();

    let result = scout.github().await.get_repo("owner", "repo").await;
    assert!(
        matches!(
            result,
            Err(github::GitHubError::RateLimited {
                retry_after: Some(600)
            })
        ),
        "expected retry_after = 600 (reset 1600 - clock 1000), got: {result:?}"
    );
}

/// [T-SB005] `with_slack_endpoint` で inject した wiremock endpoint が
/// `fetch`(slack permalink) → `fetch_slack` → `slack()` の production 経路で
/// 構築される `SlackClient` まで届くことを end-to-end で確認する。`slack()` の
/// `OnceCell` を build() で pre-set するため `SLACK_TOKEN` 未設定でも注入
/// クライアントが使われ、`conversations.history` に到達して本文を取得できる。
/// 注入が wire できていなければ `from_env` が `TokenNotSet` を返し落ちる
/// (issue #191)。
#[tokio::test]
async fn scout_builder_slack_endpoint_reaches_fetch_slack_via_seam() {
    let Some(server) = try_spawn_mock_server("tools::scout_builder_slack_seam").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"type": "message", "text": "hello from wiremock", "ts": "1773819598.273499"}
            ]
        })))
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();

    let result = scout
        .fetch(FetchParams::for_test(
            "https://team.slack.com/archives/C123/p1773819598273499",
        ))
        .await;
    let output =
        result.expect("injected slack endpoint must serve fetch_slack without SLACK_TOKEN");
    assert!(
        output.markdown().contains("hello from wiremock"),
        "fetch output must carry the wiremock message body, got: {}",
        output.markdown()
    );
}

/// [T-SK050] A thread whose reply pages never stop hits the page cap, so
/// `fetch_slack` wires the truncation into the ADR-0003 channel: `degraded` is
/// set, `degraded_reasons` carries `SLACK_THREAD_TRUNCATED`, and the Markdown
/// body gains a cap note in the frontmatter preamble (issue #222). Before this,
/// a truncated Slack thread returned `degraded: false` and the omission was
/// invisible to callers. The target ts is on page 1 so the message resolves;
/// only the reply tail past the cap is lost.
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
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

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
/// caller can tell that some `<@UID>` mentions render raw rather than resolved
/// (issue #222).
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
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

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

/// [T-SK052] A message body over `MAX_FETCH_OUTPUT_BYTES` (100KB) is truncated,
/// and `fetch_slack` reports it via `SLACK_OUTPUT_TRUNCATED` plus the inline
/// byte-count note that `truncate_with_note` appends at the body end (issue
/// #222). The non-Slack `fetch` path already cut output silently for Slack.
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
/// truncation, so the 100KB cut cannot drop it (issue #222 Finding A). Both
/// `SLACK_THREAD_TRUNCATED` and `SLACK_OUTPUT_TRUNCATED` are reported, and the
/// thread note survives in the Markdown alongside the inline truncation note.
#[tokio::test]
async fn fetch_slack_thread_cap_note_survives_output_truncation() {
    let Some(server) = try_spawn_mock_server("tools::slack_combo_cap").await else {
        return;
    };
    let parent_ts = "1000.000001";
    let huge = "x".repeat(150_000);
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
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();
    let output = scout
        .fetch(FetchParams::for_test(
            "https://acme.slack.com/archives/C1/p1000000001?thread_ts=1000.000001",
        ))
        .await
        .expect("an oversized truncated thread still resolves");

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
/// healthy fetches (issue #222 negative case).
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
/// the boundary value resolves fully and stays undegraded (issue #222 boundary).
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
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

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
/// users notes (the only path that produces a 2-note preamble) (issue #222).
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
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })))
        .mount(&server)
        .await;

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

/// [T-SB006] `with_egress(Proxied)` drives the `Scout::fetch` → `fetch_page`
/// egress plumbing end-to-end: a public-domain fetch routes through a local
/// forward proxy and returns the body, while the injected `FailingDnsResolver`
/// is never consulted (Proxied skips scout's DNS pre-check). This exercises the
/// `with_egress` seam and proves `query.rs` forwards `Scout.egress` into
/// `FetchOptions.egress`. It complements the fetch_page-level T-010/T-011: those
/// pin the proxied client shape, this pins the builder → query wiring that
/// produces it. A regression to `Direct` would consult the resolver and surface
/// as a `DnsResolution` error instead of the body.
#[tokio::test]
async fn scout_builder_with_egress_routes_proxied_fetch_through_proxy() {
    // Rich body (no <script>, >100 visible chars) so `is_js_dependent` /
    // `is_thin_extract` stay false and the CDP fallback never fires.
    let body = "<html><body><h1>Proxied Article</h1><p>proxied body content long \
        enough to clear the thin-body and thin-extract thresholds so the JS \
        rendering fallback path is never taken in this proxied fetch test.</p>\
        </body></html>";
    let Some((proxy_url, handle)) = spawn_forward_proxy(body) else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    // Mirror what production `build_default_clients` builds for Proxied: a
    // proxied, guard-free client (no `SsrfResolver`, which by design blocks the
    // loopback proxy). `for_test`'s default `fetch_http` is the Direct
    // guard-carrying client, so it must be replaced via `with_fetch_http`.
    let fetch_http = reqwest::Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();
    let scout = ScoutBuilder::for_test()
        .with_fetch_http(fetch_http)
        .with_egress(EgressMode::Proxied(proxy_url.clone()))
        .with_dns(Arc::new(FailingDnsResolver(
            "resolver must not be consulted in Proxied mode".into(),
        )))
        .build();

    let result = scout
        .fetch(FetchParams::for_test("http://example.com/page"))
        .await;
    let _ = handle.join();
    let output = result.expect("proxied fetch of a public URL should succeed");
    assert!(
        output.markdown().contains("proxied body content"),
        "proxied fetch must return the page body via the proxy, got: {}",
        output.markdown()
    );
}

/// [T-SK072] The message a `fetch_slack` timeout produces states the timeout
/// once. `SlackError::Timeout`'s Display prefixes "Slack fetch timed out: " and
/// the `tokio::time::timeout` fallback in `fetch_slack` (src/tools/query.rs)
/// supplies the payload; while both carried the phrase, `error.message` read
/// "Slack fetch timed out: slack fetch timed out after 30s" (issue #313).
///
/// The assertion runs on a real timed-out call rather than a hand-built
/// `SlackError::Timeout`, so a call site that puts the phrase back fails here.
/// `with_slack_timeout` keeps the wait at 1s instead of the production 30s; the
/// delay only has to outlast it.
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

    assert_eq!(
        err.error_kind(),
        ErrorCode::Timeout,
        "a slow Slack response must classify as Timeout, got: {}",
        err.message()
    );
    assert_eq!(
        err.message().matches("timed out").count(),
        1,
        "error.message should state the timeout once, got: {}",
        err.message()
    );
}
