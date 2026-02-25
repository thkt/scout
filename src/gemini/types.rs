use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    pub tools: Vec<Tool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Content {
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Part {
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Tool {
    pub google_search: GoogleSearch,
}

#[derive(Debug, Serialize)]
pub struct GoogleSearch {}

#[derive(Debug, Deserialize)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<Candidate>>,
    pub error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: Option<Content>,
    pub grounding_metadata: Option<GroundingMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingMetadata {
    pub grounding_chunks: Option<Vec<GroundingChunk>>,
}

#[derive(Debug, Deserialize)]
pub struct GroundingChunk {
    pub web: Option<WebChunk>,
}

#[derive(Debug, Deserialize)]
pub struct WebChunk {
    pub uri: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    pub code: Option<u16>,
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct GroundedResult {
    pub answer: String,
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub url: String,
    pub title: String,
}
