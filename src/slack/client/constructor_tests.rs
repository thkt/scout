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
