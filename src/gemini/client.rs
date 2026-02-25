use std::env;

use reqwest::Client;
use tracing::{debug, warn};

use super::grounding::extract_grounded_result;
use super::types::{
    ApiError, Content, GenerateContentRequest, GenerateContentResponse, GoogleSearch,
    GroundedResult, Part, Tool,
};

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-2.5-flash";

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
}

impl GeminiClient {
    pub fn new(http: Client, api_key: String) -> Self {
        Self {
            http,
            api_key: ApiKey(api_key),
            model: DEFAULT_MODEL.to_string(),
        }
    }

    pub fn from_env(http: Client) -> Result<Self, GeminiError> {
        let api_key = env::var("GEMINI_API_KEY").map_err(|_| GeminiError::ApiKeyNotSet)?;
        if api_key.trim().is_empty() {
            return Err(GeminiError::ApiKeyNotSet);
        }
        Ok(Self::new(http, api_key))
    }

    pub async fn generate_with_search(
        &self,
        query: &str,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let url = format!("{}/{}:generateContent", API_BASE, self.model);

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

        let response = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key.0)
            .header("User-Agent", crate::USER_AGENT)
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Gemini API rate limited");
            return Err(GeminiError::RateLimited);
        }
        if !status.is_success() {
            warn!(status = %status, "Gemini API error");
            if let Ok(body) = response.json::<GenerateContentResponse>().await
                && let Some(err) = &body.error
            {
                let classified = classify_api_error(err);
                warn!(error = %classified, "Gemini API error in response body");
                return Err(classified);
            }
            return Err(GeminiError::Api {
                code: status.as_u16(),
                message: format!("HTTP {status}"),
            });
        }

        let body: GenerateContentResponse = response.json().await?;
        debug!(model = %self.model, "gemini search complete");

        if let Some(err) = &body.error {
            let classified = classify_api_error(err);
            warn!(error = %classified, "Gemini API error in response body");
            return Err(classified);
        }

        Ok(body)
    }
}

impl SearchClient for GeminiClient {
    async fn search(&self, query: &str) -> Result<GroundedResult, GeminiError> {
        let response = self.generate_with_search(query).await?;
        Ok(extract_grounded_result(&response))
    }
}

fn classify_api_error(err: &ApiError) -> GeminiError {
    let code = err.code.unwrap_or(0);
    let message = err
        .message
        .clone()
        .unwrap_or_else(|| "Unknown error".to_string());

    match code {
        429 => GeminiError::RateLimited,
        403 => GeminiError::QuotaExhausted(message),
        _ => GeminiError::Api { code, message },
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
