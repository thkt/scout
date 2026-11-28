//! `ScoutBuilder`: the dependency-injection seam for constructing `Scout`.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, Proxy};
use tokio::sync::{OnceCell, watch};

#[cfg(test)]
use crate::brave::client::BraveClient;
use crate::clock::{Clock, SystemClock};
use crate::fetch::{DnsResolver, EgressMode, SsrfResolver, TokioDnsResolver, detect_egress_mode};
#[cfg(test)]
use crate::github::GitHubClient;
use crate::rng::{FastrandRng, Rng};
#[cfg(test)]
use crate::secret_source::StaticSecretSource;
use crate::secret_source::{EnvThenKeychain, SecretSource};
#[cfg(test)]
use crate::slack::SlackClient;
#[cfg(test)]
use crate::token_source::StaticTokenSource;
use crate::token_source::{GhCliSource, TokenSource};

use super::{RuntimeConfig, Scout, ScoutError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-request timeout for a single HTTP call. `pub(super)` so the config
/// invariant test can assert the outer `github_timeout` exceeds it.
pub(super) const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

/// Test seam for `Scout`. Production goes through `Scout::new`, which calls
/// `ScoutBuilder::from_env()?.with_not_found_candidates(..).build()`; tests use
/// the `with_*` setters to
/// inject `Clock` / `DnsResolver` doubles or to point `Brave` / `GitHub` /
/// `Slack` at wiremock endpoints without reaching into private fields.
/// `build` is sync + infallible so fallibility stays in `from_env`.
pub(super) struct ScoutBuilder {
    http: Client,
    fetch_http: Client,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    token_source: Arc<dyn TokenSource>,
    secret_source: Arc<dyn SecretSource>,
    dns: Arc<dyn DnsResolver>,
    /// Egress mode detected from the environment in `from_env`. Shapes
    /// `fetch_http` (via `build_default_clients`) and is copied into `Scout` so
    /// `fetch` can pass it to `fetch_page`. `for_test` defaults to `Direct`.
    egress: EgressMode,
    cancel: watch::Sender<bool>,
    config: RuntimeConfig,
    not_found_candidates: bool,
    /// Pre-initialize `Scout.github` (`OnceCell`) with a test client pointed at
    /// this base URL so `Scout::github()` returns it without ever calling
    /// `from_env_with_source`. `None` (production) preserves lazy init.
    #[cfg(test)]
    github_endpoint: Option<String>,
    /// Pre-init `Scout.brave` (`OnceCell`) with a test client pointed at this
    /// base URL so `Scout::brave()` returns it without resolving the key.
    /// `None` (production) preserves lazy init. Mirrors `github_endpoint`.
    #[cfg(test)]
    brave_endpoint: Option<String>,
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
    // Set a shared User-Agent so a new API call cannot silently omit it.
    let http = Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .redirect(Policy::limited(MAX_REDIRECTS))
        .build()
        .map_err(|e| ScoutError::io_error(format!("HTTP client init failed: {e}")))?;

    // Automatic redirects would bypass download's per-hop SSRF validation
    // (ADR-0001); the client must leave redirect handling to scout.
    let fetch_builder = Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .redirect(Policy::none());
    let fetch_builder = match egress {
        EgressMode::Proxied(url) => fetch_builder.proxy(
            Proxy::all(url).map_err(|e| ScoutError::io_error(format!("proxy init failed: {e}")))?,
        ),
        // `no_proxy` turns off reqwest's own proxy discovery. With it on,
        // reqwest could dial a proxy from a source `detect_egress_mode` does
        // not read, and the guard below would validate the proxy address in
        // place of the target.
        EgressMode::Direct => fetch_builder
            .no_proxy()
            .dns_resolver(Arc::new(SsrfResolver::new(TokioDnsResolver))),
    };
    let fetch_http = fetch_builder
        .build()
        .map_err(|e| ScoutError::io_error(format!("HTTP client init failed: {e}")))?;
    Ok((http, fetch_http))
}

impl ScoutBuilder {
    /// Read `SCOUT_*` env vars and construct the two `reqwest::Client`s. The
    /// Brave key, Slack token, and GitHub token stay unresolved until a command
    /// needs them.
    pub(super) fn from_env() -> Result<Self, ScoutError> {
        let config = RuntimeConfig::from_env()?;
        // `env::vars()` panics on a non-UTF-8 entry; a proxy URL is UTF-8, so
        // such entries cannot select a proxy and are skipped.
        let utf8_env = env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        let egress = detect_egress_mode(&utf8_env);
        let (http, fetch_http) = build_default_clients(&egress)?;
        Ok(Self {
            http,
            fetch_http,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            token_source: Arc::new(GhCliSource),
            secret_source: Arc::new(EnvThenKeychain),
            dns: Arc::new(TokioDnsResolver),
            egress,
            cancel: watch::channel(false).0,
            config,
            not_found_candidates: true,
            #[cfg(test)]
            github_endpoint: None,
            #[cfg(test)]
            brave_endpoint: None,
            #[cfg(test)]
            slack_endpoint: None,
        })
    }

    /// Test entry point. Uses `RuntimeConfig::for_test()` and does NOT read
    /// `SCOUT_*` env vars so a stray `SCOUT_MAX_RETRIES=abc` in the developer
    /// environment cannot panic unrelated tests. `Client::builder().build()`
    /// only fails on TLS init, which would be a real bug — `.expect` is
    /// appropriate.
    ///
    /// `token_source` is not the production `GhCliSource`: a test reaching
    /// `Scout::github()` would spawn the real `gh auth token`, making the
    /// branch taken depend on the machine running the suite (ADR-0018). A test
    /// that wants a token calls `with_token_source`.
    #[cfg(test)]
    pub(super) fn for_test() -> Self {
        let (http, fetch_http) =
            build_default_clients(&EgressMode::Direct).expect("test client init");
        Self {
            http,
            fetch_http,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            token_source: Arc::new(StaticTokenSource(None)),
            secret_source: Arc::new(StaticSecretSource(None)),
            dns: Arc::new(TokioDnsResolver),
            egress: EgressMode::Direct,
            cancel: watch::channel(false).0,
            config: RuntimeConfig::for_test(),
            not_found_candidates: true,
            github_endpoint: None,
            brave_endpoint: None,
            slack_endpoint: None,
        }
    }

    /// `for_test` never reads the real env var or Keychain; a test that wants a
    /// Brave key or Slack token resolved through `Scout::brave()` or
    /// `Scout::slack()` injects its own source here.
    #[cfg(test)]
    pub(super) fn with_secret_source(mut self, secret_source: Arc<dyn SecretSource>) -> Self {
        self.secret_source = secret_source;
        self
    }

    #[cfg(test)]
    pub(super) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    #[cfg(test)]
    pub(super) fn with_token_source(mut self, token_source: Arc<dyn TokenSource>) -> Self {
        self.token_source = token_source;
        self
    }

    #[cfg(test)]
    pub(super) fn with_dns(mut self, dns: Arc<dyn DnsResolver>) -> Self {
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
    pub(super) fn with_egress(mut self, egress: EgressMode) -> Self {
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
    /// dedicated T-F017 / T-F072 fetch_page tests, not by this client.
    #[cfg(test)]
    pub(super) fn with_fetch_http(mut self, client: Client) -> Self {
        self.fetch_http = client;
        self
    }

    /// Whether a not-found `repo-read` looks up path candidates. Only the JSON
    /// envelope renders them, so the Markdown path turns the lookup off.
    pub(super) fn with_not_found_candidates(mut self, enabled: bool) -> Self {
        self.not_found_candidates = enabled;
        self
    }

    /// Inject a short outer GitHub-command timeout so a test can force the
    /// `run()`-level guard to trip against a delayed wiremock response without
    /// waiting the production default. `RuntimeConfig` is `Copy`, so the field
    /// assignment leaves the rest of the config untouched.
    #[cfg(test)]
    pub(super) fn with_github_timeout(mut self, timeout: Duration) -> Self {
        self.config.github_timeout = timeout;
        self
    }

    /// The `search` counterpart of `with_github_timeout`: a test forces the
    /// `run()`-level guard to trip against a delayed Brave response.
    #[cfg(test)]
    pub(super) fn with_search_timeout(mut self, timeout: Duration) -> Self {
        self.config.search_timeout = timeout;
        self
    }

    /// The `fetch_slack` counterpart of `with_github_timeout`: a test forces the
    /// `tokio::time::timeout` around `fetch_message` to trip against a delayed
    /// wiremock response instead of waiting the production 60s.
    #[cfg(test)]
    pub(super) fn with_slack_timeout(mut self, timeout: Duration) -> Self {
        self.config.slack_timeout = timeout;
        self
    }

    /// Re-uses the builder's `http` so wiremock servers share the test client
    /// (avoids spawning a second `reqwest` connection pool per test).
    #[cfg(test)]
    pub(super) fn with_brave_endpoint(mut self, endpoint: &str) -> Self {
        self.brave_endpoint = Some(endpoint.to_owned());
        self
    }

    /// `build()` pre-inits `Scout.github`'s `OnceCell` with a client pointed
    /// here, so `Scout::github()` never runs `from_env_with_source`.
    #[cfg(test)]
    pub(super) fn with_github_endpoint(mut self, endpoint: &str) -> Self {
        self.github_endpoint = Some(endpoint.to_owned());
        self
    }

    /// Mirrors `with_github_endpoint` for `Scout.slack`, so `Scout::slack()`
    /// never reads `SLACK_TOKEN`.
    #[cfg(test)]
    pub(super) fn with_slack_endpoint(mut self, endpoint: &str) -> Self {
        self.slack_endpoint = Some(endpoint.to_owned());
        self
    }

    pub(super) fn build(self) -> Scout {
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
        let brave = OnceCell::new();
        #[cfg(test)]
        if let Some(endpoint) = self.brave_endpoint.as_deref() {
            let _ = brave.set(
                BraveClient::with_base_url(self.http.clone(), endpoint)
                    .with_clock(self.clock.clone())
                    .with_rng(self.rng.clone()),
            );
        }
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
            secret_source: self.secret_source,
            dns: self.dns,
            egress: self.egress,
            not_found_candidates: self.not_found_candidates,
        }
    }
}
