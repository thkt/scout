use super::test_helpers::*;
use super::*;
use crate::ErrorCode;
use crate::search::Lang;
use crate::test_support::try_spawn_mock_server;
use std::time::Duration;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, ResponseTemplate};

/// [T-TS020] run() wraps GitHub commands in the outer github_timeout
///
/// Issue #185: `repo-tree` / `repo-read` / `repo-overview` reach GitHub through
/// bare async handlers; a persistent slow/5xx upstream would hang the command
/// for minutes (each inner HTTP call has its own 30s timeout plus retries).
/// `run()` now caps the whole command. A GitHub endpoint that delays 30s past
/// the 200ms `github_timeout` must surface a timeout error AND emit the
/// `"github command timed out"` warn — the log assertion is non-tautological:
/// a real connect failure would also yield `is_err()` but emit no such warn.
/// Going through `run()` (not the handler directly) is essential since the guard
/// lives at the dispatch layer.
#[tokio::test]
#[tracing_test::traced_test]
async fn run_repo_tree_times_out_on_slow_github() {
    let Some(server) = try_spawn_mock_server("tools::t_185").await else {
        return;
    };

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let s = scout_with_github_timeout(&server.uri(), &server.uri(), Duration::from_millis(200));
    let cmd = Command::RepoTree(RepoTreeParams {
        repository: Some("owner/repo".into()),
        ref_: None,
        path: None,
        pattern: None,
    });

    let result = s.run(cmd).await;
    let err = result.expect_err("slow GitHub must trip the outer github_timeout");
    assert!(
        err.message().contains("timed out"),
        "expected a timeout error, got: {}",
        err.message()
    );
    assert!(
        logs_contain("github command timed out"),
        "expected the run()-level github timeout guard to fire",
    );
}

/// [T-TS021] run() wraps repo-read in the outer github_timeout
///
/// Companion to [T-TS020]: the guard is applied per dispatch arm in `run()`, so
/// repo-read needs its own coverage of that arm (issue #185).
#[tokio::test]
async fn run_repo_read_times_out_on_slow_github() {
    let Some(server) = try_spawn_mock_server("tools::t_185_read").await else {
        return;
    };

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let s = scout_with_github_timeout(&server.uri(), &server.uri(), Duration::from_millis(200));
    let cmd = Command::RepoRead(RepoReadParams {
        repository: Some("owner/repo".into()),
        path: Some("README.md".into()),
        ref_: None,
        lines: None,
        encoding: None,
    });

    let err = s
        .run(cmd)
        .await
        .expect_err("slow GitHub must trip the outer github_timeout");
    assert!(
        err.message().contains("timed out"),
        "expected a timeout error, got: {}",
        err.message()
    );
}

/// [T-TS022] run() wraps repo-overview in the outer github_timeout
///
/// Companion to [T-TS020]: per-arm coverage of the repo-overview dispatch arm
/// in `run()` (issue #185).
#[tokio::test]
async fn run_repo_overview_times_out_on_slow_github() {
    let Some(server) = try_spawn_mock_server("tools::t_185_overview").await else {
        return;
    };

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let s = scout_with_github_timeout(&server.uri(), &server.uri(), Duration::from_millis(200));
    let cmd = Command::RepoOverview(RepoOverviewParams {
        repository: Some("owner/repo".into()),
    });

    let err = s
        .run(cmd)
        .await
        .expect_err("slow GitHub must trip the outer github_timeout");
    assert!(
        err.message().contains("timed out"),
        "expected a timeout error, got: {}",
        err.message()
    );
}

/// [T-TS009] repo_overview: get_repo 404 -> readme/issues/pulls/releases
/// APIs receive 0 requests.
#[tokio::test]
async fn repo_overview_404_skips_remaining_apis() {
    let Some(server) = try_spawn_mock_server("tools::t_001").await else {
        return;
    };

    // get_repo returns 404
    Mock::given(method("GET"))
        .and(path("/repos/owner/nonexistent"))
        .respond_with(ResponseTemplate::new(404))
        .named("get_repo 404")
        .mount(&server)
        .await;

    // All other APIs expect 0 requests
    Mock::given(method("GET"))
        .and(path("/repos/owner/nonexistent/readme"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .named("readme must not be called")
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/repos/owner/nonexistent/issues"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .named("issues must not be called")
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/repos/owner/nonexistent/pulls"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .named("pulls must not be called")
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/repos/owner/nonexistent/releases"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .named("releases must not be called")
        .mount(&server)
        .await;

    let s = scout_with_github(&server.uri(), &server.uri());
    let params = RepoOverviewParams {
        repository: Some("owner/nonexistent".into()),
    };

    let result = s.repo_overview(params).await;
    assert!(result.is_err(), "repo_overview should fail on 404");

    // wiremock verifies expect(0) on server drop
}

/// [T-TS010] repo_overview: after get_repo succeeds, readme/issues/pulls/
/// releases run in parallel.
///
/// Proof: a barrier-synchronized TCP server requires all 4 API requests to
/// arrive before any response is sent. If requests are sequential, only one
/// arrives at a time and the barrier never releases → deadlock → timeout.
#[tokio::test(flavor = "multi_thread")]
async fn repo_overview_parallel_after_get_repo() {
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Barrier;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

    // 4 parallel APIs must all arrive before any response is sent.
    let barrier = Arc::new(Barrier::new(4));

    let server = tokio::spawn(async move {
        for _ in 0..10 {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let b = barrier.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("");

                let (body, wait) = if path == "/repos/owner/repo" {
                    (r#"{"full_name":"owner/repo","description":"test","html_url":"https://github.com/owner/repo","default_branch":"main","language":"Rust","stargazers_count":1,"forks_count":0,"open_issues_count":0,"topics":[],"license":null}"#.to_owned(), false)
                } else if path.contains("/git/blobs/") {
                    (r#"{"content":""}"#.to_owned(), false)
                } else if path.contains("/readme") {
                    (r#"{"sha":"abc123","content":""}"#.to_owned(), true)
                } else {
                    ("[]".to_owned(), true)
                };

                if wait {
                    b.wait().await;
                }

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });

    let s = scout_with_github(&base_url, &base_url);
    let params = RepoOverviewParams {
        repository: Some("owner/repo".into()),
    };

    // Parallel: barrier(4) releases instantly → completes in ms.
    // Sequential: barrier never reaches 4 → deadlock → timeout.
    let result = timeout(Duration::from_secs(5), s.repo_overview(params)).await;

    assert!(
        result.is_ok(),
        "repo_overview should complete when 4 APIs run in parallel \
             (barrier-synchronized); sequential execution deadlocks"
    );

    server.abort();
}

/// [T-TS011] scout_lazy: github OnceCell is None immediately after
/// construction.
#[test]
fn scout_lazy_github_initially_none() {
    let s = scout_lazy("http://localhost:0");
    assert!(
        s.github.get().is_none(),
        "github OnceCell should be uninitialized after scout_lazy()"
    );
}

/// [T-TS012] search command does not initialize the GitHub client on the success path.
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

/// [T-TS013] fetch command does not initialize the GitHub client.
#[tokio::test]
async fn fetch_leaves_github_uninitialized() {
    let Some(server) = try_spawn_mock_server("tools::t_005").await else {
        return;
    };
    // Serve a minimal HTML page for the fetch command to consume.
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
        .fetch(FetchParams {
            url: Some(format!("{}/page", server.uri())),
            js: false,
            raw: false,
        })
        .await;

    assert!(
        s.github.get().is_none(),
        "fetch should not initialize GitHubClient"
    );
}

/// [T-TS014] research command does not initialize the GitHub client on the success path.
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

/// [T-TS015] github() called twice returns the same reference
/// (OnceCell caching verified via std::ptr::eq).
#[tokio::test]
async fn github_returns_same_reference() {
    use std::ptr;
    // Use pre-set OnceCell to avoid triggering real `gh auth token` subprocess.
    let s = scout_with_github("http://localhost:0", "http://localhost:0");
    let first = s.github().await;
    let second = s.github().await;
    assert!(
        ptr::eq(first, second),
        "github() should return the same cached reference on second call"
    );
}

/// [T-TS016] github() initializes an empty OnceCell via from_env and caches
/// the result. Exercises the lazy-init code path at mod.rs:80-84.
///
/// from_env is infallible: it resolves token from env vars or `gh auth token`
/// (with TOKEN_RESOLVE_TIMEOUT = 5s), then returns a client. No timeout
/// wrapper — a hang here is a real bug, not a flaky environment.
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
/// in T-SB005 (issue #191).
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
