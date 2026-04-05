use clap::{Args, Subcommand};

use crate::search::Lang;

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
#[command(after_help = "\
Examples:
  scout search \"Rust async patterns\"
  scout search \"状態管理\" --lang ja

Environment:
  GEMINI_API_KEY  Required. Gemini API key for web search.")]
pub struct SearchParams {
    /// Search query
    pub query: String,
    /// Search language
    #[arg(short, long, value_enum, default_value_t = Lang::Auto)]
    pub lang: Lang,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout fetch https://example.com
  scout fetch https://example.com --js
  scout fetch https://example.com --raw")]
pub struct FetchParams {
    /// URL to fetch (must be HTTP or HTTPS)
    pub url: String,
    /// Force JavaScript rendering via headless Chrome / CDP. Usually unnecessary — auto-detected for SPA pages and pages with too little extracted content.
    #[arg(long)]
    pub js: bool,
    /// Skip Readability extraction and convert entire page
    #[arg(long)]
    pub raw: bool,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout research \"state management\"
  scout research \"Rust error handling\" --depth 5
  scout research \"型安全\" --lang ja --depth 3

Environment:
  GEMINI_API_KEY  Required. Gemini API key for web search.")]
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
#[command(after_help = "\
Examples:
  scout repo-tree facebook/react
  scout repo-tree facebook/react --path src/
  scout repo-tree facebook/react --pattern \"*.rs\"
  scout repo-tree facebook/react --ref v18.0.0

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
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
#[command(after_help = "\
Examples:
  scout repo-read facebook/react README.md
  scout repo-read facebook/react src/index.ts --lines 1-80
  scout repo-read facebook/react Cargo.toml --ref main
  scout repo-read owner/repo legacy.txt --encoding shift_jis

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
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
    /// Character encoding label (e.g., shift_jis, euc-jp, gbk).
    /// When omitted, auto-detects UTF-8, Shift_JIS, EUC-JP, GBK, EUC-KR, and other
    /// multi-byte encodings via BOM and chardetng. Single-byte encodings (windows-1252,
    /// ISO-8859-*, etc.) require explicit --encoding.
    #[arg(long)]
    pub encoding: Option<String>,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout repo-overview facebook/react
  scout repo-overview rust-lang/rust

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
pub struct RepoOverviewParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub repository: String,
}

#[cfg(test)]
mod tests {
    use clap::Args;

    fn help_text<A: Args + Send + Sync + 'static>() -> String {
        A::augment_args(clap::Command::new("test"))
            .render_help()
            .to_string()
    }

    fn assert_help_sections<A: Args + Send + Sync + 'static>(expected_env: Option<&str>) {
        let help = help_text::<A>();
        assert!(help.contains("Examples:"), "help missing Examples:");
        if let Some(env_key) = expected_env {
            assert!(help.contains(env_key), "help missing {env_key}");
        }
    }

    /// [T-H001] search --help contains Examples: and Environment: sections
    #[test]
    fn t_h001_search_help_contains_examples_and_environment() {
        assert_help_sections::<super::SearchParams>(Some("GEMINI_API_KEY"));
    }

    /// [T-H002] fetch --help contains Examples: section
    #[test]
    fn t_h002_fetch_help_contains_examples() {
        assert_help_sections::<super::FetchParams>(None);
    }

    /// [T-H003] research --help contains Examples: and Environment: sections
    #[test]
    fn t_h003_research_help_contains_examples_and_environment() {
        assert_help_sections::<super::ResearchParams>(Some("GEMINI_API_KEY"));
    }

    /// [T-H004] repo-tree --help contains Examples: and Environment: sections
    #[test]
    fn t_h004_repo_tree_help_contains_examples_and_environment() {
        assert_help_sections::<super::RepoTreeParams>(Some("GITHUB_TOKEN"));
    }

    /// [T-H005] repo-read --help contains Examples: and Environment: sections
    #[test]
    fn t_h005_repo_read_help_contains_examples_and_environment() {
        assert_help_sections::<super::RepoReadParams>(Some("GITHUB_TOKEN"));
    }

    /// [T-H006] repo-overview --help contains Examples: and Environment: sections
    #[test]
    fn t_h006_repo_overview_help_contains_examples_and_environment() {
        assert_help_sections::<super::RepoOverviewParams>(Some("GITHUB_TOKEN"));
    }

    /// [T-P001] research --depth accepts valid range 1..=10 and rejects out-of-range
    #[test]
    fn t_p001_research_depth_valid_range() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            cmd: super::Command,
        }

        for val in ["1", "3", "10"] {
            let result = Cli::try_parse_from(["scout", "research", "test query", "--depth", val]);
            assert!(result.is_ok(), "depth={val} should be accepted");
        }
        for val in ["0", "11", "255"] {
            let result = Cli::try_parse_from(["scout", "research", "test query", "--depth", val]);
            assert!(result.is_err(), "depth={val} should be rejected");
        }
    }
}
