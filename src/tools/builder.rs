//! `ScoutBuilder`: the dependency-injection seam for constructing `Scout`.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, Proxy};
use tokio::sync::{OnceCell, watch};
use tracing::warn;

use crate::brave::client::BraveClient;
use crate::clock::{Clock, SystemClock};
use crate::fetch::{DnsResolver, EgressMode, SsrfResolver, TokioDnsResolver, detect_egress_mode};
#[cfg(test)]
use crate::github::GitHubClient;
use crate::rng::{FastrandRng, Rng};
#[cfg(test)]
use crate::slack::SlackClient;
use crate::token_source::{GhCliSource, TokenSource};

use super::{RuntimeConfig, Scout, ScoutError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-request timeout for a single HTTP call. `pub(crate)` so the config
/// invariant test can assert the outer `github_timeout` exceeds it (issue #185).
pub(crate) const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

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
    /// Egress mode detected from the environment in `from_env`. Shapes
    /// `fetch_http` (via `build_default_clients`) and is copied into `Scout` so
    /// `fetch` can pass it to `fetch_page`. `for_test` defaults to `Direct`.
    egress: EgressMode,
    cancel: watch::Sender<bool>,
    config: RuntimeConfig,
    /// Pre-initialize `Scout.github` (`OnceCell`) with a test client pointed at
    /// this base URL so `Scout::github()` returns it without ever calling
    /// `from_env_with_source`. `None` (production) preserves lazy init.
    #[cfg(test)]
    github_endpoint: Option<String>,
    /// Pre-init `Scout.slack` (`OnceCell`) with a test client pointed at this
    /// base URL so `Scout::slack()` returns it without reading `SLACK_TOKEN`.
    /// `None` (production) preserves lazy init. Mirrors `github_endpoint`.
    #[cfg(test)]
    slack_endpoint: Option<String>,
}

/// Build the two `reqwest::Client`s shared between production and test paths
/// (redirect-limited + redirect-none). Extracted so `from_env` and `for_test`
/// stay in sync — drift here would change SSRF / timeout posture asymmetrically.
///
/// `egress` shapes only `fetch_http`; the redirect-limited `http` client (Brave,
/// GitHub, Slack) is identical in both modes. In `Direct` mode `fetch_http`
/// carries the ADR-0012 connect-time `SsrfResolver` guard. In `Proxied` mode it
/// instead routes every request through `Proxy::all` and drops that guard: the
/// proxy (not scout) resolves and dials, and the guard would block the
/// loopback/private proxy address itself. `ssrf_check` still rejects literal
/// private/loopback targets per hop, so dropping the guard does not open SSRF to
/// a caller-supplied URL.
fn build_default_clients(egress: &EgressMode) -> Result<(Client, Client), ScoutError> {
    // The User-Agent rides on the client rather than on each request: attaching
    // it per call site is how Slack ended up sending none at all. A call site
    // that needs a different value can still override the default per request.
    let http = Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .redirect(Policy::limited(MAX_REDIRECTS))
        .build()
        .map_err(|e| ScoutError::io_error(format!("HTTP client init failed: {e}")))?;

    let fetch_builder = Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .redirect(Policy::none());
    let fetch_builder = match egress {
        // `Proxy::all`: route every scheme through the forward proxy.
        // https://docs.rs/reqwest/0.13/reqwest/struct.Proxy.html#method.all
        EgressMode::Proxied(url) => fetch_builder.proxy(
            Proxy::all(url).map_err(|e| ScoutError::io_error(format!("proxy init failed: {e}")))?,
        ),
        EgressMode::Direct => {
            fetch_builder.dns_resolver(Arc::new(SsrfResolver::new(TokioDnsResolver)))
        }
    };
    let fetch_http = fetch_builder
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
        // Detect the proxy env once here so `build_default_clients` shapes
        // `fetch_http` to match and `Scout` carries the same mode into `fetch`.
        let egress = detect_egress_mode(&env::vars().collect());
        let (http, fetch_http) = build_default_clients(&egress)?;
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
            egress,
            cancel: watch::channel(false).0,
            config,
            #[cfg(test)]
            github_endpoint: None,
            #[cfg(test)]
            slack_endpoint: None,
        })
    }

    /// Test entry point. Uses `RuntimeConfig::default()` and does NOT read
    /// `SCOUT_*` env vars so a stray `SCOUT_MAX_RETRIES=abc` in the developer
    /// environment cannot panic unrelated tests. `Client::builder().build()`
    /// only fails on TLS init, which would be a real bug — `.expect` is
    /// appropriate.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        let (http, fetch_http) =
            build_default_clients(&EgressMode::Direct).expect("test client init");
        Self {
            http,
            fetch_http,
            brave: None,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            token_source: Arc::new(GhCliSource),
            dns: Arc::new(TokioDnsResolver),
            egress: EgressMode::Direct,
            cancel: watch::channel(false).0,
            config: RuntimeConfig::default(),
            github_endpoint: None,
            slack_endpoint: None,
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

    /// Override the egress mode `build()` installs into `Scout.egress` so a test
    /// can drive the Proxied `fetch` path without setting process-wide proxy env.
    /// Pair with `with_fetch_http` (a proxied, guard-free client, as production
    /// `build_default_clients` builds for `Proxied`) since `for_test`'s default
    /// `fetch_http` is the `Direct` guard-carrying client. Mirrors the other
    /// `with_*` setters.
    #[cfg(test)]
    pub(crate) fn with_egress(mut self, egress: EgressMode) -> Self {
        self.egress = egress;
        self
    }

    /// Replace the fetch HTTP client so a test can point `fetch` at a loopback
    /// wiremock server. Production `fetch_http` carries the connect-time
    /// `SsrfResolver` guard (ADR-0012), which by design blocks loopback; a test
    /// supplies a guard-free client (e.g. via reqwest `.resolve()`) paired with
    /// a public-IP `with_dns` so the pre-flight `ssrf_check` still passes. This
    /// seam is test-only and never weakens the production guard — `from_env`
    /// keeps `build_default_clients`. The SSRF contract stays pinned by the
    /// dedicated T-003 / T-F017 fetch_page tests, not by this client.
    #[cfg(test)]
    pub(crate) fn with_fetch_http(mut self, client: Client) -> Self {
        self.fetch_http = client;
        self
    }

    /// Inject a short outer GitHub-command timeout so a test can force the
    /// `run()`-level guard to trip against a delayed wiremock response without
    /// waiting the production 120s (issue #185). `RuntimeConfig` is `Copy`, so
    /// the field assignment leaves the rest of the config untouched.
    #[cfg(test)]
    pub(crate) fn with_github_timeout(mut self, timeout: Duration) -> Self {
        self.config.github_timeout = timeout;
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

    /// Stores `endpoint`; `build()` uses the current `clock` / `rng` to pre-init
    /// the `slack` `OnceCell`. Composes with `with_clock` / `with_rng` (call
    /// those before `build`). Mirrors `with_github_endpoint`.
    #[cfg(test)]
    pub(crate) fn with_slack_endpoint(mut self, endpoint: &str) -> Self {
        self.slack_endpoint = Some(endpoint.to_owned());
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
        let slack = OnceCell::new();
        #[cfg(test)]
        if let Some(endpoint) = self.slack_endpoint.as_deref() {
            let _ = slack.set(
                SlackClient::with_base_url(self.http.clone(), endpoint)
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
            slack,
            cancel: self.cancel,
            config: self.config,
            clock: self.clock,
            rng: self.rng,
            token_source: self.token_source,
            dns: self.dns,
            egress: self.egress,
        }
    }
}
