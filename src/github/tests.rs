use super::*;

mod http_tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::clock::FixedClock;
    use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    /// [T-GH001] get_json maps 404 responses to NotFound error
    #[tokio::test]
    async fn get_json_404_returns_not_found() {
        let Some(server) = try_spawn_mock_server("github::get_json_404").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::NotFound(_))));
    }

    /// [T-GH002] get_json maps 429 responses to RateLimited error
    #[tokio::test]
    async fn get_json_429_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
    }

    /// [T-GH003] get_json maps 403 with zero remaining to RateLimited error
    #[tokio::test]
    async fn get_json_403_with_zero_remaining_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
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
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "50")
                    .set_body_json(serde_json::json!({"message": "access denied"})),
            )
            .mount(&server)
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
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"message": "secondary rate limit"})),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::with_base_url(Client::new(), &server.uri());
        let result: Result<RepoInfo, _> = client.get_json("/repos/owner/repo").await;
        assert!(matches!(result, Err(GitHubError::RateLimited { .. })));
    }

    /// [T-GH011a] PerPage::new preserves boundary values 1 and 100 through Display.
    #[test]
    fn per_page_new_preserves_boundary_values() {
        assert_eq!(super::PerPage::new(1).to_string(), "1");
        assert_eq!(super::PerPage::new(100).to_string(), "100");
    }

    /// [T-GH011b] PerPage::new panics on 0 (ADR-0004 Rule 2; 0 is
    /// implementation-defined behavior in GitHub API)
    #[test]
    #[should_panic(expected = "PerPage must be 1..=100")]
    fn per_page_new_panics_on_zero() {
        let _ = super::PerPage::new(0);
    }

    /// [T-GH011c] PerPage::new panics on values over 100 (ADR-0004 Rule 2)
    #[test]
    #[should_panic(expected = "PerPage must be 1..=100")]
    fn per_page_new_panics_on_over_100() {
        let _ = super::PerPage::new(101);
    }

    /// [T-GH018] from_env_with_source threads the injected TokenSource through
    /// to the constructed client's token field. Proves the seam reaches the
    /// constructor without spawning `gh auth token`.
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

    /// [T-GH019] from_env_with_source propagates a None source so callers can
    /// simulate the unauthenticated path without env-var manipulation.
    #[tokio::test]
    async fn from_env_with_none_source_leaves_token_empty() {
        use crate::token_source::StaticTokenSource;
        let client =
            GitHubClient::from_env_with_source(Client::new(), 0, &StaticTokenSource(None)).await;
        assert!(client.token.is_none());
    }

    /// [T-GH006] get_json maps 500 responses to generic Api error
    #[tokio::test]
    async fn get_json_500_returns_api_error() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({"message": "internal server error"})),
            )
            .mount(&server)
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
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "30"))
            .mount(&server)
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

    /// [T-GH008] get_json_once uses x-ratelimit-reset to compute delay on 403 rate limit.
    /// Pinned clock + reset 60s later asserts the exact subtraction result.
    #[tokio::test]
    async fn get_json_403_with_ratelimit_reset_carries_delay() {
        let Some(server) = try_spawn_mock_server("github::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .append_header("x-ratelimit-reset", "1060")
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
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
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not valid json"))
            .mount(&server)
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

        let _ = handle.join();
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

    /// [T-GH015] secs_until_ratelimit_reset saturates to 0 when the injected
    /// clock has already passed the reset timestamp — `u64::saturating_sub`
    /// prevents an underflow that would otherwise wrap to a huge delay.
    #[test]
    fn secs_until_ratelimit_reset_saturates_when_clock_past_reset() {
        use reqwest::header::HeaderValue;
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("100"));
        let clock = FixedClock(200);
        assert_eq!(secs_until_ratelimit_reset(&headers, &clock), Some(0));
    }

    /// [T-GH016] secs_until_ratelimit_reset returns None when the
    /// x-ratelimit-reset header is absent; production callers then fall back
    /// to jittered backoff.
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
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("x-ratelimit-remaining", "0")
                    .append_header("x-ratelimit-reset", "1300")
                    .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
            )
            .mount(&server)
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
}

mod classify_tests {
    use super::*;

    /// [T-GHC001] Forbidden classifies as UsageError with GITHUB_TOKEN scope hint.
    #[test]
    fn forbidden_is_usage_error_with_scope_hint() {
        let c = GitHubError::Forbidden("denied".into()).classify();
        assert_eq!(c.kind, ErrorCode::UsageError);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("GITHUB_TOKEN")),
            "expected GITHUB_TOKEN hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-GHC002] Api { code: 401 } classifies as UsageError (priority 1 over 4xx fallback).
    /// Regression guard: a reorder that moves the 4xx arm above 401 would flip this to
    /// DataError(65) without `match` exhaustiveness catching it.
    #[test]
    fn api_401_is_usage_error_not_data_error() {
        let c = GitHubError::Api {
            code: 401,
            message: "Bad credentials".into(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::UsageError, "401 must precede 4xx arm");
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("gh auth login")),
            "expected gh auth login hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-GHC003] All priority-2 DataError variants classify as DataError.
    #[test]
    fn data_error_variants_classify_as_data_error() {
        let cases: Vec<GitHubError> = vec![
            GitHubError::InvalidRepo("bad".into()),
            GitHubError::InvalidRef("bad".into()),
            GitHubError::InvalidPath("bad".into()),
            GitHubError::InvalidLineRange("bad".into()),
            GitHubError::InvalidPattern("bad".into()),
            GitHubError::NonUtf8("bad".into()),
            GitHubError::InsecureUrl,
            GitHubError::Api {
                code: 400,
                message: "bad request".into(),
            },
            GitHubError::Api {
                code: 422,
                message: "unprocessable".into(),
            },
        ];
        for case in &cases {
            assert_eq!(
                case.classify().kind,
                ErrorCode::DataError,
                "{case:?} must classify as DataError"
            );
        }
    }

    /// [T-GHC004] NotFound classifies as NotFound with a "check the repo/path" hint.
    #[test]
    fn not_found_is_not_found_with_hint() {
        let c = GitHubError::NotFound("/x".into()).classify();
        assert_eq!(c.kind, ErrorCode::NotFound);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("repository or path")),
            "expected repo/path hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-GHC005] RateLimited with retry_after embeds the seconds into next_step.
    #[test]
    fn rate_limited_with_retry_after_embeds_seconds() {
        let c = GitHubError::RateLimited {
            retry_after: Some(42),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::TempFailure);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("42 seconds")),
            "expected 42 seconds in hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-GHC006] RateLimited without retry_after still suggests GITHUB_TOKEN.
    #[test]
    fn rate_limited_without_retry_after_suggests_token() {
        let c = GitHubError::RateLimited { retry_after: None }.classify();
        assert_eq!(c.kind, ErrorCode::TempFailure);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("GITHUB_TOKEN")),
            "expected GITHUB_TOKEN hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-GHC007] Api 5xx classifies as TempFailure (priority 4 over Unknown fallback).
    #[test]
    fn api_5xx_is_temp_failure() {
        for code in [500u16, 502, 503, 599] {
            let c = GitHubError::Api {
                code,
                message: "x".into(),
            }
            .classify();
            assert_eq!(c.kind, ErrorCode::TempFailure, "code {code}");
        }
    }

    /// [T-GHC008] Decode (schema drift) classifies as Internal per ADR-0011 priority 5.
    #[test]
    fn decode_is_internal() {
        let c = GitHubError::Decode("schema mismatch".into()).classify();
        assert_eq!(c.kind, ErrorCode::Internal);
    }

    /// [T-GHC009] Api codes outside 4xx/5xx (e.g., 1xx/3xx leak) land on Unknown.
    #[test]
    fn api_non_4xx_5xx_is_unknown() {
        let c = GitHubError::Api {
            code: 304,
            message: "not modified".into(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::Unknown);
    }
}
