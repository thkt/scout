use std::time::Duration;

use reqwest::redirect::Policy;
use serde_json::json;
use tokio::time::timeout;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, ResponseTemplate};

use super::*;
use crate::envelope::ErrorCode;
use crate::fetch::EgressMode;
use crate::search::Lang;
use crate::test_support::http_fixture::HttpFixture;
use crate::test_support::try_spawn_mock_server;

/// [T-HP001] A slow optional section cannot postpone fetching the README body.
#[tokio::test]
async fn overview_fetches_readme_blob_while_other_sections_are_pending() {
    let server = HttpFixture::start(&[
        (
            "/repos/owner/repo",
            json!({
                "full_name": "owner/repo", "description": null,
                "html_url": "https://github.com/owner/repo", "default_branch": "main",
                "language": "Rust", "stargazers_count": 1, "forks_count": 0,
                "open_issues_count": 0, "license": null
            }),
            &[],
        ),
        (
            "/repos/owner/repo/readme",
            json!({"sha": "readme", "content": null}),
            &[],
        ),
        (
            "/repos/owner/repo/git/blobs/readme",
            json!({"content": "UkVBRE1FIGJvZHk="}),
            &[],
        ),
        (
            "/repos/owner/repo/issues",
            json!([]),
            &["/repos/owner/repo/git/blobs/readme"],
        ),
        (
            "/repos/owner/repo/pulls",
            json!([]),
            &["/repos/owner/repo/git/blobs/readme"],
        ),
        (
            "/repos/owner/repo/releases",
            json!([]),
            &["/repos/owner/repo/git/blobs/readme"],
        ),
    ])
    .await;
    let scout = ScoutBuilder::for_test()
        .with_github_endpoint(&server.uri)
        .build();
    let result = timeout(
        Duration::from_secs(5),
        scout.run(Command::RepoOverview(RepoOverviewParams {
            repository: Some("owner/repo".into()),
        })),
    )
    .await
    .expect("README retrieval must overlap optional sections")
    .unwrap();
    assert!(result.markdown().contains("README body"));
    assert_eq!(result.data()["readme"], "README body");
    assert_eq!(
        serde_json::to_value(result.into_envelope()).unwrap()["degraded"],
        false
    );
}

/// [T-HP002] Channel lookup overlaps the complete message/user-resolution pipeline.
#[tokio::test]
async fn slack_fetch_overlaps_channel_and_message_resolution() {
    let server = HttpFixture::start(&[
        (
            "/conversations.history",
            json!({"ok": true, "messages": [
                {"user": "U1", "text": "Hello <@U2>", "ts": "1111111111.222222"}
            ]}),
            &["/conversations.info"],
        ),
        (
            "/conversations.info",
            json!({"ok": true, "channel": {"name": "general"}}),
            &["/users.info"],
        ),
        (
            "/users.info",
            json!({"ok": true, "user": {"real_name": "Alice"}}),
            &[],
        ),
    ])
    .await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri)
        .build();
    let result = timeout(
        Duration::from_secs(5),
        scout.run(Command::Fetch(FetchParams::for_test(
            "https://team.slack.com/archives/C123/p1111111111222222",
        ))),
    )
    .await
    .expect("channel lookup must overlap both history and user lookups")
    .unwrap();
    assert!(result.markdown().contains("channel: \"#general\""));
    assert!(result.markdown().contains("author: \"Alice\""));
    assert!(result.markdown().contains("Hello @Alice"));
    assert_eq!(
        serde_json::to_value(result.into_envelope()).unwrap()["degraded"],
        false
    );
}

/// [T-HP003] A missing message is NotFound even when name resolution would hang.
#[tokio::test]
async fn missing_slack_target_does_not_spend_its_budget_resolving_names() {
    let Some(server) = try_spawn_mock_server("hotpaths::missing_slack_target").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok": true, "messages": [
                {"user": "U1", "text": "Neighbouring message", "ts": "1111111111.000000"}
            ]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/conversations.info"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "channel": {"name": "general"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users.info"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .expect(0)
        .mount(&server)
        .await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .with_slack_timeout(Duration::from_millis(200))
        .build();
    let err = scout
        .run(Command::Fetch(FetchParams::for_test(
            "https://team.slack.com/archives/C123/p1111111111222222",
        )))
        .await
        .unwrap_err();
    assert_eq!(err.error_kind(), ErrorCode::NotFound);
    assert_eq!(err.exit_code(), 66);
    assert!(err.message().contains("1111111111.222222"));
}

/// [T-HP005] A reply permalink promotes its target and keeps context in source order.
#[tokio::test]
async fn slack_reply_permalink_preserves_target_and_context() {
    let server = HttpFixture::start(&[
        (
            "/conversations.replies",
            json!({"ok": true, "messages": [
                {"user": "U1", "text": "Parent body", "ts": "1111111111.000000"},
                {"user": "U1", "text": "Target body", "ts": "1111111111.222222"},
                {"user": "U1", "text": "Last body", "ts": "1111111111.333333"}
            ]}),
            &[],
        ),
        (
            "/conversations.info",
            json!({"ok": true, "channel": {"name": "general"}}),
            &[],
        ),
        (
            "/users.info",
            json!({"ok": true, "user": {"real_name": "Alice"}}),
            &[],
        ),
    ])
    .await;
    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri)
        .build();
    let result = scout
        .run(Command::Fetch(FetchParams::for_test(
            "https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1111111111.000000",
        )))
        .await
        .unwrap();
    assert_eq!(
        result.markdown(),
        "---\nworkspace: \"team\"\nchannel: \"#general\"\nauthor: \"Alice\"\nts: \"1111111111.222222\"\ncontext_messages: 2\nurl: \"https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1111111111.000000\"\n---\n\nTarget body\n\n---\n\nAlice (1111111111.000000):\nParent body\n\n---\n\nAlice (1111111111.333333):\nLast body\n"
    );
}

/// [T-HP006] Research respects depth and search rank despite out-of-order HTTP completion.
#[tokio::test]
async fn research_preserves_rank_and_depth_through_real_clients() {
    let Some(server) = try_spawn_mock_server("hotpaths::research_rank").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "Primary sources"))
        .and(query_param("search_lang", "en"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"web": {"results": [
                {"url": "http://articles.example/first", "title": "First"},
                {"url": "http://articles.example/failed-first", "title": "Missing first"},
                {"url": "http://articles.example/second", "title": "Second"},
                {"url": "http://articles.example/failed-second", "title": "Missing second"},
                {"url": "http://articles.example/not-requested", "title": "Outside depth"}
            ]}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    for (endpoint, response) in [
        (
            "/first",
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/plain")
                .set_body_string("First primary source")
                .set_delay(Duration::from_millis(100)),
        ),
        (
            "/failed-first",
            ResponseTemplate::new(404).set_delay(Duration::from_millis(100)),
        ),
        (
            "/second",
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/plain")
                .set_body_string("Second primary source"),
        ),
        ("/failed-second", ResponseTemplate::new(404)),
    ] {
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/not-requested"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let fetch_http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(server.uri()).unwrap())
        .redirect(Policy::none())
        .build()
        .unwrap();
    let scout = ScoutBuilder::for_test()
        .with_brave_endpoint(&format!("{}/search", server.uri()))
        .with_fetch_http(fetch_http)
        .with_egress(EgressMode::Proxied(server.uri()))
        .build();
    let result = scout
        .run(Command::Research(ResearchParams {
            query: Some("Primary sources".into()),
            depth: 4,
            lang: Lang::En,
        }))
        .await
        .unwrap();
    let data = result.data();
    assert_eq!(data["query"], "Primary sources");
    assert_eq!(data["fetched_pages"].as_array().unwrap().len(), 2);
    assert_eq!(
        data["fetched_pages"][0]["url"],
        "http://articles.example/first"
    );
    assert_eq!(
        data["fetched_pages"][1]["url"],
        "http://articles.example/second"
    );
    assert_eq!(data["failed_urls"].as_array().unwrap().len(), 2);
    assert_eq!(
        data["failed_urls"][0]["url"],
        "http://articles.example/failed-first"
    );
    assert_eq!(
        data["failed_urls"][1]["url"],
        "http://articles.example/failed-second"
    );
    assert_eq!(data["sources"].as_array().unwrap().len(), 5);
    assert!(result.markdown().contains("First primary source"));
    assert!(result.markdown().contains("Second primary source"));
    assert_eq!(
        serde_json::to_value(result.into_envelope()).unwrap()["degraded_reasons"],
        json!(["URL_FETCH_FAILED", "URL_FETCH_FAILED"])
    );
}

/// [T-HP007] Auto language sends one verbatim query and returns an undegraded empty report.
#[tokio::test]
async fn research_auto_language_keeps_the_query_and_zero_result_contract() {
    let Some(server) = try_spawn_mock_server("hotpaths::research_auto").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(query_param("q", "型安全 TypeScript"))
        .and(query_param_is_missing("search_lang"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"web": {"results": []}})))
        .expect(1)
        .mount(&server)
        .await;
    let scout = ScoutBuilder::for_test()
        .with_brave_endpoint(&server.uri())
        .build();
    let result = scout
        .run(Command::Research(ResearchParams {
            query: Some("型安全 TypeScript".into()),
            depth: 3,
            lang: Lang::Auto,
        }))
        .await
        .unwrap();
    assert_eq!(
        result.data(),
        &json!({
            "query": "型安全 TypeScript", "sources": [], "fetched_pages": [], "failed_urls": []
        })
    );
    assert!(result.markdown().contains("## Sources\n\n(no results)"));
    assert_eq!(
        serde_json::to_value(result.into_envelope()).unwrap()["degraded"],
        false
    );
}
