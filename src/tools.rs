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
use crate::markdown::{shift_headings, truncate_with_note};
use crate::rng::Rng;
use crate::slack::SlackClient;
use crate::token_source::TokenSource;
use crate::yaml::reneutralize_dangling_fence;

// Re-imported under `cfg(test)` so the in-module test files (which reach them
// via `use super::*`) keep compiling after the command methods that used them
// in production moved to `query` / `repo`.
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
    .map_err(|e| ScoutError::user_error(format!("failed to read stdin: {e}")))?;
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

    /// The one place `Option<String>` maps onto [`StdinState`]. Keeping it here
    /// rather than at the call site means adding or renaming a variant touches a
    /// single mapping.
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
    /// Egress mode detected once at construction (`ScoutBuilder::from_env`) and
    /// forwarded into every `fetch` via `FetchOptions.egress`. `Proxied` makes
    /// `fetch_page` skip the DNS pre-check and route through the proxy that
    /// `fetch_http` was built with; `Direct` keeps scout resolving and dialing.
    egress: EgressMode,
}

impl Scout {
    /// Production entry point. Sugar for `ScoutBuilder::from_env()?.build()`;
    /// kept async so existing `Scout::new().await` callsites compile unchanged.
    pub(crate) async fn new() -> Result<Self, ScoutError> {
        Ok(ScoutBuilder::from_env()?.build())
    }

    /// Hand back a cloned `watch::Sender` so `lib::run` can flip the
    /// cancellation flag without keeping a reference to `Scout`. The clone
    /// shares state with every receiver subscribed from the underlying
    /// fetch paths.
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

    /// Lazy Slack client, mirroring `github()`. `get_or_try_init` defers the
    /// fallible `from_env` (token read) to the first Slack fetch; tests inject a
    /// wiremock-backed client by pre-setting the `OnceCell` in `build()`.
    async fn slack(&self) -> Result<&SlackClient, ScoutError> {
        self.slack
            .get_or_try_init(|| async {
                SlackClient::from_env(self.http.clone(), self.config.max_retries)
                    .map(|c| c.with_clock(self.clock.clone()).with_rng(self.rng.clone()))
                    .map_err(ScoutError::from)
            })
            .await
    }

    /// Wrap a GitHub command future in the outer `github_timeout`. The inner
    /// `repo_*` handlers chain several per-request HTTP calls (each with its own
    /// 30s `HTTP_TIMEOUT`); a persistent 5xx makes those calls each run their
    /// full retry budget, so without this outer cap a single command could hang
    /// for minutes. Mirrors the `fetch`/`research`/`slack` timeout wrapping in
    /// `query` (issue #185).
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

/// Both notes ride in the body, not in the envelope alone: without `--json` the
/// caller receives `into_markdown()` and nothing else (`src/lib.rs`), so a
/// degradation recorded only in `degraded_reasons` reaches no one. The decode
/// note used to be pushed to the envelope without a body counterpart, which left
/// a default-mode reader holding a possibly garbled page with no sign of it —
/// while the same page through `research` was labelled. The order matches
/// `format_fetched_pages`: what produced the text, then what the text may suffer
/// from.
fn format_fetch_output(result: &FetchResult) -> String {
    let mut output = String::new();
    if result.used_raw_fallback() {
        output.push_str(RAW_FALLBACK_NOTE);
    }
    if result.decode_uncertain() {
        output.push_str(DECODE_UNCERTAIN_NOTE);
    }
    output.push_str(&shift_headings(result.markdown(), 2));

    let truncated = truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES);
    reneutralize_dangling_fence(&truncated).into_owned()
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
