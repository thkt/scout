use std::env;
use std::time::Duration;

use reqwest::Client;
use tracing::{debug, warn};

use super::grounding::extract_grounded_result;
use super::types::{
    ApiError, Content, GenerateContentRequest, GenerateContentResponse, GoogleSearch,
    GroundedResult, Part, Tool,
};

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-2.5-flash";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum GeminiError {
    #[error("GEMINI_API_KEY not set. Get one at https://aistudio.google.com/apikey")]
    ApiKeyNotSet,

    #[error("API rate limit exceeded. Please retry later.")]
    RateLimited,

    #[error("API quota exhausted: {0}")]
    QuotaExhausted(String),

    #[error("API error ({code}): {message}")]
    Api { code: u16, message: String },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
}

/// Abstraction for web search via LLM with grounding.
/// Implemented by `GeminiClient` for production; mock implementations used in tests.
pub trait SearchClient {
    async fn search(&self, query: &str) -> Result<GroundedResult, GeminiError>;
}

#[derive(Clone)]
struct ApiKey(String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
pub struct GeminiClient {
    http: Client,
    api_key: ApiKey,
    model: String,
    base_url: String,
}

impl GeminiClient {
    pub fn from_env(http: Client) -> Result<Self, GeminiError> {
        let api_key = env::var("GEMINI_API_KEY").map_err(|_| GeminiError::ApiKeyNotSet)?;
        if api_key.trim().is_empty() {
            return Err(GeminiError::ApiKeyNotSet);
        }
        let model = env::var("GEMINI_MODEL")
            .ok()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(Self {
            http,
            api_key: ApiKey(api_key.trim().to_string()),
            model,
            base_url: API_BASE.to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            api_key: ApiKey("test-key".to_string()),
            model: DEFAULT_MODEL.to_string(),
            base_url: base_url.to_string(),
        }
    }

    async fn generate_with_search(
        &self,
        query: &str,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let url = format!("{}/{}:generateContent", self.base_url, self.model);

        let request = GenerateContentRequest {
            contents: vec![Content {
                parts: vec![Part {
                    text: query.to_string(),
                }],
                role: None,
            }],
            tools: vec![Tool {
                google_search: GoogleSearch {},
            }],
        };

        debug_assert!(
            url.starts_with("https://") || cfg!(test),
            "API key must only be sent over HTTPS"
        );

        let response = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key.0)
            .header("User-Agent", crate::USER_AGENT)
            .json(&request)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Gemini API rate limited");
            return Err(GeminiError::RateLimited);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            if let Ok(body) = serde_json::from_str::<GenerateContentResponse>(&text)
                && let Some(err) = &body.error
            {
                let classified = classify_api_error(err);
                warn!(error = %classified, "Gemini API error");
                return Err(classified);
            }
            let snippet = if text.len() > 200 { &text[..200] } else { &text };
            warn!(status = %status, "Gemini API error (no structured body)");
            return Err(GeminiError::Api {
                code: status.as_u16(),
                message: format!("HTTP {status}: {snippet}"),
            });
        }

        let body: GenerateContentResponse = response.json().await?;
        debug!(model = %self.model, "gemini search complete");

        if let Some(err) = &body.error {
            let classified = classify_api_error(err);
            warn!(error = %classified, "Gemini API error in 200 response");
            return Err(classified);
        }

        Ok(body)
    }
}

const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

impl SearchClient for GeminiClient {
    async fn search(&self, query: &str) -> Result<GroundedResult, GeminiError> {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            match self.generate_with_search(query).await {
                Ok(response) => return Ok(extract_grounded_result(&response)),
                Err(e) if is_retriable(&e) => {
                    last_err = Some(e);
                    if attempt + 1 < MAX_RETRIES {
                        let delay_ms = jittered_backoff(attempt);
                        debug!(
                            attempt = attempt + 1,
                            delay_ms, "retrying after transient error"
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or(GeminiError::RateLimited))
    }
}

fn is_retriable(e: &GeminiError) -> bool {
    matches!(
        e,
        GeminiError::RateLimited
            | GeminiError::Api {
                code: 500..=599,
                ..
            }
    )
}

/// Equal jitter backoff: base/2 + rand(0, base/2).
fn jittered_backoff(attempt: u32) -> u64 {
    let base = INITIAL_BACKOFF_MS * 2u64.pow(attempt);
    let half = base / 2;
    half + fastrand::u64(..half.max(1))
}

fn classify_api_error(err: &ApiError) -> GeminiError {
    let message = err
        .message
        .clone()
        .unwrap_or_else(|| "Unknown error".to_string());

    match err.code {
        Some(429) => GeminiError::RateLimited,
        Some(403) => GeminiError::QuotaExhausted(message),
        Some(code) => GeminiError::Api { code, message },
        None => GeminiError::Api {
            code: 0,
            message: format!("Unknown error (no status code): {message}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_429_as_rate_limited() {
        let err = ApiError {
            code: Some(429),
            message: Some("Resource exhausted".into()),
        };
        assert!(matches!(classify_api_error(&err), GeminiError::RateLimited));
    }

    #[test]
    fn classify_403_as_quota_exhausted() {
        let err = ApiError {
            code: Some(403),
            message: Some("Quota exceeded".into()),
        };
        assert!(matches!(
            classify_api_error(&err),
            GeminiError::QuotaExhausted(_)
        ));
    }

    #[test]
    fn classify_500_as_generic_api_error() {
        let err = ApiError {
            code: Some(500),
            message: Some("Internal server error".into()),
        };
        match classify_api_error(&err) {
            GeminiError::Api { code, message } => {
                assert_eq!(code, 500);
                assert_eq!(message, "Internal server error");
            }
            other => panic!("expected Api error, got: {other:?}"),
        }
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn search_success_returns_grounded_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "Test answer"}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": [{
                            "web": {
                                "uri": "https://example.com",
                                "title": "Example"
                            }
                        }]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let client = GeminiClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("test query").await.unwrap();

        assert_eq!(result.answer.as_deref(), Some("Test answer"));
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].url, "https://example.com");
    }

    #[tokio::test]
    async fn search_429_returns_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let client = GeminiClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("test").await;
        assert!(matches!(result, Err(GeminiError::RateLimited)));
    }

    #[tokio::test]
    async fn search_500_with_error_body_classified() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": {
                    "code": 500,
                    "message": "Internal server error"
                }
            })))
            .mount(&server)
            .await;

        let client = GeminiClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("test").await;
        match &result {
            Err(GeminiError::Api { code: 500, message }) => {
                assert!(message.contains("Internal server error"));
            }
            other => panic!("expected Api(500) with body message, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_500_with_invalid_body_returns_generic_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(500).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = GeminiClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("test").await;
        match &result {
            Err(GeminiError::Api { code: 500, message }) => {
                assert!(message.contains("not json"), "expected body snippet in error, got: {message}");
            }
            other => panic!("expected Api(500) without body, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_200_with_error_field_returns_classified_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 403,
                    "message": "Quota exceeded"
                }
            })))
            .mount(&server)
            .await;

        let client = GeminiClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("test").await;
        assert!(matches!(result, Err(GeminiError::QuotaExhausted(_))));
    }
}
