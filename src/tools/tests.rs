use super::*;
use crate::clock::FixedClock;
use crate::envelope::ErrorCode;
use crate::fetch::{FailingDnsResolver, StaticDnsResolver};
use crate::rng::SeededRng;
use crate::search::Lang;
use crate::test_support::try_spawn_mock_server;
use crate::token_source::StaticTokenSource;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, ResponseTemplate};

fn scout_with_brave(brave_uri: &str) -> Scout {
    scout_with_github(brave_uri, "http://localhost:0")
}

/// [T-009] search returns plain URL list with no markdown decoration
#[tokio::test]
async fn search_returns_plain_url_list() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://rust-lang.org", "title": "Rust", "description": "snippet"},
                    {"url": "https://doc.rust-lang.org", "title": "Docs", "description": "more"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("What is Rust?".into()),
        lang: Lang::Auto,
    };

    let result = s.search(params).await.unwrap();
    assert_eq!(
        result.markdown(),
        "https://rust-lang.org\nhttps://doc.rust-lang.org",
        "stdout should be one URL per line, no markdown decoration"
    );
}

/// [T-009-json] search --json output schema (data.query, data.sources, no data.answer)
#[tokio::test]
async fn search_json_schema_omits_answer() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://a.com", "title": "A", "description": "d"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("foo".into()),
        lang: Lang::Auto,
    };

    let result = s.search(params).await.unwrap();
    let data = result.data();
    assert!(data.get("answer").is_none(), "answer field must be absent");
    assert_eq!(data["query"], "foo");
    assert!(data["sources"].is_array());
    assert_eq!(data["sources"][0]["url"], "https://a.com");
    assert_eq!(data["sources"][0]["title"], "A");
    assert_eq!(data["sources"][0]["description"], "d");
}

/// [T-015] search command issues exactly one Brave call (no engine::research fanout)
/// Engine path adds fetch + report; search must remain a single Brave round-trip.
#[tokio::test]
async fn search_does_not_traverse_engine_path() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {"results": [{"url": "https://a.com", "title": "A", "description": ""}]}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("foo".into()),
        lang: Lang::Auto,
    };
    s.search(params).await.unwrap();
}

/// [T-009-empty] search with zero results returns empty stdout and exit 0
#[tokio::test]
async fn search_zero_results_returns_empty() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {"results": []}
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("foo".into()),
        lang: Lang::Auto,
    };
    let result = s.search(params).await.unwrap();
    assert_eq!(result.markdown(), "", "empty stdout for zero results");
    assert_eq!(result.data()["sources"].as_array().unwrap().len(), 0);
}

/// [T-TS002] research returns report with Brave sources and no obsolete Search Result header
#[tokio::test]
async fn research_success_returns_report() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    // Brave search response. The URL is unreachable, so fetch will fail and land in
    // failed_urls, but the Sources section still proves the Brave URL flowed through.
    Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://rust-lang.test/", "title": "Rust Language", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

    let s = scout_with_brave(&server.uri());
    let params = ResearchParams {
        query: Some("What is Rust?".into()),
        depth: 1,
        lang: Lang::Auto,
    };

    let result = s.research(params).await.unwrap();
    assert!(
        result.markdown().contains("rust-lang.test"),
        "report should reference Brave source URL, got: {result:?}"
    );
    assert!(
        !result.markdown().contains("## Search Result"),
        "AC-3.1: report must not contain the obsolete Search Result header"
    );
    assert!(
        !result
            .markdown()
            .contains("vertexaisearch.cloud.google.com"),
        "AC-3.2: Sources must not contain Google redirect URLs"
    );
}

/// [T-10] AC-4.2: --json research data schema (query, sources, fetched_pages, failed_urls)
#[tokio::test]
async fn research_json_schema_includes_required_keys() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://a.test/", "title": "A", "description": "snippet"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = ResearchParams {
        query: Some("foo".into()),
        depth: 1,
        lang: Lang::Auto,
    };
    let result = s.research(params).await.unwrap();
    let data = result.data();

    assert_eq!(data["query"], "foo", "data.query must echo the request");
    assert!(data["sources"].is_array(), "data.sources must be an array");
    assert_eq!(data["sources"][0]["url"], "https://a.test/");
    assert_eq!(data["sources"][0]["title"], "A");
    assert_eq!(data["sources"][0]["description"], "snippet");
    assert!(
        data["fetched_pages"].is_array(),
        "data.fetched_pages must be an array (possibly empty)"
    );
    assert!(
        data["failed_urls"].is_array(),
        "data.failed_urls must be an array (possibly empty)"
    );
    assert!(
        data.get("answer").is_none(),
        "data.answer must be absent (AC-4.1: no LLM-generated answer)"
    );
    assert!(
        data.get("all_sources").is_none(),
        "data.all_sources is the legacy key — must be renamed to sources"
    );
}

/// [T-028] (unit / FR-019)
/// Setup: wiremock always returns HTTP 503 (still fails after retry).
/// Action: `Scout::research(...)` is invoked.
/// Expected: returns `Ok(CommandOutput)` (no hard-fail);
/// `degraded_reasons` contains `BraveSearchFailed`; `data.sources` is empty.
/// RC-03 fix: cascade no longer propagates `BraveError`; failure is absorbed
/// into the degraded report envelope.
#[tokio::test]
async fn research_brave_failure_returns_degraded_report() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let result = s
        .research(ResearchParams {
            query: Some("foo".into()),
            depth: 1,
            lang: Lang::Auto,
        })
        .await
        .expect("research should yield Ok(degraded) on Brave failure, not propagate error");

    assert!(
        result
            .degraded_reasons()
            .contains(&DegradedReason::BraveSearchFailed),
        "degraded_reasons must contain BraveSearchFailed; got: {:?}",
        result.degraded_reasons()
    );
    let data = result.data();
    assert_eq!(
        data["sources"].as_array().unwrap().len(),
        0,
        "data.sources must be empty when Brave failed"
    );
}

/// [T-029] (unit / FR-019)
/// Setup: wiremock always returns HTTP 401.
/// Action: `Scout::research(...)` is invoked.
/// Expected: returns `Err(ScoutError)` (not a degraded `Ok`), because
/// `BraveError::Unauthorized` is a configuration error and must surface to
/// the user instead of being silently absorbed into the degraded envelope.
/// Companion to T-028 which covers the transient (503) degradable path.
#[tokio::test]
async fn research_unauthorized_propagates_as_error() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let result = s
        .research(ResearchParams {
            query: Some("foo".into()),
            depth: 1,
            lang: Lang::Auto,
        })
        .await;

    assert!(
        result.is_err(),
        "Unauthorized must propagate as Err, not be degraded; got: {result:?}"
    );
}

/// [T-11] AC-4.3: zero results yield empty arrays, not null
#[tokio::test]
async fn research_json_zero_results_returns_empty_arrays() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {"results": []}
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = ResearchParams {
        query: Some("foo".into()),
        depth: 1,
        lang: Lang::Auto,
    };
    let result = s.research(params).await.unwrap();
    let data = result.data();

    assert_eq!(
        data["sources"].as_array().unwrap().len(),
        0,
        "data.sources must be an empty array (not null)"
    );
    assert_eq!(
        data["fetched_pages"].as_array().unwrap().len(),
        0,
        "data.fetched_pages must be an empty array"
    );
    assert_eq!(
        data["failed_urls"].as_array().unwrap().len(),
        0,
        "data.failed_urls must be an empty array"
    );
}

/// [T-TS003] fetch_output_shifts_headings
#[test]
fn fetch_output_shifts_headings() {
    let result = FetchResult::for_test(
        "https://example.com".into(),
        "# Title\n## Section\nContent".into(),
        false,
    );
    let output = format_fetch_output(&result);
    assert!(output.contains("### Title"), "h1 should shift to h3");
    assert!(output.contains("#### Section"), "h2 should shift to h4");
}

/// [T-TS004] fetch_output_shifts_headings_with_raw_fallback
#[test]
fn fetch_output_shifts_headings_with_raw_fallback() {
    let result = FetchResult::for_test(
        "https://example.com".into(),
        "# Raw Title\nBody".into(),
        true,
    );
    let output = format_fetch_output(&result);
    assert!(
        output.starts_with(RAW_FALLBACK_NOTE.trim_end()),
        "should prepend fallback note"
    );
    assert!(output.contains("### Raw Title"), "h1 should shift to h3");
}

/// [T-TS005] fetch_output_truncates_long_content
#[test]
fn fetch_output_truncates_long_content() {
    let result = FetchResult::for_test(
        "https://example.com".into(),
        format!("# Title\n{}", "x".repeat(150_000)),
        false,
    );
    let output = format_fetch_output(&result);
    assert!(
        output.len() < 150_000,
        "output should be truncated, got {} bytes",
        output.len()
    );
    assert!(
        output.contains("(truncated: showing"),
        "should include truncation message"
    );
    assert!(
        output.contains("### Title"),
        "headings should still be shifted"
    );
}

// --- GitHub client efficiency tests (lazy init + repo_overview) ---

fn scout_with_github(brave_uri: &str, github_uri: &str) -> Scout {
    ScoutBuilder::for_test()
        .with_brave_endpoint(brave_uri)
        .with_github_endpoint(github_uri)
        .build()
}

fn scout_lazy(brave_uri: &str) -> Scout {
    ScoutBuilder::for_test()
        .with_brave_endpoint(brave_uri)
        .build()
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

/// [T-TS017] repo_read: --encoding hint is passed to decode_content and
/// used to decode non-UTF-8 content correctly.
#[tokio::test]
async fn repo_read_decodes_with_encoding_hint() {
    let Some(server) = try_spawn_mock_server("tools::t_008").await else {
        return;
    };

    // "テスト" in Shift_JIS ([0x83, 0x65, 0x83, 0x58, 0x83, 0x67]), base64-encoded.
    // Without --encoding, chardetng auto-detects Shift_JIS for 6 bytes too.
    // With --encoding shift_jis, decode_explicit is used (deterministic).
    let shift_jis_b64 = "g2WDWINn";

    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/owner/repo/contents/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": "abc123",
            "content": shift_jis_b64
        })))
        .mount(&server)
        .await;

    let s = scout_with_github("http://localhost:0", &server.uri());
    let params = RepoReadParams {
        repository: Some("owner/repo".into()),
        path: Some("test.txt".into()),
        ref_: None,
        lines: None,
        encoding: Some("shift_jis".into()),
    };

    let result = s.repo_read(params).await.unwrap();
    assert!(
        result.markdown().contains("テスト"),
        "output should contain decoded Shift_JIS text, got: {result:?}"
    );
    assert!(
        result.markdown().contains("[encoding: shift_jis]"),
        "header should include encoding label, got: {result:?}"
    );
}

/// [T-TS018] repo_tree: --path filter is wired through RepoTreeParams to
/// filter_tree_entries; files outside the prefix are excluded from output.
#[tokio::test]
async fn repo_tree_path_filter_excludes_non_matching_files() {
    let Some(server) = try_spawn_mock_server("tools::t_009").await else {
        return;
    };

    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "full_name": "owner/repo",
            "description": null,
            "html_url": "https://github.com/owner/repo",
            "default_branch": "main",
            "language": null,
            "stargazers_count": 0,
            "forks_count": 0,
            "open_issues_count": 0,
            "topics": null,
            "license": null
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/owner/repo/git/trees/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tree": [
                {"path": "src/main.rs", "type": "blob", "size": 100},
                {"path": "src/lib.rs", "type": "blob", "size": 200},
                {"path": "README.md", "type": "blob", "size": 50},
                {"path": "Cargo.toml", "type": "blob", "size": 80},
            ],
            "truncated": false
        })))
        .mount(&server)
        .await;

    let s = scout_with_github("http://localhost:0", &server.uri());
    let params = RepoTreeParams {
        repository: Some("owner/repo".into()),
        ref_: None,
        path: Some("src/".into()),
        pattern: None,
    };

    let result = s.repo_tree(params).await.unwrap();
    assert!(
        result.markdown().contains("src/main.rs"),
        "path filter should include src/main.rs, got:\n{result:?}"
    );
    assert!(
        !result.markdown().contains("README.md"),
        "path filter should exclude README.md, got:\n{result:?}"
    );
    assert!(
        !result.markdown().contains("Cargo.toml"),
        "path filter should exclude Cargo.toml, got:\n{result:?}"
    );
}

/// [T-R001] StdinResolver: first arg consumes stdin, second uses its own value
#[test]
fn stdin_resolver_first_consumes_second_uses_arg() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    let first = r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
    assert_eq!(first, "from_stdin");
    let second = r
        .resolve(Some("test.txt".into()), "path", "<FILE_PATH>")
        .unwrap();
    assert_eq!(second, "test.txt");
}

/// [T-R002] StdinResolver: arg wins over stdin, stdin preserved for next resolve
#[test]
fn stdin_resolver_arg_wins_stdin_preserved() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    let first = r
        .resolve(Some("owner/repo".into()), "repository", "<OWNER/REPO>")
        .unwrap();
    assert_eq!(first, "owner/repo");
    let second = r.resolve(None, "path", "<FILE_PATH>").unwrap();
    assert_eq!(second, "from_stdin");
}

/// [T-R003] StdinResolver: second arg fails when stdin already consumed
#[test]
fn stdin_resolver_consumed_stdin_fails_second() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
    let result = r.resolve(None, "path", "<FILE_PATH>");
    assert!(
        result.is_err(),
        "second positional should fail when stdin consumed"
    );
}

/// [T-R005] StdinResolver: error message hints stdin was consumed, not missing
#[test]
fn stdin_resolver_consumed_error_hints_stdin_exhausted() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
    let err = r
        .resolve(None, "path", "<FILE_PATH>")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("stdin was already read"),
        "error should hint stdin was consumed, got: {err}"
    );
    assert!(
        !err.contains("pipe it via stdin"),
        "error should not suggest piping when stdin is exhausted, got: {err}"
    );
}

/// [T-R004] StdinResolver: both args provided, stdin unused
#[test]
fn stdin_resolver_both_args_stdin_unused() {
    let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
    let first = r
        .resolve(Some("owner/repo".into()), "repository", "<OWNER/REPO>")
        .unwrap();
    let second = r
        .resolve(Some("test.txt".into()), "path", "<FILE_PATH>")
        .unwrap();
    assert_eq!(first, "owner/repo");
    assert_eq!(second, "test.txt");
}

// --- ScoutBuilder seam tests (issue #103) ---

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
        .fetch(FetchParams {
            url: Some("https://example.com/page".into()),
            js: false,
            raw: false,
        })
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
        .fetch(FetchParams {
            url: Some("https://example.com/page".into()),
            js: false,
            raw: false,
        })
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
