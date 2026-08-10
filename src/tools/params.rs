use clap::{Args, Subcommand};

use crate::search::Lang;

use super::errors::ScoutError;

pub(super) fn resolve_input(
    value: Option<String>,
    stdin: Option<&str>,
    stdin_is_terminal: bool,
    label: &str,
    placeholder: &str,
) -> Result<String, ScoutError> {
    match value {
        Some(v) if v != "-" => Ok(v),
        None if stdin_is_terminal => Err(ScoutError::user_error(format!(
            "missing {label}. Pass {placeholder}, pipe it via stdin, or use `-` to read stdin interactively"
        ))),
        _ => stdin
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_owned())
            .ok_or_else(|| {
                ScoutError::user_error(format!(
                    "no {label} provided. Pass {placeholder}, pipe it via stdin, or use `-` to read stdin interactively"
                ))
            }),
    }
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Search the web using Brave Search API
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
  echo \"Rust async patterns\" | scout search
  scout search -

Environment:
  BRAVE_SEARCH_API_KEY  Required. Brave Search API key for web search.")]
pub(crate) struct SearchParams {
    /// Search query
    pub(super) query: Option<String>,
    /// Search language
    #[arg(short, long, value_enum, default_value_t = Lang::Auto)]
    pub(super) lang: Lang,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout fetch https://example.com
  scout fetch https://example.com --js
  scout fetch https://example.com --raw
  echo \"https://example.com\" | scout fetch
  scout fetch -")]
pub(crate) struct FetchParams {
    /// URL to fetch (must be HTTP or HTTPS)
    pub(super) url: Option<String>,
    /// Force JavaScript rendering via headless Chrome / CDP (requires the `js-rendering` build feature and Chrome/Chromium). Usually unnecessary — auto-detected for SPA pages and pages with too little extracted content.
    #[arg(long)]
    pub(super) js: bool,
    /// Skip Readability extraction and convert entire page
    #[arg(long)]
    pub(super) raw: bool,
}

#[cfg(test)]
impl FetchParams {
    /// Named rather than `Default` so a test that cares about `js` or `raw` has to
    /// set the flag visibly.
    pub(crate) fn for_test(url: &str) -> Self {
        Self {
            url: Some(url.to_owned()),
            js: false,
            raw: false,
        }
    }
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout research \"state management\"
  scout research \"Rust error handling\" --depth 5
  scout research \"型安全\" --lang ja --depth 3
  echo \"state management\" | scout research
  scout research -

Environment:
  BRAVE_SEARCH_API_KEY  Required. Brave Search API key for web search.")]
pub(crate) struct ResearchParams {
    /// Research query
    pub(super) query: Option<String>,
    /// Number of URLs to fetch for deep analysis (1-10)
    #[arg(short, long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=10))]
    pub(super) depth: u8,
    /// Search language
    #[arg(short, long, value_enum, default_value_t = Lang::Auto)]
    pub(super) lang: Lang,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout repo-tree facebook/react
  scout repo-tree facebook/react --path src/
  scout repo-tree facebook/react --pattern \"*.rs\"
  scout repo-tree facebook/react --ref v18.0.0
  echo \"facebook/react\" | scout repo-tree
  scout repo-tree -

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
pub(crate) struct RepoTreeParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub(super) repository: Option<String>,
    /// Git ref: branch name, tag, or commit SHA
    #[arg(long, name = "ref")]
    pub(super) ref_: Option<String>,
    /// Filter to files under this path prefix (e.g., "src/components/")
    #[arg(short, long)]
    pub(super) path: Option<String>,
    /// Glob pattern to filter filenames (e.g., "*.rs", "*.{ts,tsx}")
    #[arg(long)]
    pub(super) pattern: Option<String>,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout repo-read facebook/react README.md
  scout repo-read facebook/react src/index.ts --lines 1-80
  scout repo-read facebook/react Cargo.toml --ref main
  scout repo-read owner/repo legacy.txt --encoding shift_jis
  echo \"README.md\" | scout repo-read facebook/react
  echo \"facebook/react\" | scout repo-read - README.md

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
pub(crate) struct RepoReadParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub(super) repository: Option<String>,
    /// File path within the repository (e.g., "src/index.ts")
    pub(super) path: Option<String>,
    /// Git ref: branch name, tag, or commit SHA
    #[arg(long, name = "ref")]
    pub(super) ref_: Option<String>,
    /// Line range: "1-80", "50-", or "100" (first N lines)
    #[arg(short, long)]
    pub(super) lines: Option<String>,
    /// Character encoding label (e.g., shift_jis, euc-jp, gbk).
    /// When omitted, auto-detects UTF-8, Shift_JIS, EUC-JP, GBK, EUC-KR, and other
    /// multi-byte encodings via BOM and chardetng. Single-byte encodings (windows-1252,
    /// ISO-8859-*, etc.) require explicit --encoding.
    #[arg(long)]
    pub(super) encoding: Option<String>,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  scout repo-overview facebook/react
  scout repo-overview rust-lang/rust
  echo \"facebook/react\" | scout repo-overview
  scout repo-overview -

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
pub(crate) struct RepoOverviewParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub(super) repository: Option<String>,
}

#[cfg(test)]
mod tests {
    use clap::Args;

    use super::resolve_input;

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
    fn search_help_contains_examples_and_environment() {
        assert_help_sections::<super::SearchParams>(Some("BRAVE_SEARCH_API_KEY"));
    }

    /// [T-H002] fetch --help contains Examples: section
    #[test]
    fn fetch_help_contains_examples() {
        assert_help_sections::<super::FetchParams>(None);
    }

    /// [T-H003] research --help contains Examples: and Environment: sections
    #[test]
    fn research_help_contains_examples_and_environment() {
        assert_help_sections::<super::ResearchParams>(Some("BRAVE_SEARCH_API_KEY"));
    }

    /// [T-H004]
    #[test]
    fn repo_tree_help_contains_examples_and_environment() {
        assert_help_sections::<super::RepoTreeParams>(Some("GITHUB_TOKEN"));
    }

    /// [T-H005]
    #[test]
    fn repo_read_help_contains_examples_and_environment() {
        assert_help_sections::<super::RepoReadParams>(Some("GITHUB_TOKEN"));
    }

    /// [T-H006] repo-overview --help contains Examples: and Environment: sections
    #[test]
    fn repo_overview_help_contains_examples_and_environment() {
        assert_help_sections::<super::RepoOverviewParams>(Some("GITHUB_TOKEN"));
    }

    /// [T-H009] stdin-supporting subcommand help contains stdin usage examples
    #[test]
    fn subcommand_help_contains_stdin_examples() {
        let cases: &[(&str, &str)] = &[
            ("search", "| scout search"),
            ("fetch", "| scout fetch"),
            ("research", "| scout research"),
            ("repo-tree", "| scout repo-tree"),
            ("repo-read", "| scout repo-read"),
            ("repo-overview", "| scout repo-overview"),
        ];
        let helps = [
            help_text::<super::SearchParams>(),
            help_text::<super::FetchParams>(),
            help_text::<super::ResearchParams>(),
            help_text::<super::RepoTreeParams>(),
            help_text::<super::RepoReadParams>(),
            help_text::<super::RepoOverviewParams>(),
        ];
        for ((name, pattern), help) in cases.iter().zip(helps.iter()) {
            assert!(
                help.contains(pattern),
                "{name} help missing stdin example '{pattern}'"
            );
        }
    }

    /// [T-P001] research --depth accepts valid range 1..=10 and rejects out-of-range
    #[test]
    fn research_depth_valid_range() {
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

    /// [T-S001]
    #[test]
    fn optional_positional_is_none_when_omitted() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            cmd: super::Command,
        }

        let result = Cli::try_parse_from(["scout", "search"]);
        assert!(result.is_ok(), "parse should succeed with query omitted");
        if let Ok(cli) = result
            && let super::Command::Search(p) = cli.cmd
        {
            assert!(p.query.is_none(), "query should be None when omitted");
        }
    }

    /// [T-S002] ARG wins over piped stdin when both are present
    #[test]
    fn arg_wins_over_stdin() {
        let result = resolve_input(
            Some("from_arg".into()),
            Some("from_stdin"),
            false,
            "query",
            "<QUERY>",
        );
        assert_eq!(result.unwrap(), "from_arg");
    }

    /// [T-S003]
    #[test]
    fn stdin_used_when_arg_omitted_and_piped() {
        let result = resolve_input(None, Some("from_stdin"), false, "query", "<QUERY>");
        assert_eq!(result.unwrap(), "from_stdin");
    }

    /// [T-S004] `-` reads from stdin even when terminal
    #[test]
    fn dash_reads_from_stdin() {
        let result = resolve_input(
            Some("-".into()),
            Some("from_stdin"),
            true, // terminal
            "query",
            "<QUERY>",
        );
        assert_eq!(result.unwrap(), "from_stdin");
    }

    /// [T-S005] terminal + no arg → fail-fast error with canonical message
    #[test]
    fn terminal_no_arg_returns_fail_fast_error() {
        let result = resolve_input(None, None, true, "query", "<QUERY>");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing query"),
            "error should contain 'missing query', got: {err}"
        );
        assert!(
            err.contains("Pass <QUERY>"),
            "error should contain placeholder, got: {err}"
        );
        assert!(
            err.contains("pipe it via stdin"),
            "error should suggest stdin, got: {err}"
        );
        assert!(
            err.contains("`-`"),
            "error should mention `-` for interactive stdin, got: {err}"
        );
    }

    /// [T-S006] empty stdin → error with "no X provided" canonical message
    #[test]
    fn empty_stdin_returns_no_provided_error() {
        let result = resolve_input(None, Some("   "), false, "query", "<QUERY>");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no query provided"),
            "error should contain 'no query provided', got: {err}"
        );
    }

    /// [T-S007] `-` with empty stdin → same "no X provided" error
    #[test]
    fn dash_with_empty_stdin_returns_error() {
        let result = resolve_input(Some("-".into()), Some(""), true, "url", "<URL>");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no url provided"),
            "error should contain 'no url provided', got: {err}"
        );
    }

    /// [T-S008]
    #[test]
    fn stdin_content_is_trimmed() {
        let result = resolve_input(
            None,
            Some("  facebook/react\n"),
            false,
            "repository",
            "<OWNER/REPO>",
        );
        assert_eq!(result.unwrap(), "facebook/react");
    }
}
