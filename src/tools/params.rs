use clap::{Args, Subcommand};

pub use crate::search::Lang;

#[derive(Subcommand)]
pub enum Command {
    /// Search the web using Gemini Grounding with Google Search
    Search(SearchParams),
    /// Fetch a web page and convert it to clean Markdown
    Fetch(FetchParams),
    /// Deep research: search the web, fetch top results, and compile a report
    Research(ResearchParams),
    /// List files in a remote GitHub repository
    RepoTree(RepoTreeParams),
    /// Read a file from a remote GitHub repository
    RepoRead(RepoReadParams),
    /// Get a comprehensive overview of a remote GitHub repository
    RepoOverview(RepoOverviewParams),
}

#[derive(Args)]
pub struct SearchParams {
    /// Search query
    pub query: String,
    /// Search language
    #[arg(short, long, value_enum, default_value_t = Lang::Auto)]
    pub lang: Lang,
}

#[derive(Args)]
pub struct FetchParams {
    /// URL to fetch (must be HTTP or HTTPS)
    pub url: String,
    /// Skip Readability extraction and convert entire page
    #[arg(long)]
    pub raw: bool,
    /// Include page metadata (title, author, date) as YAML frontmatter
    #[arg(long)]
    pub meta: bool,
}

#[derive(Args)]
pub struct ResearchParams {
    /// Research query
    pub query: String,
    /// Number of URLs to fetch for deep analysis (1-10)
    #[arg(short, long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=10))]
    pub depth: u8,
    /// Search language
    #[arg(short, long, value_enum, default_value_t = Lang::Auto)]
    pub lang: Lang,
}

#[derive(Args)]
pub struct RepoTreeParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
    /// Git ref: branch name, tag, or commit SHA
    #[arg(long, name = "ref")]
    pub ref_: Option<String>,
    /// Filter to files under this path prefix (e.g., "src/components/")
    #[arg(short, long)]
    pub path: Option<String>,
    /// Glob pattern to filter filenames (e.g., "*.rs", "*.{ts,tsx}")
    #[arg(long)]
    pub pattern: Option<String>,
}

#[derive(Args)]
pub struct RepoReadParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
    /// File path within the repository (e.g., "src/index.ts")
    pub path: String,
    /// Git ref: branch name, tag, or commit SHA
    #[arg(long, name = "ref")]
    pub ref_: Option<String>,
    /// Line range: "1-80", "50-", or "100" (first N lines)
    #[arg(short, long)]
    pub lines: Option<String>,
}

#[derive(Args)]
pub struct RepoOverviewParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
}
