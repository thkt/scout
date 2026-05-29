mod config;
mod errors;
mod params;
mod typo;

pub use errors::ScoutError;
pub use params::Command;

pub(crate) use config::RuntimeConfig;
pub(crate) use errors::Classification;

use std::io::{IsTerminal, stdin};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use reqwest::redirect::Policy;
use tokio::io::{AsyncReadExt, stdin as tokio_stdin};
use tokio::sync::{OnceCell, watch};
use tokio::time::timeout;
use tracing::{info, warn};

use errors::{parse_repo_param, unwrap_or_degraded};
use params::{
    FetchParams, RepoOverviewParams, RepoReadParams, RepoTreeParams, ResearchParams, SearchParams,
    resolve_input,
};

use crate::brave::client::{BraveClient, BraveError, SearchClient as _};
use crate::clock::{Clock, SystemClock};
use crate::envelope::{CommandOutput, Degradation, DegradedReason};
use crate::fetch::converter::{FetchResult, RAW_FALLBACK_NOTE};
use crate::fetch::{
    DnsResolver, FetchError, FetchOptions, RedactedLogUrl, TokioDnsResolver, fetch_page,
};
use crate::github::types::ContentsResponse;
use crate::github::{self, GitHubClient, PerPage};
use crate::markdown::{shift_headings, truncate_with_note};
use crate::rng::{FastrandRng, Rng};
use crate::search::engine;
use crate::slack::{SlackClient, SlackError, SlackUrl, parse_slack_url};
use crate::token_source::{GhCliSource, TokenSource};

const MAX_STDIN_BYTES: u64 = 1_048_576;
/// Upper bound for waiting on piped input. Without this, a stalled or
/// half-closed pipe (upstream writer hung mid-stream) would block scout
/// indefinitely with no log output (issue #155 / CHX-006).
const STDIN_READ_TIMEOUT: Duration = Duration::from_secs(30);

async fn read_stdin(needs_stdin: bool) -> Result<Option<String>, ScoutError> {
    if !needs_stdin {
        return Ok(None);
    }
    let mut buf = String::new();
    timeout(
        STDIN_READ_TIMEOUT,
        tokio_stdin().take(MAX_STDIN_BYTES).read_to_string(&mut buf),
    )
    .await
    .map_err(|_| {
        warn!(
            timeout_secs = STDIN_READ_TIMEOUT.as_secs(),
            "stdin read timed out"
        );
        ScoutError::user_error(format!(
            "stdin read timed out after {}s; upstream writer may be stalled",
            STDIN_READ_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| ScoutError::user_error(format!("Failed to read stdin: {e}")))?;
    let trimmed = buf.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    })
}

/// Lifecycle of the once-readable stdin buffer. Variants are mutually
/// exclusive so `Available` and `Consumed` cannot coexist.
enum StdinState {
    /// stdin was a TTY, or read_stdin returned empty after trim.
    NotPiped,
    /// stdin content was buffered and no `resolve()` has taken it yet.
    Available(String),
    /// A previous `resolve()` took the buffered content.
    Consumed,
}

/// Resolves CLI positional args with stdin fallback.
/// Stdin is read once; the first arg that needs it consumes it.
struct StdinResolver {
    is_terminal: bool,
    state: StdinState,
}

impl StdinResolver {
    fn resolve(
        &mut self,
        value: Option<String>,
        label: &str,
        placeholder: &str,
    ) -> Result<String, ScoutError> {
        let needs_stdin = value.is_none() || value.as_deref() == Some("-");
        if needs_stdin && matches!(self.state, StdinState::Consumed) {
            let msg = if value.as_deref() == Some("-") {
                format!("stdin already read — cannot use `-` for {label}")
            } else {
                format!(
                    "No {label} provided. Pass {placeholder} as an argument (stdin was already read by the previous argument)"
                )
            };
            return Err(ScoutError::user_error(msg));
        }
        let content = match &self.state {
            StdinState::Available(s) => Some(s.as_str()),
            StdinState::NotPiped | StdinState::Consumed => None,
        };
        let result = resolve_input(value, content, self.is_terminal, label, placeholder)?;
        if needs_stdin {
            self.state = StdinState::Consumed;
        }
        Ok(result)
    }

    #[cfg(test)]
    fn with_content(is_terminal: bool, content: Option<String>) -> Self {
        Self {
            is_terminal,
            state: match content {
                Some(s) => StdinState::Available(s),
                None => StdinState::NotPiped,
            },
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
const MAX_REDIRECTS: usize = 5;
const OVERVIEW_ITEMS: PerPage = PerPage::new(5);
const OVERVIEW_RELEASES: PerPage = PerPage::new(3);
const MAX_FETCH_OUTPUT_BYTES: usize = 100_000;

pub struct Scout {
    http: Client,
    /// HTTP client with redirect following disabled for SSRF-safe fetching.
    /// Used by `fetch_page` which handles redirects manually with per-hop SSRF checks.
    fetch_http: Client,
    brave: Option<BraveClient>,
    /// Lazy-initialized on first GitHub API call. Non-GitHub commands
    /// (search, fetch, research) never pay the `gh auth token` cost.
    github: OnceCell<GitHubClient>,
    /// Sticky shutdown flag. `lib::run` flips this to `true` on SIGINT or
    /// SIGTERM. Each `fetch_with_cdp` invocation subscribes a fresh receiver
    /// so the cancellation is delivered to fetches that start after the
    /// signal arrives (e.g. queued slots in `research --depth N` once an
    /// earlier slot finishes). `Notify` was tried first but loses wakeups
    /// when no waiter is registered at signal time (issue #121).
    cancel: watch::Sender<bool>,
    /// Tunables overridable via `SCOUT_*` env vars (issue #120).
    config: RuntimeConfig,
    /// Forwarded into `GitHubClient` on first `github()` call. Held on `Scout`
    /// rather than constructed inside `github()` so tests can inject before
    /// the `OnceCell` initializes.
    clock: Arc<dyn Clock>,
    /// Forwarded into `GitHubClient` on first `github()` call. Same plumbing
    /// rationale as `clock`.
    rng: Arc<dyn Rng>,
    /// GitHub bearer token resolver, awaited inside `github()` lazy init.
    /// Held on `Scout` so tests can swap in a `StaticTokenSource` before any
    /// API call spawns the production `gh auth token` subprocess.
    token_source: Arc<dyn TokenSource>,
    /// DNS resolver consulted by the SSRF pre-check on every fetch. Held on
    /// `Scout` so tests can swap a scripted resolver before any real DNS
    /// lookup runs.
    dns: Arc<dyn DnsResolver>,
}

impl Scout {
    /// Production entry point. Sugar for `ScoutBuilder::from_env()?.build()`;
    /// kept async so existing `Scout::new().await` callsites compile unchanged.
    pub async fn new() -> Result<Self, ScoutError> {
        Ok(ScoutBuilder::from_env()?.build())
    }

    /// Hand back a cloned `watch::Sender` so `lib::run` can flip the
    /// cancellation flag without keeping a reference to `Scout`. The clone
    /// shares state with every receiver subscribed from the underlying
    /// fetch paths.
    pub fn cancel_handle(&self) -> watch::Sender<bool> {
        self.cancel.clone()
    }

    async fn github(&self) -> &GitHubClient {
        self.github
            .get_or_init(|| {
                let source = self.token_source.clone();
                let clock = self.clock.clone();
                let rng = self.rng.clone();
                let http = self.http.clone();
                let max_retries = self.config.max_retries;
                async move {
                    GitHubClient::from_env_with_source(http, max_retries, source.as_ref())
                        .await
                        .with_clock(clock)
                        .with_rng(rng)
                }
            })
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
        let fetch_timeout = self.config.fetch_timeout;
        let result = timeout(
            fetch_timeout,
            fetch_page(&self.fetch_http, &url, opts, self.dns.clone(), &self.cancel),
        )
        .await
        .unwrap_or_else(|_| {
            warn!(
                url = %RedactedLogUrl(&url),
                timeout_secs = fetch_timeout.as_secs(),
                "fetch timed out"
            );
            Err(FetchError::Timeout(format!(
                "fetch timed out after {}s",
                fetch_timeout.as_secs()
            )))
        })?;

        if result.used_raw_fallback() {
            warn!(url = %RedactedLogUrl(&url), "readability extraction failed, using raw fallback");
        }

        info!(url = %RedactedLogUrl(&url), "fetch complete");
        let markdown = format_fetch_output(&result);
        let data = serde_json::to_value(&result).expect("FetchResult is Serialize");
        let mut degradation = Degradation::default();
        if result.used_raw_fallback() {
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
        info!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), "fetch (slack)");
        let client = SlackClient::from_env(self.http.clone(), self.config.max_retries)?
            .with_clock(self.clock.clone())
            .with_rng(self.rng.clone());
        let slack_timeout = self.config.slack_timeout;
        let output = timeout(slack_timeout, client.fetch_message(&slack_url))
            .await
            .unwrap_or_else(|_| {
                warn!(
                    workspace = %slack_url.workspace(),
                    channel = %slack_url.channel(),
                    timeout_secs = slack_timeout.as_secs(),
                    "slack fetch timed out"
                );
                Err(SlackError::Timeout(format!(
                    "slack fetch timed out after {}s",
                    slack_timeout.as_secs()
                )))
            })
            .inspect_err(|e| {
                warn!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), error = %e, "slack fetch failed");
            })?;
        info!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), "fetch (slack) complete");
        let markdown = truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES).into_owned();
        let data = serde_json::json!({
            "url": slack_url.raw_url(),
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

        let research_timeout = self.config.research_timeout;
        let report = match timeout(
            research_timeout,
            engine::research(
                brave,
                &self.fetch_http,
                &req,
                self.dns.clone(),
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
                warn!(
                    query = %query,
                    depth = params.depth,
                    timeout_secs = research_timeout.as_secs(),
                    "research timed out"
                );
                return Err(ScoutError::timeout(format!(
                    "research timed out after {}s",
                    research_timeout.as_secs()
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
            state: match content {
                Some(s) => StdinState::Available(s),
                None => StdinState::NotPiped,
            },
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

/// Test seam for `Scout`. Production goes through `Scout::new` (sugar for
/// `ScoutBuilder::from_env()?.build()`); tests use the `with_*` setters to
/// inject `Clock` / `Rng` / `TokenSource` doubles or to point `Brave` /
/// `GitHub` at wiremock endpoints without reaching into private fields.
/// `build` is sync + infallible so fallibility stays in `from_env`
/// (issue #103).
pub(crate) struct ScoutBuilder {
    http: Client,
    fetch_http: Client,
    brave: Option<BraveClient>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    token_source: Arc<dyn TokenSource>,
    dns: Arc<dyn DnsResolver>,
    cancel: watch::Sender<bool>,
    config: RuntimeConfig,
    /// Pre-initialize `Scout.github` (`OnceCell`) with a test client pointed at
    /// this base URL so `Scout::github()` returns it without ever calling
    /// `from_env_with_source`. `None` (production) preserves lazy init.
    #[cfg(test)]
    github_endpoint: Option<String>,
}

/// Build the two `reqwest::Client`s shared between production and test paths
/// (redirect-limited + redirect-none). Extracted so `from_env` and `for_test`
/// stay in sync — drift here would change SSRF / timeout posture asymmetrically.
fn build_default_clients() -> Result<(Client, Client), ScoutError> {
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
    Ok((http, fetch_http))
}

impl ScoutBuilder {
    /// Read `SCOUT_*` env vars, construct the two `reqwest::Client`s, and
    /// probe Brave (best-effort). Defaults for `clock` / `rng` / `token_source`
    /// match production behavior; tests override via `with_*`.
    pub(crate) fn from_env() -> Result<Self, ScoutError> {
        let config = RuntimeConfig::from_env()?;
        let (http, fetch_http) = build_default_clients()?;
        let brave = BraveClient::from_env(http.clone(), config.max_retries)
            .inspect_err(|e| warn!("Brave client not available: {e}"))
            .ok();
        Ok(Self {
            http,
            fetch_http,
            brave,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            token_source: Arc::new(GhCliSource),
            dns: Arc::new(TokioDnsResolver),
            cancel: watch::channel(false).0,
            config,
            #[cfg(test)]
            github_endpoint: None,
        })
    }

    /// Test entry point. Uses `RuntimeConfig::default()` and does NOT read
    /// `SCOUT_*` env vars so a stray `SCOUT_MAX_RETRIES=abc` in the developer
    /// environment cannot panic unrelated tests. `Client::builder().build()`
    /// only fails on TLS init, which would be a real bug — `.expect` is
    /// appropriate.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        let (http, fetch_http) = build_default_clients().expect("test client init");
        Self {
            http,
            fetch_http,
            brave: None,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            token_source: Arc::new(GhCliSource),
            dns: Arc::new(TokioDnsResolver),
            cancel: watch::channel(false).0,
            config: RuntimeConfig::default(),
            github_endpoint: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_rng(mut self, rng: Arc<dyn Rng>) -> Self {
        self.rng = rng;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_token_source(mut self, source: Arc<dyn TokenSource>) -> Self {
        self.token_source = source;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_dns(mut self, dns: Arc<dyn DnsResolver>) -> Self {
        self.dns = dns;
        self
    }

    /// Re-uses the builder's `http` so wiremock servers share the test client
    /// (avoids spawning a second `reqwest` connection pool per test).
    #[cfg(test)]
    pub(crate) fn with_brave_endpoint(mut self, endpoint: &str) -> Self {
        self.brave = Some(BraveClient::with_base_url(self.http.clone(), endpoint));
        self
    }

    /// Stores `endpoint`; `build()` uses the current `clock` / `rng` to
    /// pre-init the `OnceCell`. Composes with `with_clock` / `with_rng`
    /// (call those before `build`).
    #[cfg(test)]
    pub(crate) fn with_github_endpoint(mut self, endpoint: &str) -> Self {
        self.github_endpoint = Some(endpoint.to_owned());
        self
    }

    pub(crate) fn build(self) -> Scout {
        let github = OnceCell::new();
        #[cfg(test)]
        if let Some(endpoint) = self.github_endpoint.as_deref() {
            let _ = github.set(
                GitHubClient::with_base_url(self.http.clone(), endpoint)
                    .with_clock(self.clock.clone())
                    .with_rng(self.rng.clone()),
            );
        }
        let brave = self
            .brave
            .map(|c| c.with_clock(self.clock.clone()).with_rng(self.rng.clone()));
        Scout {
            http: self.http,
            fetch_http: self.fetch_http,
            brave,
            github,
            cancel: self.cancel,
            config: self.config,
            clock: self.clock,
            rng: self.rng,
            token_source: self.token_source,
            dns: self.dns,
        }
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
        .filter(|p| p.used_raw_fallback())
        .map(FetchResult::url)
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
    let shifted = shift_headings(result.markdown(), 2);
    let output = if result.used_raw_fallback() {
        format!("{RAW_FALLBACK_NOTE}{shifted}")
    } else {
        shifted
    };

    truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES).into_owned()
}

#[cfg(test)]
mod tests;
