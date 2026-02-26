use schemars::JsonSchema;
use serde::Deserialize;

pub use crate::search::Lang;

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
