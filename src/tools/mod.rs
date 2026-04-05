mod errors;
mod params;

pub use errors::ScoutError;
pub use params::Command;

use std::time::Duration;

use reqwest::Client;
use tokio::sync::OnceCell;
use tracing::{info, warn};

use errors::{parse_repo_param, unwrap_or_note};
use params::{
    FetchParams, RepoOverviewParams, RepoReadParams, RepoTreeParams, ResearchParams, SearchParams,
};

use crate::fetch::{FetchOptions, TokioDnsResolver};
use crate::gemini::client::{GeminiClient, GeminiError, SearchClient as _};
use crate::github::{self, GitHubClient};
use crate::markdown::{escape_md_inline, escape_md_link, shift_headings, truncate_with_note};
use crate::search::engine;

impl From<&FetchParams> for FetchOptions {
    fn from(p: &FetchParams) -> Self {
        Self {
            js: p.js,
            raw: p.raw,
        }
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// HTTP_TIMEOUT (30s) + CDP_TIMEOUT (60s) + 5s margin.
const FETCH_TOOL_TIMEOUT: Duration = Duration::from_secs(95);
const MAX_REDIRECTS: usize = 5;
const OVERVIEW_ITEMS: u8 = 5;
const OVERVIEW_RELEASES: u8 = 3;
const MAX_FETCH_OUTPUT_BYTES: usize = 100_000;
/// Slack: up to 3 API calls + N user resolutions; 60s covers large threads.
const SLACK_TOOL_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Scout {
    http: Client,
    /// HTTP client with redirect following disabled for SSRF-safe fetching.
    /// Used by `fetch_page` which handles redirects manually with per-hop SSRF checks.
    fetch_http: Client,
    gemini: Option<GeminiClient>,
    /// Lazy-initialized on first GitHub API call. Non-GitHub commands
    /// (search, fetch, research) never pay the `gh auth token` cost.
    github: OnceCell<GitHubClient>,
}

impl Scout {
    pub async fn new() -> Result<Self, ScoutError> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(|e| ScoutError::internal(format!("HTTP client init failed: {e}")))?;
        let fetch_http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ScoutError::internal(format!("HTTP client init failed: {e}")))?;
        let gemini = GeminiClient::from_env(http.clone())
            .inspect_err(|e| warn!("Gemini client not available: {e}"))
            .ok();
        Ok(Self {
            http,
            fetch_http,
            gemini,
            github: OnceCell::new(),
        })
    }

    async fn github(&self) -> &GitHubClient {
        self.github
            .get_or_init(|| GitHubClient::from_env(self.http.clone()))
            .await
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

        // Shift by 2, consistent with fetch standalone output.
        let mut output = shift_headings(
            &result.answer.unwrap_or_else(|| {
                "(No answer returned — the query may have been filtered by safety settings.)"
                    .to_string()
            }),
            2,
        );

        if !result.sources.is_empty() {
            output.push_str("\n\n---\n**Sources:**\n");
            for source in &result.sources {
                output.push_str(&format!(
                    "- [{}]({})\n",
                    escape_md_inline(&source.title),
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
            crate::fetch::fetch_page(&self.fetch_http, &params.url, opts, &TokioDnsResolver),
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
            Err(crate::slack::SlackError::Timeout(format!(
                "slack fetch timed out after {}s",
                SLACK_TOOL_TIMEOUT.as_secs()
            )))
        })
        .inspect_err(|e| {
            warn!(workspace = %slack_url.workspace, channel = %slack_url.channel, error = %e, "slack fetch failed");
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
        let report = engine::research(gemini, &self.fetch_http, &req, &TokioDnsResolver).await?;

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

        let github = self.github().await;

        let ref_ = match params.ref_ {
            Some(r) => {
                github::validate_ref(&r)?;
                r
            }
            None => github.get_repo(owner, repo).await?.default_branch,
        };

        if let Some(ref p) = params.path {
            github::validate_path(p)?;
        }

        let tree = github.get_tree(owner, repo, &ref_).await?;

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

        let github = self.github().await;

        let contents = github
            .get_contents(owner, repo, &params.path, params.ref_.as_deref())
            .await?;

        let raw = if let Some(encoded) = contents.content.as_ref().filter(|c| !c.is_empty()) {
            github::decode_content(encoded)?
        } else {
            let blob = github.get_blob(owner, repo, &contents.sha).await?;
            github::decode_content(&blob.content)?
        };

        let total = raw.lines().count();
        let content = if let Some(ref range) = params.lines {
            let (start, end) = github::parse_line_range(range)?;
            github::apply_line_range(&raw, start, end)
        } else {
            github::apply_line_range(&raw, 1, None)
        };

        let output = github::format::format_file_content(&params.path, total, &content);

        info!(path = %params.path, lines = total, "repo_read complete");
        Ok(output)
    }

    async fn repo_overview(&self, params: RepoOverviewParams) -> Result<String, ScoutError> {
        let (owner, repo) = parse_repo_param(&params.repository)?;

        info!(repository = %params.repository, "repo_overview");

        let github = self.github().await;

        // Verify repo exists before issuing remaining API calls (#18).
        let repo_info = github.get_repo(owner, repo).await?;

        let (readme, issues, pulls, releases) = tokio::join!(
            github.get_readme(owner, repo),
            github.get_issues(owner, repo, OVERVIEW_ITEMS),
            github.get_pulls(owner, repo, OVERVIEW_ITEMS),
            github.get_releases(owner, repo, OVERVIEW_RELEASES),
        );

        let mut notes = Vec::new();

        let readme_entry = match readme {
            Ok(r) => Some(r),
            Err(e) => {
                if !matches!(e, github::GitHubError::NotFound(_)) {
                    warn!(%e, "failed to fetch README");
                    notes.push(format!("Could not fetch README ({e})"));
                }
                None
            }
        };

        let readme_encoded = match readme_entry {
            None => None,
            Some(r) if r.content.as_ref().is_some_and(|c| !c.is_empty()) => r.content,
            Some(r) => match github.get_blob(owner, repo, &r.sha).await {
                Ok(blob) => Some(blob.content).filter(|c| !c.is_empty()),
                Err(e) => {
                    warn!(%e, "failed to fetch README blob");
                    notes.push(format!("README could not be fetched ({e})"));
                    None
                }
            },
        };

        let readme_content = readme_encoded.and_then(|c| match github::decode_content(&c) {
            Ok(content) => Some(content),
            Err(e) => {
                warn!(%e, "failed to decode README");
                notes.push(format!("README could not be decoded ({e})"));
                None
            }
        });
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
    use crate::test_support::try_spawn_mock_server;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, ResponseTemplate};

    fn build_test_clients() -> (Client, Client) {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .unwrap();
        let fetch_http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        (http, fetch_http)
    }

    fn scout_with_gemini(gemini_uri: &str) -> Scout {
        scout_with_github(gemini_uri, "http://localhost:0")
    }

    #[tokio::test]
    async fn search_success_returns_content() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
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
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
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

    /// [TC-5] search standalone: empty answer text becomes None via
    /// `extract_grounded_result` (.filter(|t| !t.is_empty())), triggering
    /// the fallback message path in `search()`.
    #[tokio::test]
    async fn search_none_answer_returns_fallback() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": ""}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": []
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_with_gemini(&server.uri());
        let params = SearchParams {
            query: "test".into(),
            lang: Lang::En,
        };

        let result = s.search(params).await.unwrap();
        assert!(
            result.contains("No answer returned"),
            "should contain fallback message, got:\n{result}"
        );
    }

    /// [T-001] search standalone: answer with headings should have them shifted by 2
    #[tokio::test]
    async fn t_001_search_shifts_headings_in_answer() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "# Title\n\n## Sub\n\nBody text"}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": []
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_with_gemini(&server.uri());
        let params = SearchParams {
            query: "test".into(),
            lang: Lang::En,
        };

        let result = s.search(params).await.unwrap();
        assert!(
            result.contains("### Title"),
            "h1 should shift to h3 (shift by 2), got:\n{result}"
        );
        assert!(
            result.contains("#### Sub"),
            "h2 should shift to h4 (shift by 2), got:\n{result}"
        );
    }

    /// [T-003] search standalone: # inside fenced code block should NOT be shifted
    #[tokio::test]
    async fn t_003_search_preserves_headings_in_code_blocks() {
        let answer = "# Real heading\n\n```bash\n# comment in script\n```\n\n## Another heading";
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": answer}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": []
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_with_gemini(&server.uri());
        let params = SearchParams {
            query: "test".into(),
            lang: Lang::En,
        };

        let result = s.search(params).await.unwrap();
        assert!(
            result.contains("### Real heading"),
            "h1 outside code block should shift to h3, got:\n{result}"
        );
        assert!(
            result.contains("#### Another heading"),
            "h2 outside code block should shift to h4, got:\n{result}"
        );
        assert!(
            result.contains("# comment in script"),
            "# inside fenced code block should remain unchanged, got:\n{result}"
        );
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
        assert!(
            output.contains("### Title"),
            "headings should still be shifted"
        );
    }

    // --- GitHub client efficiency tests (lazy init + repo_overview) ---

    fn scout_with_github(gemini_uri: &str, github_uri: &str) -> Scout {
        let (http, fetch_http) = build_test_clients();
        let cell = OnceCell::new();
        cell.set(GitHubClient::with_base_url(http.clone(), github_uri))
            .ok();
        Scout {
            http: http.clone(),
            fetch_http,
            gemini: Some(GeminiClient::with_base_url(http, gemini_uri)),
            github: cell,
        }
    }

    fn scout_lazy(gemini_uri: &str) -> Scout {
        let (http, fetch_http) = build_test_clients();
        Scout {
            http: http.clone(),
            fetch_http,
            gemini: Some(GeminiClient::with_base_url(http, gemini_uri)),
            github: OnceCell::new(),
        }
    }

    /// [T-001] repo_overview: get_repo 404 -> readme/issues/pulls/releases
    /// APIs receive 0 requests.
    #[tokio::test]
    async fn t_001_repo_overview_404_skips_remaining_apis() {
        let Some(server) = try_spawn_mock_server("tools::t_001").await else {
            return;
        };

        // get_repo returns 404
        Mock::given(method("GET"))
            .and(path("/repos/owner/nonexistent"))
            .respond_with(ResponseTemplate::new(404))
            .named("get_repo 404")
            .mount(&server)
            .await;

        // All other APIs expect 0 requests
        Mock::given(method("GET"))
            .and(path("/repos/owner/nonexistent/readme"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .named("readme must not be called")
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/repos/owner/nonexistent/issues"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .named("issues must not be called")
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/repos/owner/nonexistent/pulls"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .named("pulls must not be called")
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/repos/owner/nonexistent/releases"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .named("releases must not be called")
            .mount(&server)
            .await;

        let s = scout_with_github(&server.uri(), &server.uri());
        let params = RepoOverviewParams {
            repository: "owner/nonexistent".into(),
        };

        let result = s.repo_overview(params).await;
        assert!(result.is_err(), "repo_overview should fail on 404");

        // wiremock verifies expect(0) on server drop
    }

    /// [T-002] repo_overview: after get_repo succeeds, readme/issues/pulls/
    /// releases run in parallel.
    ///
    /// Proof: a barrier-synchronized TCP server requires all 4 API requests to
    /// arrive before any response is sent. If requests are sequential, only one
    /// arrives at a time and the barrier never releases → deadlock → timeout.
    #[tokio::test(flavor = "multi_thread")]
    async fn t_002_repo_overview_parallel_after_get_repo() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

        // 4 parallel APIs must all arrive before any response is sent.
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));

        let server = tokio::spawn(async move {
            for _ in 0..10 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let b = barrier.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("");

                    let (body, wait) = if path == "/repos/owner/repo" {
                        (r#"{"full_name":"owner/repo","description":"test","html_url":"https://github.com/owner/repo","default_branch":"main","language":"Rust","stargazers_count":1,"forks_count":0,"open_issues_count":0,"topics":[],"license":null}"#.to_string(), false)
                    } else if path.contains("/git/blobs/") {
                        (r#"{"content":""}"#.to_string(), false)
                    } else if path.contains("/readme") {
                        (r#"{"sha":"abc123","content":""}"#.to_string(), true)
                    } else {
                        ("[]".to_string(), true)
                    };

                    if wait {
                        b.wait().await;
                    }

                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let s = scout_with_github(&base_url, &base_url);
        let params = RepoOverviewParams {
            repository: "owner/repo".into(),
        };

        // Parallel: barrier(4) releases instantly → completes in ms.
        // Sequential: barrier never reaches 4 → deadlock → timeout.
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), s.repo_overview(params)).await;

        assert!(
            result.is_ok(),
            "repo_overview should complete when 4 APIs run in parallel \
             (barrier-synchronized); sequential execution deadlocks"
        );

        server.abort();
    }

    /// [T-003] scout_lazy: github OnceCell is None immediately after
    /// construction.
    #[test]
    fn t_003_scout_lazy_github_initially_none() {
        let s = scout_lazy("http://localhost:0");
        assert!(
            s.github.get().is_none(),
            "github OnceCell should be uninitialized after scout_lazy()"
        );
    }

    /// [T-004] search command does not initialize the GitHub client.
    #[tokio::test]
    async fn t_004_search_leaves_github_uninitialized() {
        let Some(server) = try_spawn_mock_server("tools::t_004").await else {
            return;
        };
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "answer"}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": []
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_lazy(&server.uri());
        let _result = s
            .search(SearchParams {
                query: "test".into(),
                lang: Lang::En,
            })
            .await;

        assert!(
            s.github.get().is_none(),
            "search should not initialize GitHubClient"
        );
    }

    /// [T-005] fetch command does not initialize the GitHub client.
    #[tokio::test]
    async fn t_005_fetch_leaves_github_uninitialized() {
        let Some(server) = try_spawn_mock_server("tools::t_005").await else {
            return;
        };
        // Serve a minimal HTML page for the fetch command to consume.
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<html><body>hello</body></html>"),
            )
            .mount(&server)
            .await;

        let s = scout_lazy(&server.uri());
        let _result = s
            .fetch(FetchParams {
                url: format!("{}/page", server.uri()),
                js: false,
                raw: false,
            })
            .await;

        assert!(
            s.github.get().is_none(),
            "fetch should not initialize GitHubClient"
        );
    }

    /// [T-006] research command does not initialize the GitHub client.
    #[tokio::test]
    async fn t_006_research_leaves_github_uninitialized() {
        let Some(server) = try_spawn_mock_server("tools::t_006").await else {
            return;
        };
        Mock::given(method("POST"))
            .and(path_regex(r":generateContent$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "research result"}],
                        "role": "model"
                    },
                    "groundingMetadata": {
                        "groundingChunks": [{
                            "web": {
                                "uri": "https://example.com",
                                "title": "Example"
                            }
                        }]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let s = scout_lazy(&server.uri());
        let _result = s
            .research(ResearchParams {
                query: "test".into(),
                depth: 1,
                lang: Lang::En,
            })
            .await;

        assert!(
            s.github.get().is_none(),
            "research should not initialize GitHubClient"
        );
    }

    /// [T-007] github() called twice returns the same reference
    /// (OnceCell caching verified via std::ptr::eq).
    #[tokio::test]
    async fn t_007_github_returns_same_reference() {
        // Use pre-set OnceCell to avoid triggering real `gh auth token` subprocess.
        let s = scout_with_github("http://localhost:0", "http://localhost:0");
        let first = s.github().await;
        let second = s.github().await;
        assert!(
            std::ptr::eq(first, second),
            "github() should return the same cached reference on second call"
        );
    }

    /// [T-007b] github() initializes an empty OnceCell via from_env and caches
    /// the result. Exercises the lazy-init code path at mod.rs:80-84.
    ///
    /// from_env is infallible: it resolves token from env vars or `gh auth token`
    /// (with TOKEN_RESOLVE_TIMEOUT = 5s), then returns a client. No timeout
    /// wrapper — a hang here is a real bug, not a flaky environment.
    #[tokio::test]
    async fn t_007b_github_lazy_init_from_empty_cell() {
        let s = scout_lazy("http://localhost:0");
        assert!(s.github.get().is_none(), "starts empty");

        let client = s.github().await;

        assert!(
            s.github.get().is_some(),
            "OnceCell should be initialized after github() call"
        );
        let client2 = s.github().await;
        assert!(
            std::ptr::eq(client, client2),
            "second call returns the same cached reference"
        );
    }
}
