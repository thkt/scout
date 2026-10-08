//! Headless Chrome rendering via CDP, with SSRF-checked navigation.
//!
//! The error type and request gate also compile without `js-rendering`.

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
use chromiumoxide::error::CdpError;
#[cfg(feature = "js-rendering")]
use tokio::sync::watch;
#[cfg(feature = "js-rendering")]
use tokio::task::JoinHandle;
#[cfg(feature = "js-rendering")]
use tokio::time::timeout;
#[cfg(feature = "js-rendering")]
use tracing::{debug, error, warn};

use super::FetchError;
#[cfg(feature = "js-rendering")]
use super::ssrf::{self, RedactedLogUrl, ValidatedUrl};
#[cfg(feature = "js-rendering")]
use launch::{
    check_browser_request, parse_ws_url_from_lines, resolve_browser_binary, spawn_chromium_pgroup,
};

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

/// Forwards interceptor failures to navigation instead of leaving a paused
/// subrequest waiting until the CDP timeout.
#[cfg(feature = "js-rendering")]
#[derive(Debug, thiserror::Error)]
enum CdpInterceptError {
    #[error("CDP intercept execute failed: {0}")]
    Execute(CdpError),
}

#[cfg(feature = "js-rendering")]
impl From<CdpInterceptError> for BrowserError {
    fn from(e: CdpInterceptError) -> Self {
        BrowserError::ProcessFailed(e.to_string())
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
    use chromiumoxide::Browser;
    use futures::StreamExt;

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

    let connect_result = Browser::connect(&ws_url)
        .await
        .map_err(|e| BrowserError::ProcessFailed(format!("browser connect: {e}")));
    let (mut browser, mut handler) = match connect_result {
        Ok(pair) => pair,
        Err(e) => {
            process.reap().await;
            return Err(e);
        }
    };

    let mut handler_task = AbortOnDrop(tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if let Err(e) = h {
                debug!(error = ?e, "CDP handler stream ended with error");
                break;
            }
        }
    }));

    // Cancellation reaches graceful close; outer timeout/drain cutoff uses Drop.
    // `wait_for` also sees cancellation sent before subscription.
    let mut rx = cancel.subscribe();
    let result = tokio::select! {
        biased;
        _ = rx.wait_for(|&cancelled| cancelled) => Err(BrowserError::Cancelled),
        r = timeout(CDP_TIMEOUT, cdp_navigate(&mut browser, url, resolver)) => {
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
    handler_task.0.abort();
    match (&mut handler_task.0).await {
        Ok(()) => {}
        Err(e) if e.is_cancelled() => {}
        Err(e) => error!(error = ?e, "CDP handler task panicked"),
    }

    // Browser helpers and crashpad may outlive browser.close(), especially on
    // macOS where there is no PR_SET_PDEATHSIG.
    process.reap().await;

    result
}

/// Intercepts subrequests for SSRF validation before navigation.
/// Borrows the browser so the caller retains timeout cleanup ownership.
#[cfg(feature = "js-rendering")]
async fn cdp_navigate(
    browser: &mut chromiumoxide::Browser,
    url: &ValidatedUrl,
    resolver: Arc<dyn ssrf::DnsResolver>,
) -> Result<String, BrowserError> {
    use chromiumoxide::cdp::browser_protocol::fetch::{
        ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams,
    };
    use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
    use futures::StreamExt;
    use tokio::sync::oneshot;

    let page = browser
        .new_page("about:blank")
        .await
        .map_err(|e| BrowserError::ProcessFailed(format!("new page: {e}")))?;

    page.execute(EnableParams::default())
        .await
        .map_err(|e| BrowserError::ProcessFailed(format!("fetch enable: {e}")))?;

    let mut events = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|e| BrowserError::ProcessFailed(format!("event listener: {e}")))?;

    let (intercept_err_tx, intercept_err_rx) = oneshot::channel::<CdpInterceptError>();
    let intercept_page = page.clone();
    let mut interceptor = AbortOnDrop(tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let req_url = &event.request.url;
            let allowed = check_browser_request(req_url, resolver.as_ref()).await;
            let exec_result: Result<(), CdpError> = if allowed {
                intercept_page
                    .execute(ContinueRequestParams::new(event.request_id.clone()))
                    .await
                    .map(|_| ())
            } else {
                warn!(blocked_url = %RedactedLogUrl(req_url), "SSRF: blocked browser subrequest");
                intercept_page
                    .execute(FailRequestParams::new(
                        event.request_id.clone(),
                        ErrorReason::BlockedByClient,
                    ))
                    .await
                    .map(|_| ())
            };
            if let Err(e) = exec_result {
                // Receiver dropped (= navigation already completed) is harmless; ignore.
                let _ = intercept_err_tx.send(CdpInterceptError::Execute(e));
                break;
            }
        }
    }));

    let navigation = async {
        page.goto(url.as_str())
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("navigation: {e}")))?;
        page.content()
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("content: {e}")))
    };
    tokio::pin!(navigation);
    let mut intercept_err_rx = intercept_err_rx;
    // An already-sent intercept error must win over completed navigation.
    let result = tokio::select! {
        biased;
        intercept_err = &mut intercept_err_rx => Err(intercept_err
            .map(BrowserError::from)
            .unwrap_or_else(|_| BrowserError::ProcessFailed(
                "CDP intercept task dropped without status".into(),
            ))),
        nav_result = &mut navigation => nav_result,
    };

    interceptor.0.abort();
    match (&mut interceptor.0).await {
        Ok(()) => {}
        Err(e) if e.is_cancelled() => {}
        Err(e) => error!(error = ?e, "CDP intercept task panicked"),
    }
    result
}

#[cfg(all(test, feature = "js-rendering"))]
mod cdp_integration_tests;
