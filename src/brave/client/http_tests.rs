use super::*;
use crate::test_support::try_spawn_mock_server;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, ResponseTemplate};

fn ok_body() -> serde_json::Value {
    serde_json::json!({
        "web": {
            "results": [
                {"url": "https://example.com", "title": "Example", "description": "snippet"}
            ]
        }
    })
}

/// [T-BC-LOG001] (issue #166 / OPS-003)
/// Setup: wiremock returns a 1-result Brave payload.
/// Action: `client.search("foo", None)` is invoked under `traced_test`.
/// Expected: an INFO-level `Brave search dispatching` event fires before
/// dispatch, and an INFO-level `Brave search complete` event fires after,
/// carrying `result_count` and `elapsed_ms` structured fields. Operators
/// at the default `info` log level can attribute latency without enabling
/// `RUST_LOG=debug`.
#[tracing_test::traced_test]
#[tokio::test]
async fn search_emits_info_dispatch_and_complete_events() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    client.search("foo", None).await.unwrap();

    assert!(
        logs_contain("Brave search dispatching"),
        "expected INFO dispatch event before the HTTP call"
    );
    assert!(
        logs_contain("Brave search complete"),
        "expected INFO completion event after the HTTP call"
    );
    assert!(
        logs_contain("result_count=1"),
        "completion event should carry result_count"
    );
    assert!(
        logs_contain("elapsed_ms"),
        "completion event should carry elapsed_ms for latency attribution"
    );
}

/// [T-BC001]
#[tokio::test]
async fn search_sends_query_unmodified() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", "foo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].url, "https://example.com");
}

/// [T-BC002] BraveClient includes search_lang=ja when Lang::Ja maps to "ja"
#[tokio::test]
async fn search_includes_search_lang_ja() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(query_param("q", "foo"))
        .and(query_param("search_lang", "ja"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", Some("ja")).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

/// [T-BC003] BraveClient includes search_lang=en when Lang::En maps to "en"
#[tokio::test]
async fn search_includes_search_lang_en() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(query_param("q", "foo"))
        .and(query_param("search_lang", "en"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", Some("en")).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

/// [T-BC004] BraveClient omits search_lang when None is provided
#[tokio::test]
async fn search_omits_search_lang_for_auto() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(query_param("q", "foo"))
        .and(query_param_is_missing("search_lang"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

/// [T-BC005] BraveClient sends X-Subscription-Token header with api key
#[tokio::test]
async fn search_sends_subscription_token_header() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(header("X-Subscription-Token", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

/// [T-BC006] search recovers when 429 transient response is followed by 200
#[tokio::test]
async fn search_retries_after_429_then_succeeds() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };

    // First call: 429 with short Retry-After to keep test fast
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // Subsequent calls: 200
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await.unwrap();
    assert_eq!(result.len(), 1);
}

/// [T-BC007] search returns RateLimited when 429 persists across retries
#[tokio::test]
async fn search_429_persistent_returns_rate_limited() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(
        matches!(result, Err(BraveError::RateLimited { .. })),
        "expected RateLimited, got: {result:?}"
    );
}

/// [T-BC008] search returns Unauthorized without retry on 401
#[tokio::test]
async fn search_401_returns_unauthorized() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1) // exactly one call, no retries
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(
        matches!(result, Err(BraveError::Unauthorized)),
        "expected Unauthorized, got: {result:?}"
    );
}

/// [T-BC026] (unit / FR-019)
/// Setup: wiremock always returns HTTP 403.
/// Action: `client.search("foo", None)` is invoked.
/// Expected: returns `BraveError::Unauthorized`; no retry (mock call count = 1)
/// because 403/401 are auth-class failures and not retriable.
#[tokio::test]
async fn search_403_returns_unauthorized() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1) // exactly one call, no retries
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(
        matches!(result, Err(BraveError::Unauthorized)),
        "expected Unauthorized for 403, got: {result:?}"
    );
}

/// [T-BC023] search returns ServerError(503) after retries on persistent 503
#[tokio::test]
async fn search_503_persistent_returns_server_error() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(
        matches!(result, Err(BraveError::Server(503))),
        "expected ServerError(503), got: {result:?}"
    );
}

/// [T-BC-CAP001] (issue #165 / CHX-008)
/// Setup: wiremock returns a 2xx whose body exceeds `MAX_API_RESPONSE_BYTES`
/// (1 MiB), simulating an upstream Brave deployment returning unbounded
/// JSON.
/// Action: `client.search("foo", None)` is invoked.
/// Expected: returns `BraveError::ResponseTooLarge`; no retry (mock call
/// count = 1) because the variant is not retriable.
#[tokio::test]
async fn search_oversized_body_returns_too_large() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    // 1 MiB + 1 byte trips the cap regardless of pre-check vs chunk path.
    let body = vec![b'x'; MAX_API_RESPONSE_BYTES + 1];
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    assert!(
        matches!(result, Err(BraveError::ResponseTooLarge)),
        "expected ResponseTooLarge, got: {result:?}"
    );
}

/// [T-BC024]
#[tokio::test]
async fn search_malformed_json_returns_parse_error() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"web\":"))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let result = client.search("foo", None).await;
    match result {
        Err(BraveError::ParseJson(e)) => {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "parse error message should not be empty");
            assert!(
                msg.contains("EOF") || msg.contains("expected"),
                "serde diagnostic expected (EOF/expected token), got: {msg}"
            );
        }
        other => panic!("expected ParseJson, got: {other:?}"),
    }
}

/// [T-BC-LOG002] (issue #189)
/// Setup: wiremock returns a 200 with a malformed JSON body.
/// Action: `client.search("foobar", None)` is invoked under `traced_test`.
/// Expected: a WARN-level `Brave search response parse failed` event fires,
/// carrying `query_len=6` and the serde `error` field, so operators can see a
/// schema-drift fallback without the raw query text leaking.
#[tracing_test::traced_test]
#[tokio::test]
async fn search_logs_warn_on_parse_failure() {
    let Some(server) = try_spawn_mock_server("brave::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"web\":"))
        .mount(&server)
        .await;

    let client = BraveClient::with_base_url(Client::new(), &server.uri());
    let _ = client.search("foobar", None).await;

    assert!(
        logs_contain("Brave search response parse failed"),
        "expected the parse-failure WARN event"
    );
    assert!(logs_contain("WARN"), "event level should be WARN");
    assert!(
        logs_contain("query_len=6"),
        "event should carry query_len (length, not the raw query)"
    );
    assert!(
        logs_contain("error="),
        "event should carry the serde error field"
    );
}

/// [T-RC001] FR-001 / FR-002: closure returning `Err(VarError::NotPresent)` must surface
/// as `BraveError::ApiKeyNotSet` from `from_env_with`. Exercises the injectable
/// env path that `from_env` delegates to.
#[test]
fn from_env_with_returns_api_key_not_set_when_closure_errs() {
    let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Err(env::VarError::NotPresent)
    });
    assert!(
        matches!(result, Err(BraveError::ApiKeyNotSet)),
        "expected ApiKeyNotSet, got: {result:?}"
    );
}

/// [T-005] BraveClient の from_env_with は未設定 env でそれぞれ既存の欠落エラーを返す
///
/// `Redacted::from_env_var` (shared with `SlackClient::from_env_with`, see the
/// companion T-005 in `slack/client/constructor_tests.rs`) does not know about
/// `BraveError`; this pins that going through the shared helper still surfaces
/// Brave's own `ApiKeyNotSet` for an unset env var.
#[test]
fn from_env_with_returns_existing_missing_error_via_shared_helper() {
    let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Err(env::VarError::NotPresent)
    });
    assert!(
        matches!(result, Err(BraveError::ApiKeyNotSet)),
        "expected the pre-existing ApiKeyNotSet error, got: {result:?}"
    );
}

/// [T-RC002] FR-003: whitespace-only keys stay rejected — parity with the previous
/// `trim().is_empty()` check in `from_env`.
#[test]
fn from_env_with_rejects_whitespace_only_key() {
    let result =
        BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| Ok("   ".to_owned()));
    assert!(
        matches!(result, Err(BraveError::ApiKeyNotSet)),
        "expected ApiKeyNotSet for whitespace-only key, got: {result:?}"
    );
}

/// [T-RC003] FR-001 / FR-003.
#[test]
fn from_env_with_constructs_client_with_api_base_and_exposed_key() {
    let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Ok("real-key".to_owned())
    });
    let client = result.expect("expected Ok(client) from valid key");
    assert_eq!(client.api_key.expose(), "real-key");
    assert_eq!(client.base_url, API_BASE);
}

/// [T-RC006] FR-010: production constructor path must not enable the test-only HTTPS bypass.
/// `skip_https_check` is a `#[cfg(test)]` field; under `cargo test` it exists and
/// must be `false` when the client comes from `from_env_with`.
#[test]
fn from_env_with_does_not_set_skip_https_check() {
    let client =
        BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| Ok("k".to_owned()))
            .expect("expected Ok(client) from valid key");
    assert!(
        !client.skip_https_check,
        "production constructor must not skip HTTPS check"
    );
}
