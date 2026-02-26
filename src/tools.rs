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
use crate::github::{self, GitHubClient};
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

#[derive(Deserialize, JsonSchema)]
pub struct RepoTreeParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
    /// Git ref: branch name, tag, or commit SHA (default: repository's default branch)
    #[serde(rename = "ref")]
    pub ref_: Option<String>,
    /// Filter to files under this path prefix (e.g., "src/components/")
    pub path: Option<String>,
    /// Glob pattern to filter filenames (e.g., "*.rs", "*.{ts,tsx}")
    pub pattern: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RepoReadParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
    /// File path within the repository (e.g., "src/index.ts")
    pub path: String,
    /// Git ref: branch name, tag, or commit SHA (default: repository's default branch)
    #[serde(rename = "ref")]
    pub ref_: Option<String>,
    /// Line range: "1-80" (lines 1 to 80), "50-" (line 50 to end), "100" (first 100 lines). Omit to read entire file.
    pub lines: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RepoOverviewParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
}

#[derive(Clone)]
pub struct Scout {
    http: Client,
    gemini: Option<GeminiClient>,
    github: GitHubClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Scout {
    pub fn new() -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        let gemini = GeminiClient::from_env(http.clone()).ok();
        let github = GitHubClient::from_env(http.clone());
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

    #[tool(
        name = "repo_tree",
        description = "List files in a remote GitHub repository. Returns the file tree with optional path prefix and glob pattern filtering. Use this to explore a repository's structure before reading specific files."
    )]
    async fn repo_tree(
        &self,
        Parameters(params): Parameters<RepoTreeParams>,
    ) -> Result<CallToolResult, McpError> {
        let (owner, repo) = github::parse_repo(&params.repository)
            .map_err(github_to_mcp_error)?;

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

        let tree = self
            .github
            .get_tree(owner, repo, &ref_)
            .await
            .map_err(github_to_mcp_error)?;

        if let Some(ref p) = params.path {
            github::validate_path(p).map_err(github_to_mcp_error)?;
        }

        let filtered = github::filter_tree_entries(
            &tree.tree,
            params.path.as_deref(),
            params.pattern.as_deref(),
        )
        .map_err(github_to_mcp_error)?;

        let output =
            github::format::format_tree(owner, repo, &ref_, &filtered, tree.truncated);

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
        let (owner, repo) = github::parse_repo(&params.repository)
            .map_err(github_to_mcp_error)?;

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
            let (start, end) =
                github::parse_line_range(range).map_err(github_to_mcp_error)?;
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
        let (owner, repo) = github::parse_repo(&params.repository)
            .map_err(github_to_mcp_error)?;

        info!(repository = %params.repository, "tool:repo_overview");

        let (repo_info, readme, issues, pulls, releases) = tokio::join!(
            self.github.get_repo(owner, repo),
            self.github.get_readme(owner, repo),
            self.github.get_issues(owner, repo, 5),
            self.github.get_pulls(owner, repo, 5),
            self.github.get_releases(owner, repo, 3),
        );

        let repo_info = repo_info.map_err(github_to_mcp_error)?;

        let readme_content = readme
            .inspect_err(|e| warn!(%e, "failed to fetch README"))
            .ok()
            .and_then(|r| {
                r.content.and_then(|c| {
                    github::decode_content(&c)
                        .inspect_err(|e| warn!(%e, "failed to decode README"))
                        .ok()
                })
            });

        let issues = issues
            .inspect_err(|e| warn!(%e, "failed to fetch issues"))
            .unwrap_or_default();
        let pulls = pulls
            .inspect_err(|e| warn!(%e, "failed to fetch pulls"))
            .unwrap_or_default();
        let releases = releases
            .inspect_err(|e| warn!(%e, "failed to fetch releases"))
            .unwrap_or_default();

        let output = github::format::format_overview(
            &repo_info,
            readme_content.as_deref(),
            &issues,
            &pulls,
            &releases,
        );

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
            instructions: Some(
                "scout provides web search (via Gemini Grounding), page fetching (local HTML→Markdown conversion), and GitHub repository exploration (repo_tree, repo_read, repo_overview) tools."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

fn github_to_mcp_error(e: github::GitHubError) -> McpError {
    match &e {
        github::GitHubError::NotFound(_)
        | github::GitHubError::InvalidRepo(_)
        | github::GitHubError::InvalidRef(_)
        | github::GitHubError::InvalidPath(_)
        | github::GitHubError::InvalidLineRange(_)
        | github::GitHubError::InvalidPattern(_) => {
            McpError::invalid_params(e.to_string(), None)
        }
        github::GitHubError::RateLimited => {
            McpError::internal_error(format!("{e} (retriable)"), None)
        }
        github::GitHubError::Forbidden(_) => McpError::internal_error(
            format!("{e} — check that your GITHUB_TOKEN has the required scopes"),
            None,
        ),
        _ => McpError::internal_error(e.to_string(), None),
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
        Scout {
            http: Client::new(),
            gemini: None,
            github: GitHubClient::from_env(Client::new()),
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

    #[test]
    fn github_to_mcp_error_maps_not_found_to_invalid_params() {
        let err = github_to_mcp_error(github::GitHubError::NotFound("test".into()));
        assert!(err.message.contains("Not found"));
    }

    #[test]
    fn github_to_mcp_error_maps_invalid_repo_to_invalid_params() {
        let err = github_to_mcp_error(github::GitHubError::InvalidRepo("bad".into()));
        assert!(err.message.contains("owner/repo"));
    }

    #[test]
    fn github_to_mcp_error_maps_invalid_ref_to_invalid_params() {
        let err = github_to_mcp_error(github::GitHubError::InvalidRef("bad".into()));
        assert!(err.message.contains("Invalid ref"));
    }

    #[test]
    fn github_to_mcp_error_maps_invalid_path_to_invalid_params() {
        let err = github_to_mcp_error(github::GitHubError::InvalidPath("bad".into()));
        assert!(err.message.contains("Invalid path"));
    }

    #[test]
    fn github_to_mcp_error_maps_rate_limited_to_retriable() {
        let err = github_to_mcp_error(github::GitHubError::RateLimited);
        assert!(err.message.contains("retriable"));
        assert!(err.message.contains("rate limit"));
    }

    #[test]
    fn github_to_mcp_error_maps_forbidden_with_token_hint() {
        let err = github_to_mcp_error(github::GitHubError::Forbidden("denied".into()));
        assert!(err.message.contains("denied"));
        assert!(err.message.contains("GITHUB_TOKEN"));
    }

    #[test]
    fn github_to_mcp_error_maps_invalid_line_range() {
        let err = github_to_mcp_error(github::GitHubError::InvalidLineRange("bad".into()));
        assert!(err.message.contains("line range"));
    }

    #[test]
    fn github_to_mcp_error_maps_invalid_pattern() {
        let err = github_to_mcp_error(github::GitHubError::InvalidPattern("bad".into()));
        assert!(err.message.contains("pattern"));
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
