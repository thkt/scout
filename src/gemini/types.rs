use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentRequest {
    pub(crate) contents: Vec<Content>,
    pub(crate) tools: Vec<Tool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Content {
    pub(crate) parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Part {
    pub(crate) text: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Tool {
    pub(crate) google_search: GoogleSearch,
}

#[derive(Debug, Serialize)]
pub(crate) struct GoogleSearch {}

#[derive(Debug, Deserialize)]
pub(crate) struct GenerateContentResponse {
    pub(crate) candidates: Option<Vec<Candidate>>,
    pub(crate) error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    pub(crate) content: Option<Content>,
    pub(crate) grounding_metadata: Option<GroundingMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroundingMetadata {
    pub(crate) grounding_chunks: Option<Vec<GroundingChunk>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GroundingChunk {
    pub(crate) web: Option<WebChunk>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebChunk {
    pub(crate) uri: Option<String>,
    pub(crate) title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiError {
    pub(crate) code: Option<u16>,
    pub(crate) message: Option<String>,
}

/// LLM answer with grounding sources from Google Search.
#[derive(Debug)]
pub(crate) struct GroundedResult {
    pub(crate) answer: Option<String>,
    pub(crate) sources: Vec<Source>,
}

#[derive(Debug, Clone)]
pub(crate) struct Source {
    pub(crate) url: String,
    pub(crate) title: String,
}
