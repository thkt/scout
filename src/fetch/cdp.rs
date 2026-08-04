//! Headless Chrome rendering via the Chrome DevTools Protocol (CDP).
//!
//! Extracted from fetch.rs: browser discovery, launch, SSRF-checked CDP
//! navigation. Most items are gated behind the `js-rendering` feature; the
//! error type and request gate compile in both modes (dead in default).

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

/// Aborts the wrapped task when dropped, so the SSRF proxy is torn down on every
/// exit path of `fetch_with_cdp` (early `return`s included). Awaiting the task to
/// observe a panic is not possible from `Drop`; an abort is sufficient because the
/// proxy holds no state that must be flushed.
#[cfg(feature = "js-rendering")]
struct AbortOnDrop(JoinHandle<()>);

#[cfg(feature = "js-rendering")]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Resolve the browser binary, then render via CDP. Binary discovery runs per
/// call (no process-global cache) so the path stays injectable for tests
/// (issue #227, the #191 DI seam pattern).
#[cfg(feature = "js-rendering")]
pub(super) async fn fetch_with_cdp(
    url: &ValidatedUrl,
    resolver: Arc<dyn ssrf::DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<String, BrowserError> {
    let browser_path = resolve_browser_binary()?;
    fetch_with_cdp_with(url, &browser_path, resolver, cancel).await
}

/// Render `url` with headless chromium at `browser_path` via CDP.
///
/// Seam for testing: the browser binary path is injected, so a test can drive
/// the launch path with a bogus path (asserting `ProcessFailed`) without a real
/// Chrome on the host. `fetch_with_cdp` is the production caller that resolves
/// the path first.
#[cfg(feature = "js-rendering")]
pub(super) async fn fetch_with_cdp_with(
    url: &ValidatedUrl,
    browser_path: &Path,
    resolver: Arc<dyn ssrf::DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<String, BrowserError> {
    use chromiumoxide::Browser;
    use futures::StreamExt;

    // Launch the loopback SSRF proxy before chromium so its port can be wired
    // into the proxy flags. chromium routes every TCP egress through it and the
    // proxy re-validates connect-time IPs, closing the DNS-rebind gap that the
    // resolve-time `check_browser_request` pre-flight cannot reach (issue #201).
    let (proxy_port, proxy_task) =
        proxy::spawn_ssrf_proxy(Arc::clone(&resolver), cancel.subscribe())
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("spawn SSRF proxy: {e}")))?;

    // Abort the proxy on every exit path (early `return`s included) via RAII.
    // Declared before the chromium locals so it drops last — after `reap_pgroup`
    // below — so no late subrequest can reach the proxy once chromium is gone and
    // its validation context with it. On the SIGINT path the `cancel` flag already
    // ended the accept loop, so the abort is a no-op there and a stop otherwise.
    let _proxy_guard = AbortOnDrop(proxy_task);

    // `_profile_dir` guards the chromium `--user-data-dir`. Held until this
    // function returns — i.e. after every `reap_pgroup` path below — so its
    // `Drop` removes the dir only once chromium has exited (issue #198).
    let (mut child, pgid, reader, _profile_dir) = spawn_chromium_pgroup(browser_path, proxy_port)?;

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

/// Open a page, navigate to `url`, and return the rendered HTML.
///
/// Every subrequest the page issues is intercepted through `Fetch.RequestPaused`
/// and run past `resolver` before it is allowed to continue, so a page cannot
/// reach an internal address by way of a resource it loads. Borrows the browser
/// so the caller keeps ownership and can still tear it down after a timeout.
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
