mod errors;
mod params;
mod typo;

pub use errors::ScoutError;
pub use params::Command;

use std::io::{IsTerminal, stdin};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use reqwest::redirect::Policy;
use tokio::io::{AsyncReadExt, stdin as tokio_stdin};
use tokio::sync::{Notify, OnceCell};
use tokio::time::timeout;
use tracing::{info, warn};

use errors::{parse_repo_param, unwrap_or_degraded};
use params::{
    FetchParams, RepoOverviewParams, RepoReadParams, RepoTreeParams, ResearchParams, SearchParams,
    resolve_input,
};

use crate::brave::client::{BraveClient, BraveError, SearchClient as _};
use crate::envelope::{CommandOutput, Degradation, DegradedReason};
use crate::fetch::converter::{FetchResult, RAW_FALLBACK_NOTE};
use crate::fetch::{FetchError, FetchOptions, RedactedLogUrl, TokioDnsResolver, fetch_page};
use crate::github::types::ContentsResponse;
use crate::github::{self, GitHubClient};
use crate::markdown::{shift_headings, truncate_with_note};
use crate::search::engine;
use crate::slack::{SlackClient, SlackError, SlackUrl, parse_slack_url};

const MAX_STDIN_BYTES: u64 = 1_048_576;

async fn read_stdin(needs_stdin: bool) -> Result<Option<String>, ScoutError> {
    if !needs_stdin {
        return Ok(None);
    }
    let mut buf = String::new();
    tokio_stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut buf)
        .await
        .map_err(|e| ScoutError::user_error(format!("Failed to read stdin: {e}")))?;
    let trimmed = buf.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    })
}

/// Resolves CLI positional args with stdin fallback.
/// Stdin is read once; the first arg that needs it consumes it.
struct StdinResolver {
    is_terminal: bool,
    /// `None` = not piped, empty, or already consumed — check `stdin_consumed` to distinguish.
    content: Option<String>,
    /// `true` after any `resolve()` consumed stdin content; `content: None` alone cannot express this.
    stdin_consumed: bool,
}

impl StdinResolver {
    fn resolve(
        &mut self,
        value: Option<String>,
        label: &str,
        placeholder: &str,
    ) -> Result<String, ScoutError> {
        let needs_stdin = value.is_none() || value.as_deref() == Some("-");
        if needs_stdin && self.stdin_consumed {
            let msg = if value.as_deref() == Some("-") {
                format!("stdin already read — cannot use `-` for {label}")
            } else {
                format!(
                    "No {label} provided. Pass {placeholder} as an argument (stdin was already read by the previous argument)"
                )
            };
            return Err(ScoutError::user_error(msg));
        }
        let result = resolve_input(
            value,
            self.content.as_deref(),
            self.is_terminal,
            label,
            placeholder,
        )?;
        if needs_stdin {
            self.content = None;
            self.stdin_consumed = true;
        }
        Ok(result)
    }

    #[cfg(test)]
    fn with_content(is_terminal: bool, content: Option<String>) -> Self {
        Self {
            is_terminal,
            content,
            stdin_consumed: false,
        }
    }
}

async fn resolve_stdin_arg(
    value: Option<String>,
    label: &str,
    placeholder: &str,
) -> Result<String, ScoutError> {
    let is_terminal = stdin().is_terminal();
    let content =
        read_stdin((value.is_none() && !is_terminal) || value.as_deref() == Some("-")).await?;
    resolve_input(value, content.as_deref(), is_terminal, label, placeholder)
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
/// Research: Brave search (up to 20s + retries) + up to 10 page fetches (15s each).
/// 45s caps total wall-clock at the orchestration layer; without this cap a degraded
/// Brave + full-depth fetch could block ~210s.
const RESEARCH_TOOL_TIMEOUT: Duration = Duration::from_secs(45);

pub struct Scout {
    http: Client,
    /// HTTP client with redirect following disabled for SSRF-safe fetching.
    /// Used by `fetch_page` which handles redirects manually with per-hop SSRF checks.
    fetch_http: Client,
    brave: Option<BraveClient>,
    /// Lazy-initialized on first GitHub API call. Non-GitHub commands
    /// (search, fetch, research) never pay the `gh auth token` cost.
    github: OnceCell<GitHubClient>,
    /// Broadcast handle for graceful shutdown. `lib::run` fires
    /// `notify_waiters()` on SIGINT/SIGTERM so in-flight CDP fetches can
    /// invoke `browser.close().await` before the runtime drops the task,
    /// preventing chromium subprocess orphans (issue #121).
    cancel: Arc<Notify>,
}

impl Scout {
    pub async fn new() -> Result<Self, ScoutError> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(|e| ScoutError::io_error(format!("HTTP client init failed: {e}")))?;
        let fetch_http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|e| ScoutError::io_error(format!("HTTP client init failed: {e}")))?;
        let brave = BraveClient::from_env(http.clone())
            .inspect_err(|e| warn!("Brave client not available: {e}"))
            .ok();
        Ok(Self {
            http,
            fetch_http,
            brave,
            github: OnceCell::new(),
            cancel: Arc::new(Notify::new()),
        })
    }

    /// Hand back the shared cancel handle so `lib::run` can fire it
    /// when a termination signal arrives. Cloning an `Arc<Notify>` is
    /// cheap and lets the signal handler outlive the borrow of `Scout`.
    pub fn cancel_handle(&self) -> Arc<Notify> {
        self.cancel.clone()
    }

    async fn github(&self) -> &GitHubClient {
        self.github
            .get_or_init(|| GitHubClient::from_env(self.http.clone()))
            .await
    }

    fn brave(&self) -> Result<&BraveClient, ScoutError> {
        self.brave
            .as_ref()
            .ok_or_else(|| ScoutError::from(BraveError::ApiKeyNotSet))
    }

    pub async fn run(&self, cmd: Command) -> Result<CommandOutput, ScoutError> {
        match cmd {
            Command::Search(params) => self.search(params).await,
            Command::Fetch(params) => self.fetch(params).await,
            Command::Research(params) => self.research(params).await,
            Command::RepoTree(params) => self.repo_tree(params).await,
            Command::RepoRead(params) => self.repo_read(params).await,
            Command::RepoOverview(params) => self.repo_overview(params).await,
        }
    }

    async fn search(&self, params: SearchParams) -> Result<CommandOutput, ScoutError> {
        let query = resolve_stdin_arg(params.query, "query", "<QUERY>").await?;

        info!(query = %query, "search");

        let brave = self.brave()?;
        let search_lang = params.lang.to_brave_param();
        let sources = brave.search(&query, search_lang).await?;

        info!(sources = sources.len(), "search complete");

        // Default output: one URL per line, no markdown decoration.
        // OUTCOME.md: AI agents receive raw source URLs without intermediate summary.
        let markdown = sources
            .iter()
            .map(|s| s.url.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let data = serde_json::json!({
            "query": query,
            "sources": sources,
        });
        Ok(CommandOutput::ok(markdown, data))
    }

    async fn fetch(&self, params: FetchParams) -> Result<CommandOutput, ScoutError> {
        let url = resolve_stdin_arg(params.url, "url", "<URL>").await?;

        if let Some(slack_url) = parse_slack_url(&url) {
            return self.fetch_slack(slack_url).await;
        }

        info!(url = %RedactedLogUrl(&url), js = params.js, raw = params.raw, "fetch");

        let opts = FetchOptions {
            js: params.js,
            raw: params.raw,
        };
        let result = timeout(
            FETCH_TOOL_TIMEOUT,
            fetch_page(
                &self.fetch_http,
                &url,
                opts,
                &TokioDnsResolver,
                &self.cancel,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            Err(FetchError::Timeout(format!(
                "fetch timed out after {}s",
                FETCH_TOOL_TIMEOUT.as_secs()
            )))
        })?;

        if result.used_raw_fallback {
            warn!(url = %RedactedLogUrl(&url), "readability extraction failed, using raw fallback");
        }

        info!(url = %RedactedLogUrl(&url), "fetch complete");
        let markdown = format_fetch_output(&result);
        let data = serde_json::to_value(&result).expect("FetchResult is Serialize");
        let mut degradation = Degradation::default();
        if result.used_raw_fallback {
            degradation.push(
                String::from(
                    "Readability extraction failed; raw page conversion was used instead.",
                ),
                DegradedReason::ReadabilityFallback,
            );
        }
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }

    async fn fetch_slack(&self, slack_url: SlackUrl) -> Result<CommandOutput, ScoutError> {
        info!(workspace = %slack_url.workspace, channel = %slack_url.channel, "fetch (slack)");
        let client = SlackClient::from_env(self.http.clone())?;
        let output = timeout(SLACK_TOOL_TIMEOUT, client.fetch_message(&slack_url))
            .await
            .unwrap_or_else(|_| {
                Err(SlackError::Timeout(format!(
                    "slack fetch timed out after {}s",
                    SLACK_TOOL_TIMEOUT.as_secs()
                )))
            })
            .inspect_err(|e| {
                warn!(workspace = %slack_url.workspace, channel = %slack_url.channel, error = %e, "slack fetch failed");
            })?;
        info!(workspace = %slack_url.workspace, channel = %slack_url.channel, "fetch (slack) complete");
        let markdown = truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES).into_owned();
        let data = serde_json::json!({
            "url": slack_url.raw_url,
            "markdown": markdown,
        });
        Ok(CommandOutput::ok(markdown, data))
    }

    async fn research(&self, params: ResearchParams) -> Result<CommandOutput, ScoutError> {
        let query = resolve_stdin_arg(params.query, "query", "<QUERY>").await?;

        info!(query = %query, depth = params.depth, "research");

        let brave = self.brave()?;
        let req = engine::ResearchRequest {
            query: &query,
            depth: params.depth,
            lang: params.lang,
        };

        let mut degradation = Degradation::default();

        let report = match timeout(
            RESEARCH_TOOL_TIMEOUT,
            engine::research(
                brave,
                &self.fetch_http,
                &req,
                &TokioDnsResolver,
                &self.cancel,
            ),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) if e.is_degradable() => {
                warn!(error = %e, "Brave search failed; returning degraded report");
                degradation.push(
                    format!("Brave search failed: {e}"),
                    DegradedReason::BraveSearchFailed,
                );
                engine::ResearchReport {
                    fetched_pages: vec![],
                    failed_urls: vec![],
                    sources: vec![],
                }
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                return Err(ScoutError::timeout(format!(
                    "research timed out after {}s",
                    RESEARCH_TOOL_TIMEOUT.as_secs()
                )));
            }
        };

        info!(
            pages = report.fetched_pages.len(),
            failed = report.failed_urls.len(),
            sources = report.sources.len(),
            "research complete"
        );

        let markdown = engine::format_report(&report, &query);
        let mut data = serde_json::to_value(&report).expect("ResearchReport is Serialize");
        if let Some(map) = data.as_object_mut() {
            map.insert("query".to_owned(), serde_json::Value::String(query));
        }
        collect_research_degradations(&report, &mut degradation);
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }

    async fn repo_tree(&self, params: RepoTreeParams) -> Result<CommandOutput, ScoutError> {
        let repository = resolve_stdin_arg(params.repository, "repository", "<OWNER/REPO>").await?;

        let (owner, repo) = parse_repo_param(&repository)?;

        info!(repository = %repository, "repo_tree");

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

        let markdown = github::format::format_tree(owner, repo, &ref_, &filtered, tree.truncated);
        let data = serde_json::json!({
            "owner": owner,
            "repo": repo,
            "ref": ref_,
            "entries": filtered,
            "truncated": tree.truncated,
        });

        info!(files = filtered.len(), "repo_tree complete");
        Ok(CommandOutput::ok(markdown, data))
    }

    async fn repo_read(&self, params: RepoReadParams) -> Result<CommandOutput, ScoutError> {
        let is_terminal = stdin().is_terminal();
        let content = read_stdin(
            params.repository.as_deref() == Some("-")
                || params.path.as_deref() == Some("-")
                || (!is_terminal && (params.repository.is_none() || params.path.is_none())),
        )
        .await?;
        let mut resolver = StdinResolver {
            is_terminal,
            content,
            stdin_consumed: false,
        };
        let repository = resolver.resolve(params.repository, "repository", "<OWNER/REPO>")?;
        let path = resolver.resolve(params.path, "path", "<FILE_PATH>")?;

        let (owner, repo) = parse_repo_param(&repository)?;

        info!(repository = %repository, path = %path, "repo_read");

        github::validate_path(&path)?;
        if let Some(ref r) = params.ref_ {
            github::validate_ref(r)?;
        }

        let github = self.github().await;

        let contents = match github
            .get_contents(owner, repo, &path, params.ref_.as_deref())
            .await
        {
            Ok(c) => c,
            Err(github::GitHubError::NotFound(_)) => {
                let candidates =
                    collect_path_candidates(github, owner, repo, params.ref_.as_deref(), &path)
                        .await;
                let mut err = ScoutError::from(github::GitHubError::NotFound(path.clone()));
                if !candidates.is_empty() {
                    err = err.with_candidates(candidates);
                }
                return Err(err);
            }
            Err(e) => return Err(ScoutError::from(e)),
        };

        let hint = params.encoding.as_deref();
        let decode_result =
            if let Some(encoded) = contents.content.as_ref().filter(|c| !c.is_empty()) {
                github::decode_content(encoded, hint)?
            } else {
                let blob = github.get_blob(owner, repo, &contents.sha).await?;
                github::decode_content(&blob.content, hint)?
            };
        let encoding_label = match decode_result.source {
            github::encoding::DetectionSource::AssumedUtf8 => None,
            github::encoding::DetectionSource::Detected if decode_result.encoding == "utf-8" => {
                None
            }
            _ => Some(decode_result.encoding.clone()),
        };
        let raw = decode_result.text;

        let total = raw.lines().count();
        let content = if let Some(ref range) = params.lines {
            let (start, end) = github::parse_line_range(range)?;
            github::apply_line_range(&raw, start, end)
        } else {
            github::apply_line_range(&raw, 1, None)
        };

        let markdown =
            github::format::format_file_content(&path, total, &content, encoding_label.as_deref());
        let data = serde_json::json!({
            "path": path,
            "total_lines": total,
            "content": content,
            "encoding": encoding_label,
        });

        info!(path = %path, lines = total, "repo_read complete");
        Ok(CommandOutput::ok(markdown, data))
    }

    async fn repo_overview(&self, params: RepoOverviewParams) -> Result<CommandOutput, ScoutError> {
        let repository = resolve_stdin_arg(params.repository, "repository", "<OWNER/REPO>").await?;

        let (owner, repo) = parse_repo_param(&repository)?;

        info!(repository = %repository, "repo_overview");

        let github = self.github().await;

        // Verify repo exists first: a 404 here avoids 4 wasted parallel API calls (#18).
        let repo_info = github.get_repo(owner, repo).await?;

        let (readme, issues, pulls, releases) = tokio::join!(
            github.get_readme(owner, repo),
            github.get_issues(owner, repo, OVERVIEW_ITEMS),
            github.get_pulls(owner, repo, OVERVIEW_ITEMS),
            github.get_releases(owner, repo, OVERVIEW_RELEASES),
        );

        let mut degradation = Degradation::default();
        let readme_content = resolve_readme(github, owner, repo, readme, &mut degradation).await;
        let issues =
            unwrap_or_degraded(issues, DegradedReason::IssuesFetchFailed, &mut degradation);
        let pulls = unwrap_or_degraded(pulls, DegradedReason::PullsFetchFailed, &mut degradation);
        let releases = unwrap_or_degraded(
            releases,
            DegradedReason::ReleasesFetchFailed,
            &mut degradation,
        );

        let mut markdown = github::format::format_overview(
            &repo_info,
            readme_content.as_deref(),
            &issues,
            &pulls,
            &releases,
        );

        if !degradation.is_empty() {
            markdown.push_str("\n> **Note:** ");
            markdown.push_str(&degradation.notes().join(". "));
            markdown.push_str(".\n");
        }

        // GitHub's issues endpoint returns PRs too; filter them out so JSON
        // consumers don't see PRs duplicated under issues.
        let real_issues: Vec<&github::types::IssueInfo> =
            issues.iter().filter(|i| i.pull_request.is_none()).collect();
        let data = serde_json::json!({
            "repository": repo_info,
            "readme": readme_content,
            "issues": real_issues,
            "pulls": pulls,
            "releases": releases,
        });

        info!(
            issues = issues.len(),
            pulls = pulls.len(),
            releases = releases.len(),
            has_readme = readme_content.is_some(),
            "repo_overview complete"
        );
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }
}

/// Maximum time spent on best-effort candidate generation in the NotFound
/// error path. The user is already waiting on a failure; we'd rather skip
/// candidates than block them on a slow tree fetch.
const CANDIDATE_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum OSA distance and number of suggestions returned per error.
const CANDIDATE_MAX_DISTANCE: usize = 3;
const CANDIDATE_TOP_N: usize = 3;

/// Best-effort: fetch the repo tree and return up to `CANDIDATE_TOP_N` paths
/// most similar to `target` (OSA distance ≤ `CANDIDATE_MAX_DISTANCE`).
/// Returns empty on any API failure or if the fetch exceeds
/// `CANDIDATE_FETCH_TIMEOUT`.
async fn collect_path_candidates(
    github: &GitHubClient,
    owner: &str,
    repo: &str,
    ref_: Option<&str>,
    target: &str,
) -> Vec<String> {
    let fut = async {
        let resolved_ref = match ref_ {
            Some(r) => r.to_owned(),
            None => match github.get_repo(owner, repo).await {
                Ok(info) => info.default_branch,
                Err(e) => {
                    warn!(%e, "candidate fetch: get_repo failed");
                    return Vec::new();
                }
            },
        };
        let tree = match github.get_tree(owner, repo, &resolved_ref).await {
            Ok(t) => t,
            Err(e) => {
                warn!(%e, "candidate fetch: get_tree failed");
                return Vec::new();
            }
        };
        if tree.truncated {
            warn!("candidate fetch: tree truncated (>100k entries); candidates may be incomplete");
        }
        let entries = tree
            .tree
            .iter()
            .filter(|e| matches!(e.entry_type, github::types::EntryType::Blob))
            .map(|e| e.path.as_str());
        typo::closest_matches(target, entries, CANDIDATE_MAX_DISTANCE, CANDIDATE_TOP_N)
    };
    timeout(CANDIDATE_FETCH_TIMEOUT, fut)
        .await
        .unwrap_or_default()
}

async fn resolve_readme(
    github: &GitHubClient,
    owner: &str,
    repo: &str,
    readme: Result<ContentsResponse, github::GitHubError>,
    degradation: &mut Degradation,
) -> Option<String> {
    let entry = match readme {
        Ok(r) => Some(r),
        Err(e) => {
            if !matches!(e, github::GitHubError::NotFound(_)) {
                warn!(%e, "failed to fetch README");
                degradation.push(
                    format!("Could not fetch README ({e})"),
                    DegradedReason::ReadmeFetchFailed,
                );
            }
            None
        }
    };

    let encoded = match entry {
        None => None,
        Some(r) if r.content.as_ref().is_some_and(|c| !c.is_empty()) => r.content,
        Some(r) => match github.get_blob(owner, repo, &r.sha).await {
            Ok(blob) => Some(blob.content).filter(|c| !c.is_empty()),
            Err(e) => {
                warn!(%e, "failed to fetch README blob");
                degradation.push(
                    format!("README could not be fetched ({e})"),
                    DegradedReason::ReadmeBlobFetchFailed,
                );
                None
            }
        },
    };

    encoded.and_then(|c| match github::decode_content(&c, None) {
        Ok(result) => Some(result.text),
        Err(e) => {
            warn!(%e, "failed to decode README");
            degradation.push(
                format!("README could not be decoded ({e})"),
                DegradedReason::ReadmeDecodeFailed,
            );
            None
        }
    })
}

fn collect_research_degradations(report: &engine::ResearchReport, degradation: &mut Degradation) {
    for f in &report.failed_urls {
        degradation.push(
            format!("Failed to fetch {}: {}", f.url, f.reason),
            DegradedReason::UrlFetchFailed,
        );
    }
    let raw_fallback_pages: Vec<&str> = report
        .fetched_pages
        .iter()
        .filter(|p| p.used_raw_fallback)
        .map(|p| p.url.as_str())
        .collect();
    if !raw_fallback_pages.is_empty() {
        degradation.push(
            format!(
                "Readability extraction failed for: {}",
                raw_fallback_pages.join(", ")
            ),
            DegradedReason::ReadabilityFallback,
        );
    }
}

fn format_fetch_output(result: &FetchResult) -> String {
    let shifted = shift_headings(&result.markdown, 2);
    let output = if result.used_raw_fallback {
        format!("{RAW_FALLBACK_NOTE}{shifted}")
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
            .redirect(Policy::limited(MAX_REDIRECTS))
            .build()
            .unwrap();
        let fetch_http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .unwrap();
        (http, fetch_http)
    }

    fn scout_with_brave(brave_uri: &str) -> Scout {
        scout_with_github(brave_uri, "http://localhost:0")
    }

    /// [T-009] search returns plain URL list with no markdown decoration
    #[tokio::test]
    async fn search_returns_plain_url_list() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://rust-lang.org", "title": "Rust", "description": "snippet"},
                        {"url": "https://doc.rust-lang.org", "title": "Docs", "description": "more"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = SearchParams {
            query: Some("What is Rust?".into()),
            lang: Lang::Auto,
        };

        let result = s.search(params).await.unwrap();
        assert_eq!(
            result.markdown, "https://rust-lang.org\nhttps://doc.rust-lang.org",
            "stdout should be one URL per line, no markdown decoration"
        );
    }

    /// [T-009-json] search --json output schema (data.query, data.sources, no data.answer)
    #[tokio::test]
    async fn search_json_schema_omits_answer() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://a.com", "title": "A", "description": "d"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = SearchParams {
            query: Some("foo".into()),
            lang: Lang::Auto,
        };

        let result = s.search(params).await.unwrap();
        let data = &result.data;
        assert!(data.get("answer").is_none(), "answer field must be absent");
        assert_eq!(data["query"], "foo");
        assert!(data["sources"].is_array());
        assert_eq!(data["sources"][0]["url"], "https://a.com");
        assert_eq!(data["sources"][0]["title"], "A");
        assert_eq!(data["sources"][0]["description"], "d");
    }

    /// [T-015] search command issues exactly one Brave call (no engine::research fanout)
    /// Engine path adds fetch + report; search must remain a single Brave round-trip.
    #[tokio::test]
    async fn search_does_not_traverse_engine_path() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {"results": [{"url": "https://a.com", "title": "A", "description": ""}]}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = SearchParams {
            query: Some("foo".into()),
            lang: Lang::Auto,
        };
        s.search(params).await.unwrap();
    }

    /// [T-009-empty] search with zero results returns empty stdout and exit 0
    #[tokio::test]
    async fn search_zero_results_returns_empty() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {"results": []}
            })))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = SearchParams {
            query: Some("foo".into()),
            lang: Lang::Auto,
        };
        let result = s.search(params).await.unwrap();
        assert_eq!(result.markdown, "", "empty stdout for zero results");
        assert_eq!(result.data["sources"].as_array().unwrap().len(), 0);
    }

    /// [T-TS002] research returns report with Brave sources and no obsolete Search Result header
    #[tokio::test]
    async fn research_success_returns_report() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        // Brave search response. The URL is unreachable, so fetch will fail and land in
        // failed_urls, but the Sources section still proves the Brave URL flowed through.
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://rust-lang.test/", "title": "Rust Language", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = ResearchParams {
            query: Some("What is Rust?".into()),
            depth: 1,
            lang: Lang::Auto,
        };

        let result = s.research(params).await.unwrap();
        assert!(
            result.markdown.contains("rust-lang.test"),
            "report should reference Brave source URL, got: {result:?}"
        );
        assert!(
            !result.markdown.contains("## Search Result"),
            "AC-3.1: report must not contain the obsolete Search Result header"
        );
        assert!(
            !result.markdown.contains("vertexaisearch.cloud.google.com"),
            "AC-3.2: Sources must not contain Google redirect URLs"
        );
    }

    /// [T-10] AC-4.2: --json research data schema (query, sources, fetched_pages, failed_urls)
    #[tokio::test]
    async fn research_json_schema_includes_required_keys() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://a.test/", "title": "A", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = ResearchParams {
            query: Some("foo".into()),
            depth: 1,
            lang: Lang::Auto,
        };
        let result = s.research(params).await.unwrap();
        let data = &result.data;

        assert_eq!(data["query"], "foo", "data.query must echo the request");
        assert!(data["sources"].is_array(), "data.sources must be an array");
        assert_eq!(data["sources"][0]["url"], "https://a.test/");
        assert_eq!(data["sources"][0]["title"], "A");
        assert_eq!(data["sources"][0]["description"], "snippet");
        assert!(
            data["fetched_pages"].is_array(),
            "data.fetched_pages must be an array (possibly empty)"
        );
        assert!(
            data["failed_urls"].is_array(),
            "data.failed_urls must be an array (possibly empty)"
        );
        assert!(
            data.get("answer").is_none(),
            "data.answer must be absent (AC-4.1: no LLM-generated answer)"
        );
        assert!(
            data.get("all_sources").is_none(),
            "data.all_sources is the legacy key — must be renamed to sources"
        );
    }

    /// [T-028] (unit / FR-019)
    /// Setup: wiremock always returns HTTP 503 (still fails after retry).
    /// Action: `Scout::research(...)` is invoked.
    /// Expected: returns `Ok(CommandOutput)` (no hard-fail);
    /// `degraded_reasons` contains `BraveSearchFailed`; `data.sources` is empty.
    /// RC-03 fix: cascade no longer propagates `BraveError`; failure is absorbed
    /// into the degraded report envelope.
    #[tokio::test]
    async fn research_brave_failure_returns_degraded_report() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let result = s
            .research(ResearchParams {
                query: Some("foo".into()),
                depth: 1,
                lang: Lang::Auto,
            })
            .await
            .expect("research should yield Ok(degraded) on Brave failure, not propagate error");

        assert!(
            result
                .degraded_reasons
                .contains(&DegradedReason::BraveSearchFailed),
            "degraded_reasons must contain BraveSearchFailed; got: {:?}",
            result.degraded_reasons
        );
        let data = &result.data;
        assert_eq!(
            data["sources"].as_array().unwrap().len(),
            0,
            "data.sources must be empty when Brave failed"
        );
    }

    /// [T-029] (unit / FR-019)
    /// Setup: wiremock always returns HTTP 401.
    /// Action: `Scout::research(...)` is invoked.
    /// Expected: returns `Err(ScoutError)` (not a degraded `Ok`), because
    /// `BraveError::Unauthorized` is a configuration error and must surface to
    /// the user instead of being silently absorbed into the degraded envelope.
    /// Companion to T-028 which covers the transient (503) degradable path.
    #[tokio::test]
    async fn research_unauthorized_propagates_as_error() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let result = s
            .research(ResearchParams {
                query: Some("foo".into()),
                depth: 1,
                lang: Lang::Auto,
            })
            .await;

        assert!(
            result.is_err(),
            "Unauthorized must propagate as Err, not be degraded; got: {result:?}"
        );
    }

    /// [T-11] AC-4.3: zero results yield empty arrays, not null
    #[tokio::test]
    async fn research_json_zero_results_returns_empty_arrays() {
        let Some(server) = try_spawn_mock_server("tools::integration").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {"results": []}
            })))
            .mount(&server)
            .await;

        let s = scout_with_brave(&server.uri());
        let params = ResearchParams {
            query: Some("foo".into()),
            depth: 1,
            lang: Lang::Auto,
        };
        let result = s.research(params).await.unwrap();
        let data = &result.data;

        assert_eq!(
            data["sources"].as_array().unwrap().len(),
            0,
            "data.sources must be an empty array (not null)"
        );
        assert_eq!(
            data["fetched_pages"].as_array().unwrap().len(),
            0,
            "data.fetched_pages must be an empty array"
        );
        assert_eq!(
            data["failed_urls"].as_array().unwrap().len(),
            0,
            "data.failed_urls must be an empty array"
        );
    }

    /// [T-TS003] fetch_output_shifts_headings
    #[test]
    fn fetch_output_shifts_headings() {
        let result = FetchResult {
            url: "https://example.com".into(),
            markdown: "# Title\n## Section\nContent".into(),
            used_raw_fallback: false,
        };
        let output = format_fetch_output(&result);
        assert!(output.contains("### Title"), "h1 should shift to h3");
        assert!(output.contains("#### Section"), "h2 should shift to h4");
    }

    /// [T-TS004] fetch_output_shifts_headings_with_raw_fallback
    #[test]
    fn fetch_output_shifts_headings_with_raw_fallback() {
        let result = FetchResult {
            url: "https://example.com".into(),
            markdown: "# Raw Title\nBody".into(),
            used_raw_fallback: true,
        };
        let output = format_fetch_output(&result);
        assert!(
            output.starts_with(RAW_FALLBACK_NOTE.trim_end()),
            "should prepend fallback note"
        );
        assert!(output.contains("### Raw Title"), "h1 should shift to h3");
    }

    /// [T-TS005] fetch_output_truncates_long_content
    #[test]
    fn fetch_output_truncates_long_content() {
        let result = FetchResult {
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

    fn scout_with_github(brave_uri: &str, github_uri: &str) -> Scout {
        let (http, fetch_http) = build_test_clients();
        let cell = OnceCell::new();
        cell.set(GitHubClient::with_base_url(http.clone(), github_uri))
            .ok();
        Scout {
            http: http.clone(),
            fetch_http,
            brave: Some(BraveClient::with_base_url(http, brave_uri)),
            github: cell,
            cancel: Arc::new(Notify::new()),
        }
    }

    fn scout_lazy(brave_uri: &str) -> Scout {
        let (http, fetch_http) = build_test_clients();
        Scout {
            http: http.clone(),
            fetch_http,
            brave: Some(BraveClient::with_base_url(http, brave_uri)),
            github: OnceCell::new(),
            cancel: Arc::new(Notify::new()),
        }
    }

    /// [T-TS009] repo_overview: get_repo 404 -> readme/issues/pulls/releases
    /// APIs receive 0 requests.
    #[tokio::test]
    async fn repo_overview_404_skips_remaining_apis() {
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
            repository: Some("owner/nonexistent".into()),
        };

        let result = s.repo_overview(params).await;
        assert!(result.is_err(), "repo_overview should fail on 404");

        // wiremock verifies expect(0) on server drop
    }

    /// [T-TS010] repo_overview: after get_repo succeeds, readme/issues/pulls/
    /// releases run in parallel.
    ///
    /// Proof: a barrier-synchronized TCP server requires all 4 API requests to
    /// arrive before any response is sent. If requests are sequential, only one
    /// arrives at a time and the barrier never releases → deadlock → timeout.
    #[tokio::test(flavor = "multi_thread")]
    async fn repo_overview_parallel_after_get_repo() {
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::Barrier;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

        // 4 parallel APIs must all arrive before any response is sent.
        let barrier = Arc::new(Barrier::new(4));

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
                        (r#"{"full_name":"owner/repo","description":"test","html_url":"https://github.com/owner/repo","default_branch":"main","language":"Rust","stargazers_count":1,"forks_count":0,"open_issues_count":0,"topics":[],"license":null}"#.to_owned(), false)
                    } else if path.contains("/git/blobs/") {
                        (r#"{"content":""}"#.to_owned(), false)
                    } else if path.contains("/readme") {
                        (r#"{"sha":"abc123","content":""}"#.to_owned(), true)
                    } else {
                        ("[]".to_owned(), true)
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
            repository: Some("owner/repo".into()),
        };

        // Parallel: barrier(4) releases instantly → completes in ms.
        // Sequential: barrier never reaches 4 → deadlock → timeout.
        let result = timeout(Duration::from_secs(5), s.repo_overview(params)).await;

        assert!(
            result.is_ok(),
            "repo_overview should complete when 4 APIs run in parallel \
             (barrier-synchronized); sequential execution deadlocks"
        );

        server.abort();
    }

    /// [T-TS011] scout_lazy: github OnceCell is None immediately after
    /// construction.
    #[test]
    fn scout_lazy_github_initially_none() {
        let s = scout_lazy("http://localhost:0");
        assert!(
            s.github.get().is_none(),
            "github OnceCell should be uninitialized after scout_lazy()"
        );
    }

    /// [T-TS012] search command does not initialize the GitHub client on the success path.
    #[tokio::test]
    async fn search_leaves_github_uninitialized() {
        let Some(server) = try_spawn_mock_server("tools::t_004").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://example.com", "title": "Example", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let s = scout_lazy(&server.uri());
        let result = s
            .search(SearchParams {
                query: Some("test".into()),
                lang: Lang::En,
            })
            .await;

        assert!(
            result.is_ok(),
            "search should succeed against Brave mock; got: {:?}",
            result.err()
        );
        assert!(
            s.github.get().is_none(),
            "search should not initialize GitHubClient on the success path"
        );
    }

    /// [T-TS013] fetch command does not initialize the GitHub client.
    #[tokio::test]
    async fn fetch_leaves_github_uninitialized() {
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
                url: Some(format!("{}/page", server.uri())),
                js: false,
                raw: false,
            })
            .await;

        assert!(
            s.github.get().is_none(),
            "fetch should not initialize GitHubClient"
        );
    }

    /// [T-TS014] research command does not initialize the GitHub client on the success path.
    /// Brave succeeds; the fetched URL is invalid (DNS failure), driving a degraded
    /// ResearchReport (Ok) without touching GitHub.
    #[tokio::test]
    async fn research_leaves_github_uninitialized() {
        let Some(server) = try_spawn_mock_server("tools::t_006").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://nonexistent.invalid", "title": "Example", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let s = scout_lazy(&server.uri());
        let result = s
            .research(ResearchParams {
                query: Some("test".into()),
                depth: 1,
                lang: Lang::En,
            })
            .await;

        assert!(
            result.is_ok(),
            "research should return Ok (degraded report) even when fetch fails; got: {:?}",
            result.err()
        );
        assert!(
            s.github.get().is_none(),
            "research should not initialize GitHubClient on the success path"
        );
    }

    /// [T-TS015] github() called twice returns the same reference
    /// (OnceCell caching verified via std::ptr::eq).
    #[tokio::test]
    async fn github_returns_same_reference() {
        use std::ptr;
        // Use pre-set OnceCell to avoid triggering real `gh auth token` subprocess.
        let s = scout_with_github("http://localhost:0", "http://localhost:0");
        let first = s.github().await;
        let second = s.github().await;
        assert!(
            ptr::eq(first, second),
            "github() should return the same cached reference on second call"
        );
    }

    /// [T-TS016] github() initializes an empty OnceCell via from_env and caches
    /// the result. Exercises the lazy-init code path at mod.rs:80-84.
    ///
    /// from_env is infallible: it resolves token from env vars or `gh auth token`
    /// (with TOKEN_RESOLVE_TIMEOUT = 5s), then returns a client. No timeout
    /// wrapper — a hang here is a real bug, not a flaky environment.
    #[tokio::test]
    async fn github_lazy_init_from_empty_cell() {
        use std::ptr;
        let s = scout_lazy("http://localhost:0");
        assert!(s.github.get().is_none(), "starts empty");

        let client = s.github().await;

        assert!(
            s.github.get().is_some(),
            "OnceCell should be initialized after github() call"
        );
        let client2 = s.github().await;
        assert!(
            ptr::eq(client, client2),
            "second call returns the same cached reference"
        );
    }

    /// [T-TS017] repo_read: --encoding hint is passed to decode_content and
    /// used to decode non-UTF-8 content correctly.
    #[tokio::test]
    async fn repo_read_decodes_with_encoding_hint() {
        let Some(server) = try_spawn_mock_server("tools::t_008").await else {
            return;
        };

        // "テスト" in Shift_JIS ([0x83, 0x65, 0x83, 0x58, 0x83, 0x67]), base64-encoded.
        // Without --encoding, chardetng auto-detects Shift_JIS for 6 bytes too.
        // With --encoding shift_jis, decode_explicit is used (deterministic).
        let shift_jis_b64 = "g2WDWINn";

        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/contents/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sha": "abc123",
                "content": shift_jis_b64
            })))
            .mount(&server)
            .await;

        let s = scout_with_github("http://localhost:0", &server.uri());
        let params = RepoReadParams {
            repository: Some("owner/repo".into()),
            path: Some("test.txt".into()),
            ref_: None,
            lines: None,
            encoding: Some("shift_jis".into()),
        };

        let result = s.repo_read(params).await.unwrap();
        assert!(
            result.markdown.contains("テスト"),
            "output should contain decoded Shift_JIS text, got: {result:?}"
        );
        assert!(
            result.markdown.contains("[encoding: shift_jis]"),
            "header should include encoding label, got: {result:?}"
        );
    }

    /// [T-TS018] repo_tree: --path filter is wired through RepoTreeParams to
    /// filter_tree_entries; files outside the prefix are excluded from output.
    #[tokio::test]
    async fn repo_tree_path_filter_excludes_non_matching_files() {
        let Some(server) = try_spawn_mock_server("tools::t_009").await else {
            return;
        };

        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "full_name": "owner/repo",
                "description": null,
                "html_url": "https://github.com/owner/repo",
                "default_branch": "main",
                "language": null,
                "stargazers_count": 0,
                "forks_count": 0,
                "open_issues_count": 0,
                "topics": null,
                "license": null
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/git/trees/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tree": [
                    {"path": "src/main.rs", "type": "blob", "size": 100},
                    {"path": "src/lib.rs", "type": "blob", "size": 200},
                    {"path": "README.md", "type": "blob", "size": 50},
                    {"path": "Cargo.toml", "type": "blob", "size": 80},
                ],
                "truncated": false
            })))
            .mount(&server)
            .await;

        let s = scout_with_github("http://localhost:0", &server.uri());
        let params = RepoTreeParams {
            repository: Some("owner/repo".into()),
            ref_: None,
            path: Some("src/".into()),
            pattern: None,
        };

        let result = s.repo_tree(params).await.unwrap();
        assert!(
            result.markdown.contains("src/main.rs"),
            "path filter should include src/main.rs, got:\n{result:?}"
        );
        assert!(
            !result.markdown.contains("README.md"),
            "path filter should exclude README.md, got:\n{result:?}"
        );
        assert!(
            !result.markdown.contains("Cargo.toml"),
            "path filter should exclude Cargo.toml, got:\n{result:?}"
        );
    }

    /// [T-R001] StdinResolver: first arg consumes stdin, second uses its own value
    #[test]
    fn stdin_resolver_first_consumes_second_uses_arg() {
        let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
        let first = r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
        assert_eq!(first, "from_stdin");
        let second = r
            .resolve(Some("test.txt".into()), "path", "<FILE_PATH>")
            .unwrap();
        assert_eq!(second, "test.txt");
    }

    /// [T-R002] StdinResolver: arg wins over stdin, stdin preserved for next resolve
    #[test]
    fn stdin_resolver_arg_wins_stdin_preserved() {
        let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
        let first = r
            .resolve(Some("owner/repo".into()), "repository", "<OWNER/REPO>")
            .unwrap();
        assert_eq!(first, "owner/repo");
        let second = r.resolve(None, "path", "<FILE_PATH>").unwrap();
        assert_eq!(second, "from_stdin");
    }

    /// [T-R003] StdinResolver: second arg fails when stdin already consumed
    #[test]
    fn stdin_resolver_consumed_stdin_fails_second() {
        let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
        r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
        let result = r.resolve(None, "path", "<FILE_PATH>");
        assert!(
            result.is_err(),
            "second positional should fail when stdin consumed"
        );
    }

    /// [T-R005] StdinResolver: error message hints stdin was consumed, not missing
    #[test]
    fn stdin_resolver_consumed_error_hints_stdin_exhausted() {
        let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
        r.resolve(None, "repository", "<OWNER/REPO>").unwrap();
        let err = r
            .resolve(None, "path", "<FILE_PATH>")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("stdin was already read"),
            "error should hint stdin was consumed, got: {err}"
        );
        assert!(
            !err.contains("pipe it via stdin"),
            "error should not suggest piping when stdin is exhausted, got: {err}"
        );
    }

    /// [T-R004] StdinResolver: both args provided, stdin unused
    #[test]
    fn stdin_resolver_both_args_stdin_unused() {
        let mut r = super::StdinResolver::with_content(false, Some("from_stdin".into()));
        let first = r
            .resolve(Some("owner/repo".into()), "repository", "<OWNER/REPO>")
            .unwrap();
        let second = r
            .resolve(Some("test.txt".into()), "path", "<FILE_PATH>")
            .unwrap();
        assert_eq!(first, "owner/repo");
        assert_eq!(second, "test.txt");
    }
}
