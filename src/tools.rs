mod builder;
mod config;
mod errors;
mod params;
mod query;
mod repo;
mod typo;

pub(crate) use errors::ScoutError;
pub(crate) use params::Command;

use builder::ScoutBuilder;
use config::RuntimeConfig;

use std::borrow::Cow;
use std::future::Future;
use std::io::{IsTerminal, stdin};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use tokio::io::{AsyncReadExt, stdin as tokio_stdin};
use tokio::sync::{OnceCell, watch};
use tokio::time::timeout;
use tracing::warn;

use params::resolve_input;

use crate::brave::client::{BraveClient, BraveError};
use crate::clock::Clock;
use crate::envelope::CommandOutput;
use crate::fetch::converter::{DECODE_UNCERTAIN_NOTE, FetchResult, RAW_FALLBACK_NOTE};
use crate::fetch::{DnsResolver, EgressMode};
use crate::github::GitHubClient;
use crate::rng::Rng;
use crate::slack::SlackClient;
use crate::token_source::TokenSource;
use crate::yaml::truncate_and_reneutralize;

#[cfg(test)]
use crate::envelope::DegradedReason;
#[cfg(test)]
use crate::github;
#[cfg(test)]
use params::{
    FetchParams, RepoOverviewParams, RepoReadParams, RepoTreeParams, ResearchParams, SearchParams,
};

const MAX_STDIN_BYTES: u64 = 1_048_576;
/// Upper bound for waiting on piped input. Without this, a stalled or
/// half-closed pipe (upstream writer hung mid-stream) would block scout
/// indefinitely with no log output.
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
    .map_err(|e| ScoutError::user_error(format!("failed to read stdin: {e}")))?;
    let trimmed = buf.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    })
}

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
                    "no {label} provided. Pass {placeholder} as an argument (stdin was already read by the previous argument)"
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

const MAX_FETCH_OUTPUT_BYTES: usize = 100_000;

pub(crate) struct Scout {
    http: Client,
    /// HTTP client with redirect following disabled for SSRF-safe fetching.
    /// Used by `fetch_page` which handles redirects manually with per-hop SSRF checks.
    fetch_http: Client,
    brave: Option<BraveClient>,
    /// Lazy-initialized on first GitHub API call. Non-GitHub commands
    /// (search, fetch, research) never pay the `gh auth token` cost.
    github: OnceCell<GitHubClient>,
    /// Lazy-initialized on first Slack permalink fetch. Mirrors `github`:
    /// non-Slack commands never read `SLACK_TOKEN`. Tests pre-set the cell via
    /// `ScoutBuilder::with_slack_endpoint`.
    slack: OnceCell<SlackClient>,
    /// Sticky shutdown flag delivered even to fetches queued after SIGINT/SIGTERM.
    /// Each CDP invocation subscribes afresh; a Notify would lose earlier wakeups.
    cancel: watch::Sender<bool>,
    /// Tunables overridable via `SCOUT_*` env vars.
    config: RuntimeConfig,
    /// Forwarded into `GitHubClient` on first `github()` call. Held on `Scout`
    /// rather than constructed inside `github()` so tests can inject before
    /// the `OnceCell` initializes.
    clock: Arc<dyn Clock>,
    /// Injected retry randomness for lazy GitHub initialization.
    rng: Arc<dyn Rng>,
    /// GitHub bearer token resolver, awaited inside `github()` lazy init.
    /// Held on `Scout` so tests can swap in a `StaticTokenSource` before any
    /// API call spawns the production `gh auth token` subprocess.
    token_source: Arc<dyn TokenSource>,
    /// DNS resolver consulted by the SSRF pre-check on every fetch. Held on
    /// `Scout` so tests can swap a scripted resolver before any real DNS
    /// lookup runs.
    dns: Arc<dyn DnsResolver>,
    /// Egress mode detected once at construction (`ScoutBuilder::from_env`) and
    /// forwarded into every `fetch` via `FetchOptions.egress`. `Proxied` makes
    /// `fetch_page` skip the DNS pre-check and route through the proxy that
    /// `fetch_http` was built with; `Direct` keeps scout resolving and dialing.
    egress: EgressMode,
}

impl Scout {
    /// Production entry point; async for existing `Scout::new().await` callers.
    pub(crate) async fn new() -> Result<Self, ScoutError> {
        Ok(ScoutBuilder::from_env()?.build())
    }

    /// Share shutdown state with `lib::run` without borrowing `Scout`.
    pub(crate) fn cancel_handle(&self) -> watch::Sender<bool> {
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

    /// Defer the fallible Slack token read until the first Slack fetch.
    /// Tests can pre-set the client via ScoutBuilder.
    async fn slack(&self) -> Result<&SlackClient, ScoutError> {
        self.slack
            .get_or_try_init(|| async {
                SlackClient::from_env(self.http.clone(), self.config.max_retries)
                    .map(|c| c.with_clock(self.clock.clone()).with_rng(self.rng.clone()))
                    .map_err(ScoutError::from)
            })
            .await
    }

    /// Cap the whole GitHub command: several calls can each exhaust their
    /// HTTP timeout and retry budget, otherwise taking minutes in total.
    async fn with_github_timeout<F>(&self, label: &str, fut: F) -> Result<CommandOutput, ScoutError>
    where
        F: Future<Output = Result<CommandOutput, ScoutError>>,
    {
        timeout(self.config.github_timeout, fut)
            .await
            .unwrap_or_else(|_| {
                warn!(
                    command = label,
                    timeout_secs = self.config.github_timeout.as_secs(),
                    "github command timed out"
                );
                Err(ScoutError::timeout(format!(
                    "{label} timed out after {}s",
                    self.config.github_timeout.as_secs()
                )))
            })
    }

    pub(crate) async fn run(&self, cmd: Command) -> Result<CommandOutput, ScoutError> {
        match cmd {
            Command::Search(params) => self.search(params).await,
            Command::Fetch(params) => self.fetch(params).await,
            Command::Research(params) => self.research(params).await,
            Command::RepoTree(params) => {
                self.with_github_timeout("repo-tree", self.repo_tree(params))
                    .await
            }
            Command::RepoRead(params) => {
                self.with_github_timeout("repo-read", self.repo_read(params))
                    .await
            }
            Command::RepoOverview(params) => {
                self.with_github_timeout("repo-overview", self.repo_overview(params))
                    .await
            }
        }
    }
}

/// Put degradation notes in the body so non-JSON callers also see them.
/// Match research ordering: raw fallback before decoding uncertainty.
fn format_fetch_output(result: &FetchResult) -> String {
    let body = result.with_heading_offset(2);
    let output = if !result.used_raw_fallback() && !result.decode_uncertain() {
        body.into_owned()
    } else {
        let mut output = String::new();
        if result.used_raw_fallback() {
            output.push_str(RAW_FALLBACK_NOTE);
        }
        if result.decode_uncertain() {
            output.push_str(DECODE_UNCERTAIN_NOTE);
        }
        output.push_str(&body);
        output
    };

    match truncate_and_reneutralize(&output, MAX_FETCH_OUTPUT_BYTES) {
        Cow::Borrowed(_) => output,
        Cow::Owned(truncated) => truncated,
    }
}

#[cfg(test)]
mod builder_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod repo_io_tests;
#[cfg(test)]
mod repo_lazy_tests;
#[cfg(test)]
mod stdin_tests;
#[cfg(test)]
mod test_helpers;
