mod errors;
mod params;

pub use params::{
    FetchParams, RepoOverviewParams, RepoReadParams, RepoTreeParams, ResearchParams, SearchParams,
};

use std::time::Duration;

use reqwest::Client;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use tracing::{info, warn};

use errors::{
    fetch_to_mcp_error, gemini_to_mcp_error, github_to_mcp_error, parse_repo_param,
    unwrap_or_note,
};

use crate::fetch::TokioDnsResolver;
use crate::gemini::client::{GeminiClient, GeminiError, SearchClient};
use crate::github::{self, GitHubClient};
use crate::markdown::escape_md_link;
use crate::search::engine;

/// TCP connection establishment timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Global HTTP client timeout covering DNS + connect + response body.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Tool-level timeout for fetch operations (SSRF check + download + extraction).
const FETCH_TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum redirect hops before aborting.
const MAX_REDIRECTS: usize = 5;
const OVERVIEW_ITEMS: u8 = 5;
const OVERVIEW_RELEASES: u8 = 3;

/// MCP server handler providing search, fetch, and GitHub tools.
///
/// Configuration via environment variables:
/// - `GEMINI_API_KEY`: enables search/research tools (optional)
/// - `GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token`: GitHub API auth (optional)
#[derive(Clone)]
pub struct Scout {
    http: Client,
    gemini: Option<GeminiClient>,
    github: GitHubClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Scout {
    pub async fn new() -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()?;
        let gemini = GeminiClient::from_env(http.clone())
            .inspect_err(|e| warn!("Gemini client not available: {e}"))
            .ok();
        let github = GitHubClient::from_env(http.clone()).await;
        Ok(Self {
            http,
            gemini,
            github,
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

        let search_query = params
            .lang
            .unwrap_or_default()
            .apply_to_query(&params.query);

        let result = gemini
            .search(&search_query)
            .await
            .map_err(gemini_to_mcp_error)?;

        let mut output = result.answer.unwrap_or_else(|| {
            "(No answer returned — the query may have been filtered by safety settings.)"
                .to_string()
        });

        if !result.sources.is_empty() {
            output.push_str("\n\n---\n**Sources:**\n");
            for source in &result.sources {
                output.push_str(&format!(
                    "- [{}]({})\n",
                    escape_md_link(&source.title),
                    escape_md_link(&source.url)
                ));
            }
        }

        info!(sources = result.sources.len(), "search complete");
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        name = "fetch",
        description = "Fetch a web page and convert it to clean Markdown. For GitHub repository URLs (github.com/owner/repo/...), prefer repo_read, repo_tree, or repo_overview instead — they use the GitHub API and return structured, accurate results. Use this tool for non-GitHub URLs. Uses Readability algorithm to extract main content, removing ads and navigation. No AI/LLM round-trip; you can analyze the returned Markdown yourself."
    )]
    async fn fetch(
        &self,
        Parameters(params): Parameters<FetchParams>,
    ) -> Result<CallToolResult, McpError> {
        if !params.url.starts_with("http://") && !params.url.starts_with("https://") {
            return Err(McpError::invalid_params(
                "URL must use http or https scheme",
                None,
            ));
        }

        info!(url = %params.url, "tool:fetch");

        let raw = params.raw.unwrap_or(false);
        let meta = params.meta.unwrap_or(false);

        let result = tokio::time::timeout(
            FETCH_TOOL_TIMEOUT,
            crate::fetch::fetch_page(&self.http, &params.url, raw, meta, &TokioDnsResolver),
        )
        .await
        .unwrap_or_else(|_| {
            Err(crate::fetch::FetchError::Timeout(format!(
                "fetch timed out after {}s",
                FETCH_TOOL_TIMEOUT.as_secs()
            )))
        })
        .map_err(fetch_to_mcp_error)?;

        let mut output = if result.used_raw_fallback {
            warn!(url = %params.url, "readability extraction failed, using raw fallback");
            format!(
                "> Note: Readability extraction failed. Showing raw page conversion.\n\n{}",
                result.markdown
            )
        } else {
            result.markdown
        };

        const MAX_FETCH_OUTPUT_CHARS: usize = 100_000;
        if output.len() > MAX_FETCH_OUTPUT_CHARS {
            let end = output.floor_char_boundary(MAX_FETCH_OUTPUT_CHARS);
            output.truncate(end);
            output.push_str("\n\n(truncated)");
        }

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

        let req = engine::ResearchRequest {
            query: &params.query,
            depth,
            lang,
        };
        let report = engine::research(gemini, &self.http, &req, &TokioDnsResolver)
            .await
            .map_err(gemini_to_mcp_error)?;

        info!(
            pages = report.fetched_pages.len(),
            failed = report.failed_urls.len(),
            sources = report.all_sources.len(),
            "research complete"
        );

        let output = engine::format_report(&report, &params.query);

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        name = "repo_tree",
        description = "List files in a remote GitHub repository. Returns the file tree with optional path prefix and glob pattern filtering. Use this to explore a repository's structure before reading specific files."
    )]
    async fn repo_tree(
        &self,
        Parameters(params): Parameters<RepoTreeParams>,
    ) -> Result<CallToolResult, McpError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, "tool:repo_tree");

        let ref_ = match params.ref_ {
            Some(r) => {
                github::validate_ref(&r).map_err(github_to_mcp_error)?;
                r
            }
            None => {
                self.github
                    .get_repo(owner, repo)
                    .await
                    .map_err(github_to_mcp_error)?
                    .default_branch
            }
        };

        if let Some(ref p) = params.path {
            github::validate_path(p).map_err(github_to_mcp_error)?;
        }

        let tree = self
            .github
            .get_tree(owner, repo, &ref_)
            .await
            .map_err(github_to_mcp_error)?;

        let filtered = github::filter_tree_entries(
            &tree.tree,
            params.path.as_deref(),
            params.pattern.as_deref(),
        )
        .map_err(github_to_mcp_error)?;

        let output = github::format::format_tree(owner, repo, &ref_, &filtered, tree.truncated);

        info!(files = filtered.len(), "repo_tree complete");
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        name = "repo_read",
        description = "Read a file from a remote GitHub repository. Returns file content with optional line range selection (e.g., '1-80', '50-', '100'). Supports large files via git blob fallback."
    )]
    async fn repo_read(
        &self,
        Parameters(params): Parameters<RepoReadParams>,
    ) -> Result<CallToolResult, McpError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, path = %params.path, "tool:repo_read");

        github::validate_path(&params.path).map_err(github_to_mcp_error)?;
        if let Some(ref r) = params.ref_ {
            github::validate_ref(r).map_err(github_to_mcp_error)?;
        }

        let contents = self
            .github
            .get_contents(owner, repo, &params.path, params.ref_.as_deref())
            .await
            .map_err(github_to_mcp_error)?;

        let raw = if let Some(ref encoded) = contents.content {
            github::decode_content(encoded).map_err(github_to_mcp_error)?
        } else {
            let blob = self
                .github
                .get_blob(owner, repo, &contents.sha)
                .await
                .map_err(github_to_mcp_error)?;
            github::decode_content(&blob.content).map_err(github_to_mcp_error)?
        };

        let total = raw.lines().count();
        let content = if let Some(ref range) = params.lines {
            let (start, end) = github::parse_line_range(range).map_err(github_to_mcp_error)?;
            github::apply_line_range(&raw, start, end)
        } else {
            github::apply_line_range(&raw, 1, None)
        };

        let output = format!("{} ({total} lines)\n\n{content}", params.path);

        info!(path = %params.path, lines = total, "repo_read complete");
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        name = "repo_overview",
        description = "Get a comprehensive overview of a remote GitHub repository: metadata (stars, language, topics), README content, recent open issues, pull requests, and releases. Use this as the starting point when investigating a repository."
    )]
    async fn repo_overview(
        &self,
        Parameters(params): Parameters<RepoOverviewParams>,
    ) -> Result<CallToolResult, McpError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, "tool:repo_overview");

        let (repo_info, readme, issues, pulls, releases) = tokio::join!(
            self.github.get_repo(owner, repo),
            self.github.get_readme(owner, repo),
            self.github.get_issues(owner, repo, OVERVIEW_ITEMS),
            self.github.get_pulls(owner, repo, OVERVIEW_ITEMS),
            self.github.get_releases(owner, repo, OVERVIEW_RELEASES),
        );

        let repo_info = repo_info.map_err(github_to_mcp_error)?;

        let mut notes = Vec::new();

        let readme_content = match readme {
            Ok(r) => r.content.and_then(|c| match github::decode_content(&c) {
                Ok(content) => Some(content),
                Err(e) => {
                    warn!(%e, "failed to decode README");
                    notes.push(format!("README could not be decoded ({e})"));
                    None
                }
            }),
            Err(e) => {
                if !matches!(e, github::GitHubError::NotFound(_)) {
                    warn!(%e, "failed to fetch README");
                    notes.push(format!("Could not fetch README ({e})"));
                }
                None
            }
        };
        let issues = unwrap_or_note(issues, "issues", &mut notes);
        let pulls = unwrap_or_note(pulls, "pull requests", &mut notes);
        let releases = unwrap_or_note(releases, "releases", &mut notes);

        let mut output = github::format::format_overview(
            &repo_info,
            readme_content.as_deref(),
            &issues,
            &pulls,
            &releases,
        );

        if !notes.is_empty() {
            output.push_str("\n> **Note:** ");
            output.push_str(&notes.join(". "));
            output.push_str(".\n");
        }

        info!(
            issues = issues.len(),
            pulls = pulls.len(),
            releases = releases.len(),
            has_readme = readme_content.is_some(),
            "repo_overview complete"
        );
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

#[tool_handler]
impl ServerHandler for Scout {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "scout".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: Some(
                "scout provides web search (via Gemini Grounding), page fetching (local HTML→Markdown conversion), and GitHub repository exploration (repo_tree, repo_read, repo_overview) tools."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Lang;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_http_client() -> Client {
        Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .unwrap()
    }

    fn scout() -> Scout {
        let http = test_http_client();
        Scout {
            http: http.clone(),
            gemini: None,
            github: GitHubClient::with_base_url(http, "http://localhost:0"),
            tool_router: Scout::tool_router(),
        }
    }

    fn scout_with_gemini(gemini_uri: &str) -> Scout {
        let http = test_http_client();
        Scout {
            http: http.clone(),
            gemini: Some(GeminiClient::with_base_url(http.clone(), gemini_uri)),
            github: GitHubClient::with_base_url(http, "http://localhost:0"),
            tool_router: Scout::tool_router(),
        }
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
        let s = scout();
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

    #[tokio::test]
    async fn repo_tree_rejects_invalid_repo() {
        let s = scout();
        let params = Parameters(RepoTreeParams {
            repository: "invalid".into(),
            ref_: None,
            path: None,
            pattern: None,
        });
        let err = s.repo_tree(params).await.unwrap_err();
        assert!(err.message.contains("owner/repo"), "got: {}", err.message);
    }

    #[tokio::test]
    async fn repo_read_rejects_invalid_repo() {
        let s = scout();
        let params = Parameters(RepoReadParams {
            repository: "invalid".into(),
            path: "README.md".into(),
            ref_: None,
            lines: None,
        });
        let err = s.repo_read(params).await.unwrap_err();
        assert!(err.message.contains("owner/repo"), "got: {}", err.message);
    }

    #[tokio::test]
    async fn repo_overview_rejects_invalid_repo() {
        let s = scout();
        let params = Parameters(RepoOverviewParams {
            repository: "".into(),
        });
        let err = s.repo_overview(params).await.unwrap_err();
        assert!(err.message.contains("owner/repo"), "got: {}", err.message);
    }

    #[tokio::test]
    async fn search_success_returns_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "Rust is a systems programming language."}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": [{
                            "web": {
                                "uri": "https://rust-lang.org",
                                "title": "Rust"
                            }
                        }]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_with_gemini(&server.uri());
        let params = Parameters(SearchParams {
            query: "What is Rust?".into(),
            lang: None,
        });

        let result = s.search(params).await.unwrap();
        assert!(!result.content.is_empty());
    }

    #[tokio::test]
    async fn fetch_rejects_non_http_scheme() {
        let s = scout();
        for url in [
            "ftp://example.com",
            "javascript:alert(1)",
            "data:text/html,<h1>hi</h1>",
        ] {
            let params = Parameters(FetchParams {
                url: url.into(),
                raw: None,
                meta: None,
            });
            let err = s.fetch(params).await.unwrap_err();
            assert!(
                err.message.contains("http or https"),
                "should reject {url}, got: {}",
                err.message
            );
        }
    }

    #[tokio::test]
    async fn research_success_returns_report() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "Rust is a systems programming language focused on safety."}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": [{
                            "web": {
                                "uri": "https://rust-lang.org",
                                "title": "Rust Language"
                            }
                        }]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_with_gemini(&server.uri());
        let params = Parameters(ResearchParams {
            query: "What is Rust?".into(),
            depth: Some(1),
            lang: None,
        });

        let result = s.research(params).await.unwrap();
        let text = &result.content[0].as_text().unwrap().text;
        assert!(text.contains("Rust"), "report should contain search answer, got: {text}");
        assert!(text.contains("rust-lang.org"), "report should reference source URL");
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
