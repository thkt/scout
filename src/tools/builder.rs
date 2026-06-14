//! `ScoutBuilder`: the dependency-injection seam for constructing `Scout`.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use reqwest::redirect::Policy;
use tokio::sync::{OnceCell, watch};
use tracing::warn;

use crate::brave::client::BraveClient;
use crate::clock::{Clock, SystemClock};
use crate::fetch::{DnsResolver, SsrfResolver, TokioDnsResolver};
#[cfg(test)]
use crate::github::GitHubClient;
use crate::rng::{FastrandRng, Rng};
use crate::token_source::{GhCliSource, TokenSource};

use super::{RuntimeConfig, Scout, ScoutError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
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
        // ADR-0012: re-validate connect-time IPs to close the DNS-rebind TOCTOU
        // gap left by the `ssrf_check` pre-flight (which reqwest re-resolves).
        .dns_resolver(Arc::new(SsrfResolver::new(TokioDnsResolver)))
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
