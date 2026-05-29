//! Headless Chrome rendering via the Chrome DevTools Protocol (CDP).
//!
//! Extracted from fetch.rs: browser discovery, launch, SSRF-checked CDP
//! navigation. Most items are gated behind the `js-rendering` feature; the
//! error type and request gate compile in both modes (dead in default).

mod launch;

#[cfg(feature = "js-rendering")]
use std::sync::Arc;
#[cfg(feature = "js-rendering")]
use std::time::Duration;

#[cfg(feature = "js-rendering")]
use chromiumoxide::error::CdpError;
#[cfg(feature = "js-rendering")]
use tokio::sync::watch;
#[cfg(feature = "js-rendering")]
use tokio::time::timeout;
#[cfg(feature = "js-rendering")]
use tracing::{debug, error, warn};

use super::FetchError;
#[cfg(feature = "js-rendering")]
use super::ssrf::{self, RedactedLogUrl, ValidatedUrl};
#[cfg(feature = "js-rendering")]
use launch::{
    check_browser_request, parse_ws_url_from_lines, reap_pgroup, resolve_browser_binary,
    spawn_chromium_pgroup,
};

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
#[derive(Debug, thiserror::Error)]
pub(super) enum BrowserError {
    #[error("Chrome/Chromium not found. Install Chrome or set PATH to include chromium")]
    NotFound,
    #[error("browser failed: {0}")]
    ProcessFailed(String),
    #[error("browser rendering timed out")]
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

/// Reasons the CDP request-pause interceptor aborts.
///
/// Surfaces a failure inside the spawned interceptor task to the navigation
/// task via a `oneshot` channel — without this, an `execute()` failure on the
/// Continue/Fail command would be silently dropped and the subrequest would
/// hang until the CDP timeout fires.
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

#[cfg(feature = "js-rendering")]
const CDP_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(feature = "js-rendering")]
pub(super) async fn fetch_with_cdp(
    url: &ValidatedUrl,
    resolver: Arc<dyn ssrf::DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<String, BrowserError> {
    use chromiumoxide::Browser;
    use futures::StreamExt;

    let browser_path = resolve_browser_binary()?;

    let (mut child, pgid, reader) = spawn_chromium_pgroup(&browser_path)?;

    let ws_url = match timeout(CDP_TIMEOUT, parse_ws_url_from_lines(reader)).await {
        Ok(Ok(url)) => url,
        Ok(Err(e)) => {
            reap_pgroup(pgid, &mut child).await;
            return Err(e);
        }
        Err(_) => {
            reap_pgroup(pgid, &mut child).await;
            return Err(BrowserError::TimedOut);
        }
    };

    let connect_result = Browser::connect(&ws_url)
        .await
        .map_err(|e| BrowserError::ProcessFailed(format!("browser connect: {e}")));
    let (mut browser, mut handler) = match connect_result {
        Ok(pair) => pair,
        Err(e) => {
            reap_pgroup(pgid, &mut child).await;
            return Err(e);
        }
    };

    let handler_task = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if let Err(e) = h {
                debug!(error = ?e, "CDP handler stream ended with error");
                break;
            }
        }
    });

    // Race navigate against cancellation so SIGINT/SIGTERM still reaches the
    // graceful close path below. Without this branch the future would be
    // dropped by the outer select! in lib::run and chromium subprocesses
    // would orphan to ppid=1.
    //
    // `wait_for` is sticky: if the flag was already `true` at subscribe time
    // (e.g. SIGINT arrived while reqwest was still downloading the initial
    // HTML), the closure runs against the current value and returns
    // immediately. `Notify` would have silently dropped that wakeup.
    let mut rx = cancel.subscribe();
    let result = tokio::select! {
        biased;
        _ = rx.wait_for(|&cancelled| cancelled) => Err(BrowserError::Cancelled),
        r = timeout(CDP_TIMEOUT, cdp_navigate(&mut browser, url, resolver)) => {
            r.unwrap_or(Err(BrowserError::TimedOut))
        }
    };

    // Drive the CDP graceful close first so chromium runs its own teardown
    // sequence (flush IPC, write profile state, etc). Surface both arms:
    // an `Err` from close() means CDP refused the teardown, an `Elapsed`
    // means chromium hung past the budget; either way `reap_pgroup` below
    // will SIGTERM/SIGKILL but operators need to see why the graceful path
    // failed (issue #152).
    match timeout(Duration::from_secs(5), browser.close()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = ?e, "CDP browser.close() returned error"),
        Err(_) => warn!(
            timeout_secs = 5,
            "CDP browser.close() exceeded timeout; falling back to reap_pgroup"
        ),
    }
    handler_task.abort();
    match handler_task.await {
        Ok(()) => {}
        Err(e) if e.is_cancelled() => {}
        Err(e) => error!(error = ?e, "CDP handler task panicked"),
    }

    // macOS has no PR_SET_PDEATHSIG, so Helper Renderer / GPU / Network service
    // would otherwise reparent to ppid=1 after browser.close() returns.
    // chrome_crashpad_handler may also outlive its parent by design.
    reap_pgroup(pgid, &mut child).await;

    result
}

/// Spawn chromium in a new process group and return (Child, pgid, stderr reader).
///
/// Synchronous so the caller captures `pgid` before any timeout can drop the
/// future and orphan the group. The pgid equals the chromium child's pid (the
/// call uses `process_group(0)`, which means "make the child the leader of a
/// new group whose id is its pid"). scout retains the `Child` so the kernel
/// can reap the parent after we kill the group.
///
/// chromiumoxide 0.9 hides `tokio::process::Command` behind a private wrapper,
/// so `BrowserConfig::launch` cannot set `process_group(0)`. We self-spawn and
/// hand the resulting WebSocket URL to `Browser::connect` instead.
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
    let interceptor = tokio::spawn(async move {
        let mut intercept_err_tx = Some(intercept_err_tx);
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
            if let Err(e) = exec_result
                && let Some(tx) = intercept_err_tx.take()
            {
                // Receiver dropped (= navigation already completed) is harmless; ignore.
                let _ = tx.send(CdpInterceptError::Execute(e));
                break;
            }
        }
    });

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
    // `biased;` so a fast-completing navigation cannot race past an
    // already-sent intercept error.
    let result = tokio::select! {
        biased;
        intercept_err = &mut intercept_err_rx => Err(intercept_err
            .map(BrowserError::from)
            .unwrap_or_else(|_| BrowserError::ProcessFailed(
                "CDP intercept task dropped without status".into(),
            ))),
        nav_result = &mut navigation => nav_result,
    };

    interceptor.abort();
    match interceptor.await {
        Ok(()) => {}
        Err(e) if e.is_cancelled() => {}
        Err(e) => error!(error = ?e, "CDP intercept task panicked"),
    }
    result
}

#[cfg(all(test, feature = "js-rendering"))]
mod cdp_integration_tests;
