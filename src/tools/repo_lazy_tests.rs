use super::test_helpers::*;
use super::*;
use crate::ErrorCode;
use crate::search::Lang;
use crate::test_support::try_spawn_mock_server;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-TS036] `repo-tree` rejects a malformed `--path` or `--ref` before the
/// GitHub client exists.
///
/// Both checks read the argument's shape alone and need nothing from the
/// network, while the first `Scout::github()` touch in production pays a
/// `gh auth token` subprocess (DR-0008). `repo_read` already validates first;
/// this pins the same order for `repo_tree`, whose own comment claims it.
#[tokio::test]
async fn repo_tree_rejects_a_malformed_argument_before_building_the_client() {
    for (label, params) in [
        (
            "path",
            RepoTreeParams {
                repository: Some("owner/repo".into()),
                ref_: None,
                path: Some("../etc/passwd".into()),
                pattern: None,
            },
        ),
        (
            "ref",
            RepoTreeParams {
                repository: Some("owner/repo".into()),
                ref_: Some("main..evil".into()),
                path: None,
                pattern: None,
            },
        ),
    ] {
        let s = scout_lazy("http://localhost:0");
        let result = s.repo_tree(params).await;
        assert!(result.is_err(), "a malformed --{label} must be rejected");
        assert!(
            s.github.get().is_none(),
            "a malformed --{label} must not cost a GitHubClient build"
        );
    }
}

/// [T-TS012]
#[tokio::test]
async fn search_leaves_github_uninitialized() {
    let Some(server) = try_spawn_mock_server("tools::t_004").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://example.com", "title": "Example", "description": "snippet"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_lazy(&server.uri());
    let result = s
        .search(SearchParams {
            query: Some("test".into()),
            lang: Lang::En,
        })
        .await;

    assert!(
        result.is_ok(),
        "search should succeed against Brave mock; got: {:?}",
        result.err()
    );
    assert!(
        s.github.get().is_none(),
        "search should not initialize GitHubClient on the success path"
    );
}

/// [T-TS013]
#[tokio::test]
async fn fetch_leaves_github_uninitialized() {
    let Some(server) = try_spawn_mock_server("tools::t_005").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<html><body>hello</body></html>"),
        )
        .mount(&server)
        .await;

    let s = scout_lazy(&server.uri());
    let _result = s
        .fetch(FetchParams::for_test(&format!("{}/page", server.uri())))
        .await;

    assert!(
        s.github.get().is_none(),
        "fetch should not initialize GitHubClient"
    );
}

/// [T-TS014]
/// Brave succeeds; the fetched URL is invalid (DNS failure), driving a degraded
/// ResearchReport (Ok) without touching GitHub.
#[tokio::test]
async fn research_leaves_github_uninitialized() {
    let Some(server) = try_spawn_mock_server("tools::t_006").await else {
        return;
    };
    Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://nonexistent.invalid", "title": "Example", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

    let s = scout_lazy(&server.uri());
    let result = s
        .research(ResearchParams {
            query: Some("test".into()),
            depth: 1,
            lang: Lang::En,
        })
        .await;

    assert!(
        result.is_ok(),
        "research should return Ok (degraded report) even when fetch fails; got: {:?}",
        result.err()
    );
    assert!(
        s.github.get().is_none(),
        "research should not initialize GitHubClient on the success path"
    );
}

/// [T-TS016] The first `github()` call on an empty cell builds the client,
/// and the second call returns the same cached reference.
///
/// `for_test()` supplies an empty `StaticTokenSource`, so the init reads no
/// token env var and never runs `gh auth token` (T-SB007).
#[tokio::test]
async fn github_lazy_init_from_empty_cell() {
    use std::ptr;
    let s = scout_lazy("http://localhost:0");
    assert!(s.github.get().is_none(), "starts empty");

    let client = s.github().await;

    assert!(
        s.github.get().is_some(),
        "OnceCell should be initialized after github() call"
    );
    let client2 = s.github().await;
    assert!(
        ptr::eq(client, client2),
        "second call returns the same cached reference"
    );
}

/// [T-TS023] slack() lazily routes the non-injected production path through
/// `SlackClient::from_env`, and a missing `SLACK_TOKEN` surfaces as a
/// `ScoutError` without poisoning the `OnceCell` (`get_or_try_init` does not
/// cache errors). This covers the lazy seam that `with_slack_endpoint` bypasses
/// in T-SB005.
///
/// Guarded like `try_spawn_mock_server`: when the ambient environment provides a
/// real `SLACK_TOKEN` (developer shell), `from_env` would succeed and this spec
/// no longer applies, so skip rather than assert a different spec. CI runs with
/// `SLACK_TOKEN` unset, exercising the asserted `TokenNotSet` branch.
#[tokio::test]
async fn slack_lazy_init_surfaces_token_not_set_without_token() {
    use std::env;
    if env::var("SLACK_TOKEN").is_ok_and(|t| !t.trim().is_empty()) {
        return;
    }
    let s = scout_lazy("http://localhost:0");
    assert!(s.slack.get().is_none(), "slack OnceCell starts empty");

    // `.map(|_| ())` drops the `&SlackClient` (no `Debug`) so `expect_err` can
    // format the `Ok` side on failure.
    let err = s
        .slack()
        .await
        .map(|_| ())
        .expect_err("unset SLACK_TOKEN must surface as an error");
    assert_eq!(
        err.error_kind(),
        ErrorCode::UsageError,
        "missing Slack token maps to UsageError (SlackError::TokenNotSet)"
    );
    assert!(
        s.slack.get().is_none(),
        "get_or_try_init must not cache a failed initialization"
    );
}
