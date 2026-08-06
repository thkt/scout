use std::sync::atomic::Ordering;

use super::*;
use crate::clock::FixedClock;
use crate::envelope::ErrorCode;
use crate::test_support::{mount_get, spawn_mid_stream_drop_server, try_spawn_mock_server};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-GH021] get_contents on a directory reports the path shape, not a decode fault
///
/// The contents endpoint answers with a JSON array for a directory, which the
/// file-shaped struct cannot parse. Left as `Decode`, that reaches the caller as
/// INTERNAL (70) — telling an agent scout has a bug when it passed a directory,
/// which is the natural next step after `repo-tree`.
#[tokio::test]
async fn get_contents_on_a_directory_is_a_data_error_not_internal() {
    let Some(server) = try_spawn_mock_server("github::get_contents_dir").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo/contents/src",
        ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"name": "lib.rs", "path": "src/lib.rs", "sha": "abc", "type": "file"},
            {"name": "main.rs", "path": "src/main.rs", "sha": "def", "type": "file"},
        ])),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let err = client
        .get_contents("owner", "repo", "src", None)
        .await
        .expect_err("a directory path cannot yield file contents");

    assert!(
        matches!(err, GitHubError::PathIsDirectory(ref p) if p == "src"),
        "expected PathIsDirectory carrying the path, got: {err:?}"
    );
    assert_eq!(
        err.classify().kind,
        ErrorCode::DataError,
        "a directory path is caller input, not a scout-side invariant violation"
    );
}

/// [T-GH001]
#[tokio::test]
async fn get_json_404_returns_not_found() {
    let Some(server) = try_spawn_mock_server("github::get_json_404").await else {
        return;
    };
    mount_get(&server, "/repos/owner/repo", ResponseTemplate::new(404)).await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(matches!(result, Err(GitHubError::NotFound(_))));
}

/// [T-GH002]
#[tokio::test]
async fn get_json_429_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(&server, "/repos/owner/repo", ResponseTemplate::new(429)).await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
}

/// [T-GH022] a 429 without Retry-After still derives the wait from x-ratelimit-reset
///
/// GitHub sends `Retry-After` on some rate limits and `x-ratelimit-reset` on
/// others and guarantees neither for a given status, so reading only one of them
/// per status left the 429 path with no wait time at all — falling back to
/// jittered backoff while the server had stated when the window reopens.
#[tokio::test]
async fn get_json_429_without_retry_after_uses_ratelimit_reset() {
    let Some(server) = try_spawn_mock_server("github::http_429_reset").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(429).insert_header("x-ratelimit-reset", "1000300"),
    )
    .await;

    // `get_json_once`, not `get_json`: the retry loop honors the returned delay,
    // so driving the full loop here would sleep for the window under test.
    let client = GitHubClient::with_base_url(Client::new(), &server.uri())
        .with_clock(Arc::new(FixedClock(1_000_000)));
    let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
    assert!(
        matches!(
            result,
            Err(GitHubError::RateLimited {
                retry_after: Some(300)
            })
        ),
        "expected the 300s window from x-ratelimit-reset, got: {result:?}"
    );
}

/// [T-GH003]
#[tokio::test]
async fn get_json_403_with_zero_remaining_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(403)
            .append_header("x-ratelimit-remaining", "0")
            .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
}

/// [T-GH004] get_json maps 403 with non-zero remaining to Forbidden error
#[tokio::test]
async fn get_json_403_with_remaining_returns_forbidden() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(403)
            .append_header("x-ratelimit-remaining", "50")
            .set_body_json(serde_json::json!({"message": "access denied"})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(matches!(result, Err(GitHubError::Forbidden(ref msg)) if msg == "access denied"));
}

/// [T-GH010] 403 without x-ratelimit-remaining header maps to RateLimited (ADR-0004 Rule 1)
#[tokio::test]
async fn get_json_403_with_missing_remaining_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(403)
            .set_body_json(serde_json::json!({"message": "secondary rate limit"})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
}

/// [T-GH011a]
#[test]
fn per_page_new_preserves_boundary_values() {
    assert_eq!(super::PerPage::new(1).to_string(), "1");
    assert_eq!(super::PerPage::new(100).to_string(), "100");
}

/// [T-GH011b] ADR-0004 Rule 2; GitHub API's per_page=0 behavior is implementation-defined (empty array or default)
#[test]
#[should_panic(expected = "PerPage must be 1..=100")]
fn per_page_new_panics_on_zero() {
    let _ = super::PerPage::new(0);
}

/// [T-GH011c] ADR-0004 Rule 2
#[test]
#[should_panic(expected = "PerPage must be 1..=100")]
fn per_page_new_panics_on_over_100() {
    let _ = super::PerPage::new(101);
}

/// [T-GH018] Proves the injected `TokenSource` reaches the constructor without spawning `gh auth token`.
#[tokio::test]
async fn from_env_with_static_source_installs_token() {
    use crate::token_source::StaticTokenSource;
    let source = StaticTokenSource(Some(
        Redacted::new("injected-token").expect("static literal is non-empty"),
    ));
    let client = GitHubClient::from_env_with_source(Client::new(), 0, &source).await;
    assert_eq!(
        client.token.as_ref().map(Redacted::expose),
        Some("injected-token")
    );
}

/// [T-GH019] A None source lets callers simulate the unauthenticated path
/// without env-var manipulation.
#[tokio::test]
async fn from_env_with_none_source_leaves_token_empty() {
    use crate::token_source::StaticTokenSource;
    let client =
        GitHubClient::from_env_with_source(Client::new(), 0, &StaticTokenSource(None)).await;
    assert!(client.token.is_none());
}

/// [T-GH006]
#[tokio::test]
async fn get_json_500_returns_api_error() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/test",
        ResponseTemplate::new(500)
            .set_body_json(serde_json::json!({"message": "internal server error"})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<serde_json::Value, _> = client.get_json("/test").await;
    assert!(matches!(result, Err(GitHubError::Api { code: 500, .. })));
}

/// [T-GH007] get_json_once propagates Retry-After header value on 429
#[tokio::test]
async fn get_json_429_with_retry_after_carries_delay() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(429).append_header("Retry-After", "30"),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
    assert!(matches!(
        result,
        Err(GitHubError::RateLimited {
            retry_after: Some(30)
        })
    ));
}

/// [T-GH008] Pinned clock + reset 60s later asserts the exact subtraction result.
#[tokio::test]
async fn get_json_403_with_ratelimit_reset_carries_delay() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(403)
            .append_header("x-ratelimit-remaining", "0")
            .append_header("x-ratelimit-reset", "1060")
            .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri())
        .with_clock(Arc::new(FixedClock(1000)));
    let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
    assert!(
        matches!(
            result,
            Err(GitHubError::RateLimited {
                retry_after: Some(60)
            })
        ),
        "expected retry_after = 60 (reset 1060 - clock 1000), got: {result:?}"
    );
}

/// [T-GH012] 2xx response with malformed JSON classifies as Decode (issue #101).
///
/// Before this fix, `Ok(response.json().await?)` routed schema failures through
/// `#[from] reqwest::Error` to `GitHubError::Network`, surfacing as TempFailure(75)
/// retryable=true. Schema fail is a scout-side invariant violation that retry
/// cannot resolve — must classify as Decode → Internal(70) retryable=false.
#[tokio::test]
async fn get_json_2xx_malformed_body_returns_decode() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(200).set_body_string("{not valid json"),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(
        matches!(result, Err(GitHubError::Decode(_))),
        "expected GitHubError::Decode for 2xx malformed JSON, got: {result:?}"
    );
}

/// [T-GH013] 2xx mid-stream body drop is treated as transient and the
/// retry loop exhausts the configured max_retries attempts before failing
/// (issue #113). The test uses the client default (3); issue #120 lets
/// production callers override the budget via `SCOUT_MAX_RETRIES`.
///
/// reqwest 0.13 surfaces a mid-stream drop as `is_decode() == true` with
/// an io::Error in the source chain. Without `is_transient_decode`,
/// every attempt would route to `GitHubError::Decode` → Internal(70)
/// retryable=false. With it, attempts route to `GitHubError::Network` →
/// TempFailure(75) and the retry loop kicks in.
///
/// `start_paused = true` advances the tokio runtime past `retry_with`'s
/// `sleep` calls as soon as the task parks; the std::thread-driven
/// TcpListener is unaffected. Total wall time stays under 100 ms.
#[tokio::test(start_paused = true)]
async fn get_json_2xx_mid_stream_drop_exhausts_retries() {
    // Total attempts = 1 (initial) + DEFAULT_MAX_RETRIES (retries).
    let expected_attempts =
        usize::try_from(DEFAULT_MAX_RETRIES + 1).expect("DEFAULT_MAX_RETRIES + 1 fits usize");
    let Some((url, counter, handle)) = spawn_mid_stream_drop_server(expected_attempts) else {
        return;
    };

    let client = GitHubClient::with_base_url(Client::new(), &url);
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;

    assert!(
        matches!(result, Err(GitHubError::Network(_))),
        "expected GitHubError::Network for exhausted mid-stream drop, got: {result:?}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        expected_attempts,
        "retry loop must consume the full max_retries budget"
    );

    handle
        .join()
        .expect("server thread should not panic")
        .expect("server thread should not fail while writing the response");
}

/// [T-GH020] A 2xx response whose body exceeds `MAX_GITHUB_RESPONSE_BYTES`
/// returns `ResponseTooLarge` instead of buffering the whole body (issue #186).
/// The variant is non-retriable, so the mock is hit exactly once.
#[tokio::test]
async fn get_json_2xx_oversized_body_returns_too_large() {
    let Some(server) = try_spawn_mock_server("github::http").await else {
        return;
    };
    // One byte past the cap. wiremock sets a matching Content-Length, so this
    // exercises the pre-check arm (the chunk-loop arm guards a lying/absent header).
    let body = vec![b'x'; MAX_GITHUB_RESPONSE_BYTES + 1];
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
    assert!(
        matches!(result, Err(GitHubError::ResponseTooLarge)),
        "expected ResponseTooLarge for oversized 2xx body, got: {result:?}"
    );
}

/// [T-001] github は HTTP 408 を 1 度返す API に対し再試行し 2 度目の成功レスポンスを返す
///
/// The retryable status set must not be re-tabled in `is_retriable`: 408 is
/// retried only because `Classification::from_http_status` calls it
/// TempFailure.
#[tokio::test]
async fn get_json_408_retries_once_then_succeeds() {
    let Some(server) = try_spawn_mock_server("github::http_408_retry").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(
            ResponseTemplate::new(408).set_body_json(serde_json::json!({"message": "timeout"})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 1})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri());
    let result: Result<serde_json::Value, _> = client.get_json("/repos/owner/repo").await;
    assert!(
        result.is_ok(),
        "408 should retry once and return the second call's success, got: {result:?}"
    );
}

/// [T-GH014] secs_until_ratelimit_reset subtracts the injected clock
/// from the x-ratelimit-reset header. Pinning the clock removes wall-clock
/// flakiness from the arithmetic test.
#[test]
fn secs_until_ratelimit_reset_uses_injected_clock() {
    use reqwest::header::HeaderValue;
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-reset", HeaderValue::from_static("1300"));
    let clock = FixedClock(1000);
    assert_eq!(secs_until_ratelimit_reset(&headers, &clock), Some(300));
}

/// [T-GH015] A plain `reset_ts - clock.now_secs()` would wrap to a huge delay
/// here; `saturating_sub` clamps it.
#[test]
fn secs_until_ratelimit_reset_saturates_when_clock_past_reset() {
    use reqwest::header::HeaderValue;
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-reset", HeaderValue::from_static("100"));
    let clock = FixedClock(200);
    assert_eq!(secs_until_ratelimit_reset(&headers, &clock), Some(0));
}

/// [T-GH016] None leaves production callers on jittered backoff.
#[test]
fn secs_until_ratelimit_reset_returns_none_when_header_missing() {
    let headers = HeaderMap::new();
    let clock = FixedClock(1000);
    assert_eq!(secs_until_ratelimit_reset(&headers, &clock), None);
}

/// [T-GH017] with_clock injection threads through to the 403 retry_after
/// calculation. End-to-end proof that the seam works for callers (the
/// pure-function tests above only cover the helper in isolation).
#[tokio::test]
async fn get_json_403_uses_injected_clock_for_retry_after() {
    let Some(server) = try_spawn_mock_server("github::http_clock_inject").await else {
        return;
    };
    mount_get(
        &server,
        "/repos/owner/repo",
        ResponseTemplate::new(403)
            .append_header("x-ratelimit-remaining", "0")
            .append_header("x-ratelimit-reset", "1300")
            .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
    )
    .await;

    let client = GitHubClient::with_base_url(Client::new(), &server.uri())
        .with_clock(Arc::new(FixedClock(1000)));
    let result: Result<RepoInfo, _> = client.get_json_once("/repos/owner/repo").await;
    assert!(
        matches!(
            result,
            Err(GitHubError::RateLimited {
                retry_after: Some(300)
            })
        ),
        "expected retry_after = 300 (reset 1300 - clock 1000), got: {result:?}"
    );
}
