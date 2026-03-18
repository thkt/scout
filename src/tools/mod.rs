mod errors;
mod params;

pub use errors::ScoutError;
pub use params::Command;

use std::time::Duration;

use reqwest::Client;
use tracing::{info, warn};

use errors::{parse_repo_param, unwrap_or_note};
use params::{
    FetchParams, RepoOverviewParams, RepoReadParams, RepoTreeParams, ResearchParams, SearchParams,
};

use crate::fetch::{FetchOptions, TokioDnsResolver};
use crate::gemini::client::{GeminiClient, GeminiError, SearchClient as _};
use crate::github::{self, GitHubClient};
use crate::markdown::{escape_md_link, shift_headings, truncate_with_note};
use crate::search::engine;

impl From<&FetchParams> for FetchOptions {
    fn from(p: &FetchParams) -> Self {
        Self { js: p.js, raw: p.raw }
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// HTTP_TIMEOUT (30s) + PLAYWRIGHT_TIMEOUT (60s) + 5s margin.
const FETCH_TOOL_TIMEOUT: Duration = Duration::from_secs(95);
const MAX_REDIRECTS: usize = 5;
const OVERVIEW_ITEMS: u8 = 5;
const OVERVIEW_RELEASES: u8 = 3;
const MAX_FETCH_OUTPUT_BYTES: usize = 100_000;
/// Slack: up to 3 API calls + N user resolutions; 60s covers large threads.
const SLACK_TOOL_TIMEOUT: Duration = Duration::from_secs(60);

/// CLI tool runner providing search, fetch, and GitHub tools.
pub struct Scout {
    http: Client,
    gemini: Option<GeminiClient>,
    github: GitHubClient,
}

impl Scout {
    pub async fn new() -> Result<Self, ScoutError> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(|e| ScoutError::internal(format!("HTTP client init failed: {e}")))?;
        let gemini = GeminiClient::from_env(http.clone())
            .inspect_err(|e| warn!("Gemini client not available: {e}"))
            .ok();
        let github = GitHubClient::from_env(http.clone()).await;
        Ok(Self {
            http,
            gemini,
            github,
        })
    }

    fn gemini(&self) -> Result<&GeminiClient, ScoutError> {
        self.gemini
            .as_ref()
            .ok_or_else(|| ScoutError::from(GeminiError::ApiKeyNotSet))
    }

    pub async fn run(&self, cmd: Command) -> Result<String, ScoutError> {
        match cmd {
            Command::Search(params) => self.search(params).await,
            Command::Fetch(params) => self.fetch(params).await,
            Command::Research(params) => self.research(params).await,
            Command::RepoTree(params) => self.repo_tree(params).await,
            Command::RepoRead(params) => self.repo_read(params).await,
            Command::RepoOverview(params) => self.repo_overview(params).await,
        }
    }

    async fn search(&self, params: SearchParams) -> Result<String, ScoutError> {
        info!(query = %params.query, "search");

        let gemini = self.gemini()?;
        let search_query = params.lang.apply_to_query(&params.query);
        let result = gemini.search(&search_query).await?;

        let mut output = result
            .answer
            .unwrap_or_else(|| {
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
        Ok(output)
    }

    async fn fetch(&self, params: FetchParams) -> Result<String, ScoutError> {
        if let Some(slack_url) = crate::slack::parse_slack_url(&params.url) {
            return self.fetch_slack(slack_url).await;
        }

        info!(url = %params.url, js = params.js, raw = params.raw, "fetch");

        let opts = FetchOptions::from(&params);
        let result = tokio::time::timeout(
            FETCH_TOOL_TIMEOUT,
            crate::fetch::fetch_page(&self.http, &params.url, opts, &TokioDnsResolver),
        )
        .await
        .unwrap_or_else(|_| {
            Err(crate::fetch::FetchError::Timeout(format!(
                "fetch timed out after {}s",
                FETCH_TOOL_TIMEOUT.as_secs()
            )))
        })?;

        if result.used_raw_fallback {
            warn!(url = %params.url, "readability extraction failed, using raw fallback");
        }

        Ok(format_fetch_output(&result))
    }

    async fn fetch_slack(&self, slack_url: crate::slack::SlackUrl) -> Result<String, ScoutError> {
        info!(workspace = %slack_url.workspace, channel = %slack_url.channel, "fetch (slack)");
        let client = crate::slack::SlackClient::from_env(self.http.clone())?;
        let output = tokio::time::timeout(
            SLACK_TOOL_TIMEOUT,
            client.fetch_message(&slack_url),
        )
        .await
        .unwrap_or_else(|_| {
            Err(crate::slack::SlackError::Network(format!(
                "slack fetch timed out after {}s",
                SLACK_TOOL_TIMEOUT.as_secs()
            )))
        })?;
        Ok(truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES).into_owned())
    }

    async fn research(&self, params: ResearchParams) -> Result<String, ScoutError> {
        info!(query = %params.query, depth = params.depth, "research");

        let gemini = self.gemini()?;

        let req = engine::ResearchRequest {
            query: &params.query,
            depth: params.depth,
            lang: params.lang,
        };
        let report = engine::research(gemini, &self.http, &req, &TokioDnsResolver).await?;

        info!(
            pages = report.fetched_pages.len(),
            failed = report.failed_urls.len(),
            sources = report.all_sources.len(),
            "research complete"
        );

        Ok(engine::format_report(&report, &params.query))
    }

    async fn repo_tree(&self, params: RepoTreeParams) -> Result<String, ScoutError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, "repo_tree");

        let ref_ = match params.ref_ {
            Some(r) => {
                github::validate_ref(&r)?;
                r
            }
            None => self.github.get_repo(owner, repo).await?.default_branch,
        };

        if let Some(ref p) = params.path {
            github::validate_path(p)?;
        }

        let tree = self.github.get_tree(owner, repo, &ref_).await?;

        let filtered = github::filter_tree_entries(
            &tree.tree,
            params.path.as_deref(),
            params.pattern.as_deref(),
        )?;

        let output = github::format::format_tree(owner, repo, &ref_, &filtered, tree.truncated);

        info!(files = filtered.len(), "repo_tree complete");
        Ok(output)
    }

    async fn repo_read(&self, params: RepoReadParams) -> Result<String, ScoutError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, path = %params.path, "repo_read");

        github::validate_path(&params.path)?;
        if let Some(ref r) = params.ref_ {
            github::validate_ref(r)?;
        }

        let contents = self
            .github
            .get_contents(owner, repo, &params.path, params.ref_.as_deref())
            .await?;

        let raw = if let Some(ref encoded) = contents.content {
            github::decode_content(encoded)?
        } else {
            let blob = self.github.get_blob(owner, repo, &contents.sha).await?;
            github::decode_content(&blob.content)?
        };

        let total = raw.lines().count();
        let content = if let Some(ref range) = params.lines {
            let (start, end) = github::parse_line_range(range)?;
            github::apply_line_range(&raw, start, end)
        } else {
            github::apply_line_range(&raw, 1, None)
        };

        let output = format!("{} ({total} lines)\n\n{content}", params.path);

        info!(path = %params.path, lines = total, "repo_read complete");
        Ok(output)
    }

    async fn repo_overview(&self, params: RepoOverviewParams) -> Result<String, ScoutError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, "repo_overview");

        let (repo_info, readme, issues, pulls, releases) = tokio::join!(
            self.github.get_repo(owner, repo),
            self.github.get_readme(owner, repo),
            self.github.get_issues(owner, repo, OVERVIEW_ITEMS),
            self.github.get_pulls(owner, repo, OVERVIEW_ITEMS),
            self.github.get_releases(owner, repo, OVERVIEW_RELEASES),
        );

        let repo_info = repo_info?;

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
        Ok(output)
    }
}

fn format_fetch_output(result: &crate::fetch::converter::FetchResult) -> String {
    let shifted = shift_headings(&result.markdown, 2);
    let output = if result.used_raw_fallback {
        format!("{}{shifted}", crate::fetch::converter::RAW_FALLBACK_NOTE)
    } else {
        shifted
    };

    truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Lang;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn scout_with_gemini(gemini_uri: &str) -> Scout {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .unwrap();
        Scout {
            http: http.clone(),
            gemini: Some(GeminiClient::with_base_url(http.clone(), gemini_uri)),
            github: GitHubClient::with_base_url(http, "http://localhost:0"),
        }
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
        let params = SearchParams {
            query: "What is Rust?".into(),
            lang: Lang::Auto,
        };

        let result = s.search(params).await.unwrap();
        assert!(!result.is_empty());
        assert!(
            result.contains("Rust is a systems programming language"),
            "should contain answer text"
        );
        assert!(
            !result.contains("**Query:**"),
            "should not contain Query header (redundant for LLMs)"
        );
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
        let params = ResearchParams {
            query: "What is Rust?".into(),
            depth: 1,
            lang: Lang::Auto,
        };

        let result = s.research(params).await.unwrap();
        assert!(
            result.contains("Rust"),
            "report should contain search answer, got: {result}"
        );
        assert!(
            result.contains("rust-lang.org"),
            "report should reference source URL"
        );
    }

    #[test]
    fn fetch_output_shifts_headings() {
        let result = crate::fetch::converter::FetchResult {
            url: "https://example.com".into(),
            markdown: "# Title\n## Section\nContent".into(),
            used_raw_fallback: false,
        };
        let output = format_fetch_output(&result);
        assert!(output.contains("### Title"), "h1 should shift to h3");
        assert!(output.contains("#### Section"), "h2 should shift to h4");
    }

    #[test]
    fn fetch_output_shifts_headings_with_raw_fallback() {
        let result = crate::fetch::converter::FetchResult {
            url: "https://example.com".into(),
            markdown: "# Raw Title\nBody".into(),
            used_raw_fallback: true,
        };
        let output = format_fetch_output(&result);
        assert!(
            output.starts_with(crate::fetch::converter::RAW_FALLBACK_NOTE.trim_end()),
            "should prepend fallback note"
        );
        assert!(output.contains("### Raw Title"), "h1 should shift to h3");
    }

    #[test]
    fn fetch_output_truncates_long_content() {
        let result = crate::fetch::converter::FetchResult {
            url: "https://example.com".into(),
            markdown: format!("# Title\n{}", "x".repeat(150_000)),
            used_raw_fallback: false,
        };
        let output = format_fetch_output(&result);
        assert!(
            output.len() < 150_000,
            "output should be truncated, got {} bytes",
            output.len()
        );
        assert!(
            output.contains("(truncated: showing"),
            "should include truncation message"
        );
        assert!(output.contains("### Title"), "headings should still be shifted");
    }
}
