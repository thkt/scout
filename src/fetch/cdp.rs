//! Headless Chrome rendering via CDP, with SSRF-checked navigation.
//!
//! The error type and request gate also compile without `js-rendering`.

#[cfg(feature = "js-rendering")]
mod connection;
mod launch;
#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
mod proxy;

#[cfg(feature = "js-rendering")]
use std::path::Path;
#[cfg(feature = "js-rendering")]
use std::sync::Arc;
#[cfg(feature = "js-rendering")]
use std::time::Duration;

#[cfg(feature = "js-rendering")]
use tokio::sync::watch;
#[cfg(feature = "js-rendering")]
use tokio::task::JoinHandle;
#[cfg(feature = "js-rendering")]
use tokio::time::timeout;
#[cfg(feature = "js-rendering")]
use tracing::warn;

use super::FetchError;
#[cfg(feature = "js-rendering")]
use super::ssrf::{self, ValidatedUrl};
#[cfg(feature = "js-rendering")]
use launch::{parse_ws_url_from_lines, resolve_browser_binary, spawn_chromium_pgroup};

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
#[derive(Debug, thiserror::Error)]
pub(super) enum BrowserError {
    #[error("Chrome/Chromium not found. Install Chrome or set PATH to include chromium")]
    NotFound,
    #[error("browser failed: {0}")]
    ProcessFailed(String),
    /// Reaches the caller as the payload of `FetchError::Timeout`, so it names
    /// the stage that ran out of budget rather than the timeout (src/fetch.rs).
    #[error("browser rendering did not finish")]
    TimedOut,
    #[error("browser cancelled by signal")]
    Cancelled,
}

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
impl From<BrowserError> for FetchError {
    fn from(e: BrowserError) -> Self {
        match e {
            BrowserError::NotFound => Self::BrowserNotFound(e.to_string()),
            BrowserError::ProcessFailed(msg) => Self::BrowserFailed(msg),
            BrowserError::TimedOut | BrowserError::Cancelled => Self::Timeout(e.to_string()),
        }
    }
}

/// Each DevTools URL wait and navigation has a separate 60s cap (DR-0019).
/// Their combined budget can exceed the default 95s outer fetch timeout.
/// Exposed within the crate for the timeout hierarchy check.
#[cfg(feature = "js-rendering")]
pub(crate) const CDP_TIMEOUT: Duration = Duration::from_secs(60);

/// Aborts tasks on Future destruction; browser cleanup has a synchronous owner.
#[cfg(feature = "js-rendering")]
struct AbortOnDrop(JoinHandle<()>);

#[cfg(feature = "js-rendering")]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Resolves the binary per fetch so PATH injection remains isolated.
#[cfg(feature = "js-rendering")]
pub(super) async fn fetch_with_cdp(
    url: &ValidatedUrl,
    resolver: Arc<dyn ssrf::DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<String, BrowserError> {
    let browser_path = resolve_browser_binary()?;
    fetch_with_cdp_with(url, &browser_path, resolver, cancel).await
}

/// Renders via an injected browser path, including the production launch path.
#[cfg(feature = "js-rendering")]
pub(super) async fn fetch_with_cdp_with(
    url: &ValidatedUrl,
    browser_path: &Path,
    resolver: Arc<dyn ssrf::DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<String, BrowserError> {
    // Start the proxy first to supply its port to chromium. Connect-time IP
    // validation closes the DNS-rebinding gap left by subrequest pre-checks.
    let (proxy_port, proxy_task) =
        proxy::spawn_ssrf_proxy(Arc::clone(&resolver), cancel.subscribe())
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("spawn SSRF proxy: {e}")))?;

    // Declared before chromium so group termination precedes proxy abort.
    let _proxy_guard = AbortOnDrop(proxy_task);

    let (mut process, reader) = spawn_chromium_pgroup(browser_path, proxy_port)?;

    let ws_url = match timeout(CDP_TIMEOUT, parse_ws_url_from_lines(reader)).await {
        Ok(Ok(url)) => url,
        Ok(Err(e)) => {
            process.reap().await;
            return Err(e);
        }
        Err(_) => {
            process.reap().await;
            return Err(BrowserError::TimedOut);
        }
    };

    let connect_result = connection::Connection::connect(&ws_url, resolver).await;
    let mut browser = match connect_result {
        Ok(browser) => browser,
        Err(error) => {
            process.reap().await;
            return Err(error);
        }
    };

    // Cancellation reaches graceful close; outer timeout/drain cutoff uses Drop.
    // `wait_for` also sees cancellation sent before subscription.
    let mut rx = cancel.subscribe();
    let result = tokio::select! {
        biased;
        _ = rx.wait_for(|&cancelled| cancelled) => Err(BrowserError::Cancelled),
        r = timeout(CDP_TIMEOUT, browser.navigate(url)) => {
            r.unwrap_or(Err(BrowserError::TimedOut))
        }
    };

    // Close before group termination so chromium can flush profile state.
    match timeout(Duration::from_secs(5), browser.close()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = ?e, "CDP browser.close() returned error"),
        Err(_) => warn!(
            timeout_secs = 5,
            "CDP browser.close() exceeded timeout; falling back to process group cleanup"
        ),
    }
    // Browser helpers and crashpad may outlive browser.close(), especially on
    // macOS where there is no PR_SET_PDEATHSIG.
    process.reap().await;

    result
}

#[cfg(all(test, feature = "js-rendering"))]
mod cdp_integration_tests;
