use super::*;
use crate::test_support::try_spawn_mock_server;
use reqwest::Client;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-SK025] SlackClient::with_base_url constructs a client that reaches a wiremock server
#[tokio::test]
async fn t010_with_base_url_constructs_usable_client() {
    let Some(server) = try_spawn_mock_server("slack::http").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/auth.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(Client::new(), &server.uri());

    let result: Result<DummyBody, _> = client.api_get_once("auth.test", &[]).await;
    assert!(result.is_ok());
}

/// [T-SK033] from_env_with surfaces a closure `Err(VarError::NotPresent)` as
/// `SlackError::TokenNotSet` — the token-unset path that `unsafe_code = "forbid"`
/// blocks from being reached via `env::set_var` (ADR-0007, issue #191).
#[test]
fn t033_from_env_with_returns_token_not_set_when_closure_errs() {
    // `.map(|_| ())` drops the `SlackClient` (no `Debug`) so the failure
    // message can format the `Result`.
    let result = SlackClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Err(env::VarError::NotPresent)
    })
    .map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenNotSet)),
        "expected TokenNotSet, got: {result:?}"
    );
}

/// [T-SK034] from_env_with rejects a whitespace-only token as `TokenNotSet`
/// (parity with `Redacted::new` rejecting blank secrets).
#[test]
fn t034_from_env_with_rejects_whitespace_only_token() {
    let result =
        SlackClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| Ok("   ".to_owned()))
            .map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenNotSet)),
        "expected TokenNotSet for whitespace-only token, got: {result:?}"
    );
}

/// [T-SK035]
#[test]
fn t035_from_env_with_constructs_client_with_api_base_and_exposed_token() {
    let result = SlackClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Ok("xoxp-real".to_owned())
    });
    let client = result.expect("expected Ok(client) from valid token");
    assert_eq!(client.token.expose(), "xoxp-real");
    assert_eq!(client.base_url, API_BASE);
}

/// [T-SK065] The `SLACK_TOKEN must be a User OAuth token` contract is now
/// enforced at construction, so a bot token can no longer pass through to fail
/// later with an opaque API error (issue #261).
#[test]
fn t065_from_env_with_rejects_bot_token_as_wrong_type() {
    let result = SlackClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Ok("xoxb-bot-token".to_owned())
    })
    .map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenWrongType)),
        "expected TokenWrongType for bot token, got: {result:?}"
    );
}

/// [T-SK066] from_env_with rejects an arbitrary non-`xoxp-` string as
/// `TokenWrongType` — the prefix check covers any string outside Slack's
/// user-token taxonomy, not just bot tokens.
#[test]
fn t066_from_env_with_rejects_arbitrary_string_as_wrong_type() {
    let result = SlackClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Ok("garbage".to_owned())
    })
    .map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenWrongType)),
        "expected TokenWrongType for arbitrary string, got: {result:?}"
    );
}
