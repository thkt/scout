use std::env;
use std::time::Duration;

use reqwest::Client;
use tracing::{debug, warn};

use crate::redacted::{Redacted, assert_https};
use crate::retry::{
    is_transient_network, parse_retry_after, retry_after_or_backoff, retry_after_within_cap,
    retry_with,
};

use super::types::{SearchResult, WebSearchResponse};

const API_BASE: &str = "https://api.search.brave.com/res/v1/web/search";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const BODY_SNIPPET_BYTES: usize = 200;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BraveError {
    #[error("BRAVE_SEARCH_API_KEY not set. Get one at https://api-dashboard.search.brave.com/")]
    ApiKeyNotSet,

    #[error("API rate limit exceeded. Please retry later.")]
    RateLimited { retry_after: Option<u64> },

    #[error("Unauthorized: check your BRAVE_SEARCH_API_KEY")]
    Unauthorized,

    #[error("Brave API server error (HTTP {0})")]
    Server(u16),

    #[error("Failed to parse Brave API response: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("Invalid Brave API URL: {0}")]
    ParseUrl(#[from] url::ParseError),

    #[error("API error ({code}): {message}")]
    Api { code: u16, message: String },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
}

impl BraveError {
    /// Returns `true` when the error is a transient infrastructure failure that callers
    /// may legitimately surface as a degraded result instead of propagating.
    ///
    /// Configuration errors (`ApiKeyNotSet`, `Unauthorized`, `ParseUrl`) and data
    /// errors (`ParseJson`, `Api` 4xx) require user action and must not be silently
    /// swallowed.
    pub(crate) fn is_degradable(&self) -> bool {
        match self {
            Self::ApiKeyNotSet | Self::Unauthorized | Self::ParseJson(_) | Self::ParseUrl(_) => {
                false
            }
            Self::Api { code, .. } if (400..500).contains(code) => false,
            Self::RateLimited { .. } | Self::Server(_) | Self::Network(_) | Self::Api { .. } => {
                true
            }
        }
    }
}

pub(crate) trait SearchClient {
    async fn search(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<Vec<SearchResult>, BraveError>;
}

#[derive(Clone)]
pub(crate) struct BraveClient {
    http: Client,
    api_key: Redacted,
    base_url: String,
}

impl BraveClient {
    /// Constructs a `BraveClient` from the `BRAVE_SEARCH_API_KEY` env var.
    ///
    /// The env var path is verified end-to-end in the Phase 5a integration test (T-020).
    /// Unit tests cannot mutate env without `unsafe`, and scout is `unsafe_code = "forbid"`,
    /// so this constructor is not covered by unit tests.
    pub(crate) fn from_env(http: Client) -> Result<Self, BraveError> {
        let api_key = env::var("BRAVE_SEARCH_API_KEY").map_err(|_| BraveError::ApiKeyNotSet)?;
        if api_key.trim().is_empty() {
            return Err(BraveError::ApiKeyNotSet);
        }
        Ok(Self {
            http,
            api_key: Redacted::new(&api_key),
            base_url: API_BASE.to_owned(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            api_key: Redacted::new("test-key"),
            base_url: base_url.to_owned(),
        }
    }

    async fn send_request(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<WebSearchResponse, BraveError> {
        assert_https(&self.base_url);
        let url = build_url(&self.base_url, query, search_lang)?;

        let response = self
            .http
            .get(url)
            .header("X-Subscription-Token", self.api_key.expose())
            .header("Accept", "application/json")
            .header("User-Agent", crate::USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?;

        let response = classify_response(response).await?;
        let bytes = response.bytes().await?;
        let parsed: WebSearchResponse = serde_json::from_slice(&bytes)?;

        debug!(
            query_len = query.len(),
            result_count = parsed.web.as_ref().map_or(0, |w| w.results.len()),
            "Brave search complete"
        );
        Ok(parsed)
    }
}

fn build_url(
    base_url: &str,
    query: &str,
    search_lang: Option<&str>,
) -> Result<reqwest::Url, BraveError> {
    let mut params = vec![("q", query)];
    if let Some(lang) = search_lang {
        params.push(("search_lang", lang));
    }
    Ok(reqwest::Url::parse_with_params(base_url, &params)?)
}

async fn classify_response(response: reqwest::Response) -> Result<reqwest::Response, BraveError> {
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = parse_retry_after(response.headers());
        warn!(retry_after_secs = retry_after, "Brave API rate limited");
        return Err(BraveError::RateLimited { retry_after });
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        warn!(status = %status, "Brave API unauthorized");
        return Err(BraveError::Unauthorized);
    }
    if status.is_server_error() {
        warn!(status = %status, "Brave API server error");
        return Err(BraveError::Server(status.as_u16()));
    }
    if !status.is_success() {
        let mut text = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                warn!(status = %status, error = %e, "Brave API error; body unreadable");
                "(body unreadable)".to_owned()
            }
        };
        if text.len() > BODY_SNIPPET_BYTES {
            let mut end = BODY_SNIPPET_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        warn!(status = %status, "Brave API error");
        return Err(BraveError::Api {
            code: status.as_u16(),
            message: format!("HTTP {status}: {text}"),
        });
    }
    Ok(response)
}

impl SearchClient for BraveClient {
    async fn search(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<Vec<SearchResult>, BraveError> {
        let response = retry_with(
            || self.send_request(query, search_lang),
            is_retriable,
            brave_delay,
            || BraveError::RateLimited { retry_after: None },
        )
        .await?;
        Ok(response.into_results())
    }
}

fn is_retriable(e: &BraveError) -> bool {
    match e {
        BraveError::RateLimited { retry_after } => retry_after_within_cap(*retry_after),
        BraveError::Server(_) => true,
        BraveError::Network(e) => is_transient_network(e),
        _ => false,
    }
}

fn brave_delay(e: &BraveError, attempt: u32) -> Duration {
    let retry_after = match e {
        BraveError::RateLimited { retry_after } => *retry_after,
        _ => None,
    };
    retry_after_or_backoff(retry_after, attempt)
}

#[cfg(test)]
mod http_tests {
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

    /// [T-001] BraveClient sends query unmodified with q parameter
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

    /// [T-002] BraveClient includes search_lang=ja when Lang::Ja maps to "ja"
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

    /// [T-003] BraveClient includes search_lang=en when Lang::En maps to "en"
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

    /// [T-004] BraveClient omits search_lang when None is provided
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

    /// [T-005] BraveClient sends X-Subscription-Token header with api key
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

    /// [T-006] search recovers when 429 transient response is followed by 200
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

    /// [T-007] search returns RateLimited when 429 persists across retries
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

    /// [T-008] search returns Unauthorized without retry on 401
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

    /// [T-026] (unit / FR-019)
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

    /// [T-023] search returns ServerError(503) after retries on persistent 503
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

    /// [T-024] search returns ParseJson error when response body is malformed JSON
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
}
