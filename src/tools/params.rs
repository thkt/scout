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
  scout fetch https://acme.slack.com/archives/C123/p1700000000000000
  echo \"https://example.com\" | scout fetch
  scout fetch -

Environment:
  SLACK_TOKEN  Required when the URL is a Slack permalink. User OAuth token (xoxp-…).")]
pub(crate) struct FetchParams {
    /// URL to fetch (must be HTTP or HTTPS)
    pub(super) url: Option<String>,
    /// Force JavaScript rendering via headless Chrome / CDP (requires the `js-rendering` build feature and Chrome/Chromium). Usually unnecessary — auto-detected for SPA pages and pages with too little extracted content.
    #[arg(long)]
    pub(super) js: bool,
    /// Skip Readability extraction and convert the whole page. Script, style and other active HTML still have their content dropped.
    #[arg(long)]
    pub(super) raw: bool,
}

#[cfg(test)]
impl FetchParams {
    /// Named rather than `Default` so a test that cares about `js` or `raw` has to
    /// set the flag visibly.
    pub(super) fn for_test(url: &str) -> Self {
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
    /// Glob pattern matched against the whole repo-relative path
    /// (e.g., "*.rs", "src/*.rs", "*.{ts,tsx}")
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

Output:
  Metadata, README, the 5 open issues and 5 open pull requests GitHub returns
  first, and the 3 most recent releases. Not paginated: each list is one page,
  so a busier repository holds more than this shows.

Environment:
  GITHUB_TOKEN  Optional. Increases rate limit and enables private repos.")]
pub(crate) struct RepoOverviewParams {
    /// GitHub repository in "owner/repo" format (e.g., "facebook/react")
    pub(super) repository: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::{iter, mem};

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

    /// [T-H002] fetch --help contains Examples: and Environment: sections
    ///
    /// `fetch` was the one subcommand whose help named no environment variable,
    /// yet a Slack permalink fails without `SLACK_TOKEN`. Only the root help
    /// carried it, so an agent that read `scout fetch --help` after that failure
    /// found nothing to act on.
    #[test]
    fn fetch_help_contains_examples_and_environment() {
        assert_help_sections::<super::FetchParams>(Some("SLACK_TOKEN"));
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

    /// [T-H012] repo-tree --help states that --pattern matches the whole
    /// repo-relative path
    ///
    /// A reader who takes the pattern for a filename-only match never writes
    /// `src/*.rs`, and the path-scoped form is the one worth reaching for.
    #[test]
    fn repo_tree_help_states_pattern_matches_the_repo_relative_path() {
        let help = help_text::<super::RepoTreeParams>();
        assert!(
            help.contains("repo-relative path"),
            "--pattern help must say the glob matches the repo-relative path:\n{help}"
        );
        assert!(
            help.contains("src/*.rs"),
            "--pattern help must show a path-scoped example:\n{help}"
        );
    }

    /// [T-H013] repo-overview --help states the per-section item caps and the
    /// absence of pagination
    ///
    /// The output prints a total next to each heading, so a reader can infer a
    /// cut from a mismatch. `--help` alone gives them nothing to infer from.
    #[test]
    fn repo_overview_help_states_item_caps_and_no_pagination() {
        let help = help_text::<super::RepoOverviewParams>();
        for expected in ["5 open issues", "5 open pull requests", "3 most recent"] {
            assert!(
                help.contains(expected),
                "repo-overview help must state the cap {expected:?}:\n{help}"
            );
        }
        assert!(
            help.contains("Not paginated"),
            "repo-overview help must state that the lists are not paginated:\n{help}"
        );
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

    /// Split an example line the way a shell would. Only double quotes appear in
    /// these examples, so nothing else is handled.
    fn shell_split(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = String::new();
        let mut quoted = false;
        for ch in s.chars() {
            match ch {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    if !current.is_empty() {
                        out.push(mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }

    /// The argv of every line in the help's `Examples:` block, taken from the
    /// last `scout ` on the line so a piped example contributes the command and
    /// not the `echo` in front of it.
    fn example_argvs(help: &str) -> Vec<Vec<String>> {
        help.lines()
            .skip_while(|line| !line.trim_start().starts_with("Examples:"))
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .filter_map(|line| {
                line.rsplit_once("scout ")
                    .map(|(_, rest)| shell_split(rest))
            })
            .collect()
    }

    /// [T-H011] every example printed in a subcommand's help parses
    ///
    /// The other help tests assert that an `Examples:` block exists, not that
    /// what it shows works — a renamed flag or a mistyped subcommand left them
    /// all passing while the help told an agent to run something that exits 64.
    /// Parsing is the whole check: it needs no network, and the examples carry
    /// no shell syntax beyond quoting.
    #[test]
    fn help_examples_parse() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            cmd: super::Command,
        }

        let helps = [
            help_text::<super::SearchParams>(),
            help_text::<super::FetchParams>(),
            help_text::<super::ResearchParams>(),
            help_text::<super::RepoTreeParams>(),
            help_text::<super::RepoReadParams>(),
            help_text::<super::RepoOverviewParams>(),
        ];

        let mut checked = 0;
        for help in &helps {
            for argv in example_argvs(help) {
                let full: Vec<String> = iter::once("scout".to_owned())
                    .chain(argv.iter().cloned())
                    .collect();
                assert!(
                    Cli::try_parse_from(&full).is_ok(),
                    "help shows an example that does not parse: scout {}",
                    argv.join(" ")
                );
                checked += 1;
            }
        }
        // Guards the extraction itself: a change to the help layout that stopped
        // matching would otherwise leave this test passing on zero examples.
        assert!(
            checked >= 25,
            "expected every subcommand's examples to be checked, got {checked}"
        );
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
