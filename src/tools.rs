use std::time::Duration;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
};
use reqwest::Client;
use schemars::JsonSchema;
use serde::Deserialize;

use tracing::{info, warn};

use crate::gemini::client::{GeminiClient, GeminiError, SearchClient};
use crate::search::engine;

#[derive(Deserialize, JsonSchema, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ja,
    En,
    #[default]
    Auto,
}

impl Lang {
    pub fn apply_to_query(self, query: &str) -> String {
        match self {
            Lang::Ja => format!("{query} (日本語で回答)"),
            Lang::En => format!("{query} (answer in English)"),
            Lang::Auto => query.to_string(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query
    pub query: String,
    /// Search language: "ja", "en", or "auto" (default)
    pub lang: Option<Lang>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FetchParams {
    /// URL to fetch (must be HTTP or HTTPS)
    pub url: String,
    /// Skip Readability extraction and convert entire page (default: false)
    pub raw: Option<bool>,
    /// Include page metadata (title, author, date) as YAML frontmatter (default: false)
    pub meta: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResearchParams {
    /// Research query
    pub query: String,
    /// Number of URLs to fetch for deep analysis (1-10, default: 3)
    pub depth: Option<u8>,
    /// Search language: "ja", "en", or "auto" (default)
    pub lang: Option<Lang>,
}

#[derive(Clone)]
pub struct Scout {
    http: Client,
    gemini: Option<GeminiClient>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Scout {
    pub fn new() -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        let gemini = GeminiClient::from_env(http.clone()).ok();
        Ok(Self {
            http,
            gemini,
            tool_router: Self::tool_router(),
        })
    }

    fn gemini(&self) -> Result<&GeminiClient, McpError> {
        self.gemini
            .as_ref()
            .ok_or_else(|| gemini_to_mcp_error(GeminiError::ApiKeyNotSet))
    }

    #[tool(
        name = "search",
        description = "Search the web using Gemini Grounding with Google Search. Returns an AI-generated answer with source URLs. Use this for factual queries, current events, documentation lookups, and technical research."
    )]
    async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.query.is_empty() {
            return Err(McpError::invalid_params("query must not be empty", None));
        }

        info!(query = %params.query, "tool:search");

        let gemini = self.gemini()?;

        let search_query = params.lang.unwrap_or_default().apply_to_query(&params.query);

        let result = gemini
            .search(&search_query)
            .await
            .map_err(gemini_to_mcp_error)?;

        let mut output = if result.answer.is_empty() {
            "(No answer returned — the query may have been filtered by safety settings.)".to_string()
        } else {
            result.answer
        };

        if !result.sources.is_empty() {
            output.push_str("\n\n---\n**Sources:**\n");
            for source in &result.sources {
                output.push_str(&format!("- [{}]({})\n", source.title, source.url));
            }
        }

        info!(sources = result.sources.len(), "search complete");
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        name = "fetch",
        description = "Fetch a web page and convert it to clean Markdown. Prefer this over WebFetch when you need to read a URL's content — it is faster, free, and more accurate for code blocks and tables. Uses Readability algorithm to extract main content, removing ads and navigation. No AI/LLM round-trip; you can analyze the returned Markdown yourself."
    )]
    async fn fetch(
        &self,
        Parameters(params): Parameters<FetchParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(url = %params.url, "tool:fetch");

        let raw = params.raw.unwrap_or(false);
        let meta = params.meta.unwrap_or(false);

        let result = crate::fetch::fetch_page(&self.http, &params.url, raw, meta)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let output = if result.used_raw_fallback {
            warn!(url = %params.url, "readability extraction failed, using raw fallback");
            format!(
                "> Note: Readability extraction failed. Showing raw page conversion.\n\n{}",
                result.markdown
            )
        } else {
            result.markdown
        };

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        name = "research",
        description = "Deep research: search the web, fetch top results, and compile a comprehensive report with sources. Combines Gemini search with local page fetching for thorough investigation. Use for complex questions requiring multiple sources."
    )]
    async fn research(
        &self,
        Parameters(params): Parameters<ResearchParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.query.is_empty() {
            return Err(McpError::invalid_params("query must not be empty", None));
        }

        let depth = params.depth.unwrap_or(3).clamp(1, 10);
        let lang = params.lang.unwrap_or_default();

        info!(query = %params.query, depth, "tool:research");

        let gemini = self.gemini()?;

        let report = engine::research(gemini, &self.http, &params.query, depth, lang)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        info!(
            pages = report.fetched_pages.len(),
            failed = report.failed_urls.len(),
            sources = report.all_sources.len(),
            "research complete"
        );

        let output = engine::format_report(&report, &params.query);

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

#[tool_handler]
impl ServerHandler for Scout {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "scout provides web search (via Gemini Grounding) and page fetching (local HTML→Markdown conversion) tools."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

fn gemini_to_mcp_error(e: GeminiError) -> McpError {
    match &e {
        GeminiError::RateLimited => McpError::internal_error(
            format!("{e} (retriable)"),
            None,
        ),
        _ => McpError::internal_error(e.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scout() -> Scout {
        Scout::new().unwrap()
    }

    #[tokio::test]
    async fn search_rejects_empty_query() {
        let s = scout();
        let params = Parameters(SearchParams {
            query: String::new(),
            lang: None,
        });

        let err = s.search(params).await.unwrap_err();
        assert!(err.message.contains("empty"), "got: {}", err.message);
    }

    #[tokio::test]
    async fn research_rejects_empty_query() {
        let s = scout();
        let params = Parameters(ResearchParams {
            query: String::new(),
            depth: None,
            lang: None,
        });

        let err = s.research(params).await.unwrap_err();
        assert!(err.message.contains("empty"), "got: {}", err.message);
    }

    #[tokio::test]
    async fn search_without_api_key_returns_error() {
        let s = Scout {
            http: Client::new(),
            gemini: None,
            tool_router: Scout::tool_router(),
        };
        let params = Parameters(SearchParams {
            query: "test query".into(),
            lang: None,
        });

        let err = s.search(params).await.unwrap_err();
        assert!(
            err.message.contains("GEMINI_API_KEY"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn lang_deserializes_from_json() {
        let ja: Lang = serde_json::from_str(r#""ja""#).unwrap();
        assert!(matches!(ja, Lang::Ja));

        let en: Lang = serde_json::from_str(r#""en""#).unwrap();
        assert!(matches!(en, Lang::En));

        let auto: Lang = serde_json::from_str(r#""auto""#).unwrap();
        assert!(matches!(auto, Lang::Auto));
    }
}
