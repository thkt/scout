use super::*;
use crate::test_support::try_spawn_mock_server;
use reqwest::Client;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-SK025] SlackClient::with_base_url constructs a client that reaches a wiremock server
#[tokio::test]
async fn client_with_base_url_reaches_that_server() {
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

/// [T-SK033] A token neither the env var nor the Keychain held surfaces as
/// `SlackError::TokenNotSet`.
#[test]
fn missing_token_is_rejected_as_token_not_set() {
    // `.map(|_| ())` drops the `SlackClient` (no `Debug`) so the failure
    // message can format the `Result`.
    let result = SlackClient::from_secret(Client::new(), DEFAULT_MAX_RETRIES, None).map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenNotSet)),
        "expected TokenNotSet, got: {result:?}"
    );
}

/// [T-SK035]
#[test]
fn user_token_builds_a_client_on_the_slack_api_base() {
    let result = SlackClient::from_secret(
        Client::new(),
        DEFAULT_MAX_RETRIES,
        Redacted::new("xoxp-real"),
    );
    let client = result.expect("expected Ok(client) from valid token");
    assert_eq!(client.token.expose(), "xoxp-real");
    assert_eq!(client.base_url, API_BASE);
}

/// [T-SK065] The `SLACK_TOKEN must be a User OAuth token` contract is enforced
/// at construction, so a bot token cannot pass through to fail later with an
/// opaque API error. A token read from the Keychain goes through the same check.
#[test]
fn bot_token_is_rejected_as_wrong_type() {
    let result = SlackClient::from_secret(
        Client::new(),
        DEFAULT_MAX_RETRIES,
        Redacted::new("xoxb-bot-token"),
    )
    .map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenWrongType)),
        "expected TokenWrongType for bot token, got: {result:?}"
    );
}

/// [T-SK066] from_secret rejects an arbitrary non-`xoxp-` string as
/// `TokenWrongType` — the prefix check covers any string outside Slack's
/// user-token taxonomy, not just bot tokens.
#[test]
fn token_without_user_prefix_is_rejected_as_wrong_type() {
    let result =
        SlackClient::from_secret(Client::new(), DEFAULT_MAX_RETRIES, Redacted::new("garbage"))
            .map(|_| ());
    assert!(
        matches!(result, Err(SlackError::TokenWrongType)),
        "expected TokenWrongType for arbitrary string, got: {result:?}"
    );
}
