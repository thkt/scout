use super::test_helpers::*;
use super::*;
use crate::test_support::try_spawn_mock_server;
use std::time::Duration;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, ResponseTemplate};

/// [T-TS020] Slow repo-tree requests return the command's timeout error.
#[tokio::test]
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
    assert_eq!(err.exit_code(), 124);
}

/// [T-TS021] run() wraps repo-read in the outer github_timeout
///
/// Companion to T-TS020: the guard is applied per dispatch arm in `run()`, so
/// repo-read needs its own coverage of that arm.
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
/// Companion to T-TS020: per-arm coverage of the repo-overview dispatch arm
/// in `run()`.
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

/// [T-TS009]
#[tokio::test]
async fn repo_overview_404_skips_remaining_apis() {
    let Some(server) = try_spawn_mock_server("tools::t_001").await else {
        return;
    };

    Mock::given(method("GET"))
        .and(path("/repos/owner/nonexistent"))
        .respond_with(ResponseTemplate::new(404))
        .named("get_repo 404")
        .mount(&server)
        .await;

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
}

/// [T-TS010] All four optional overview requests progress before any completes.
#[tokio::test]
async fn repo_overview_parallel_after_get_repo() {
    use crate::test_support::http_fixture::HttpFixture;
    use serde_json::json;

    let server = HttpFixture::start(&[
        (
            "/repos/owner/repo",
            json!({
                "full_name": "owner/repo", "description": null,
                "html_url": "https://github.com/owner/repo", "default_branch": "main",
                "language": null, "stargazers_count": 0, "forks_count": 0,
                "open_issues_count": 0, "license": null
            }),
            &[],
        ),
        (
            "/repos/owner/repo/readme",
            json!({"sha": "readme", "content": "UkVBRE1FIGJvZHk="}),
            &[
                "/repos/owner/repo/issues",
                "/repos/owner/repo/pulls",
                "/repos/owner/repo/releases",
            ],
        ),
        (
            "/repos/owner/repo/issues",
            json!([]),
            &[
                "/repos/owner/repo/readme",
                "/repos/owner/repo/pulls",
                "/repos/owner/repo/releases",
            ],
        ),
        (
            "/repos/owner/repo/pulls",
            json!([]),
            &[
                "/repos/owner/repo/readme",
                "/repos/owner/repo/issues",
                "/repos/owner/repo/releases",
            ],
        ),
        (
            "/repos/owner/repo/releases",
            json!([]),
            &[
                "/repos/owner/repo/readme",
                "/repos/owner/repo/issues",
                "/repos/owner/repo/pulls",
            ],
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
    .expect("independent overview requests must progress together")
    .unwrap();
    assert_eq!(result.data()["readme"], "README body");
    assert_eq!(result.data()["issues"], json!([]));
    assert_eq!(result.data()["pulls"], json!([]));
    assert_eq!(result.data()["releases"], json!([]));
    assert!(result.markdown().contains("README body"));
}
