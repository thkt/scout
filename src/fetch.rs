//! Web page fetching with SSRF defense-in-depth.
//!
//! URL validation → DNS pre-check → download → post-redirect recheck → content extraction.

pub(crate) mod converter;
mod extractor;
mod ssrf;

pub(crate) use ssrf::{DnsResolver, RedactedLogUrl, TokioDnsResolver};
#[cfg(test)]
pub(crate) use ssrf::{FailingDnsResolver, StaticDnsResolver};
use ssrf::{ValidatedUrl, ssrf_check};

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
#[cfg(feature = "js-rendering")]
use std::sync::OnceLock;
#[cfg(feature = "js-rendering")]
use std::time::Duration;

use tokio::sync::watch;
#[cfg(feature = "js-rendering")]
use tokio::time::timeout;

use converter::{FetchResult, to_fetch_result};
use extractor::{extract_article, extract_raw};
use reqwest::Client;
use reqwest::header::LOCATION;

#[cfg(feature = "js-rendering")]
use chromiumoxide::error::CdpError;
#[cfg(feature = "js-rendering")]
use nix::unistd::Pid;
#[cfg(feature = "js-rendering")]
use tokio::io::{AsyncBufRead, BufReader};
#[cfg(feature = "js-rendering")]
use tokio::process::{Child as TokioChild, ChildStderr, Command as TokioCommand};
#[cfg(feature = "js-rendering")]
use tokio::time::sleep;
#[cfg(feature = "js-rendering")]
use tracing::error;
use tracing::{debug, info, warn};

/// Options for [`fetch_page`] that control rendering and output.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FetchOptions {
    /// Force JS rendering via CDP (skip auto-detection). Requires `js-rendering` feature.
    pub js: bool,
    /// Skip Readability extraction; return full HTML converted to Markdown.
    pub raw: bool,
}

const MAX_RESPONSE_BYTES: usize = 10_000_000;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error("invalid URL: must be HTTP(S)")]
    InvalidScheme,

    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("blocked: internal/private host not allowed")]
    InternalHost,

    #[error("fetch failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("too many redirects (>{0})")]
    TooManyRedirects(usize),

    #[error("redirect without Location header")]
    RedirectMissingLocation,

    #[error("DNS resolution failed: {0}")]
    DnsResolution(String),

    #[error("fetch failed: status {0}")]
    Status(u16),

    #[error("unsupported content type: {0} (expected text/HTML)")]
    UnsupportedContentType(String),

    #[error("response too large (>{} bytes)", MAX_RESPONSE_BYTES)]
    TooLarge,

    #[error("fetch timed out: {0}")]
    Timeout(String),

    #[error("browser not available: {0}")]
    BrowserNotFound(String),

    #[error("browser rendering failed: {0}")]
    BrowserFailed(String),
}

/// ~1 sentence; pages below this almost always need JS rendering.
const EXTRACT_TEXT_THRESHOLD: usize = 50;

pub(crate) async fn fetch_page(
    client: &Client,
    url: &str,
    opts: FetchOptions,
    resolver: Arc<dyn DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<FetchResult, FetchError> {
    // `cancel` is only consumed on the CDP path. Silence the unused-arg
    // warning in builds without `js-rendering` instead of duplicating the
    // function signature behind cfg.
    #[cfg(not(feature = "js-rendering"))]
    let _ = cancel;

    #[cfg(not(feature = "js-rendering"))]
    if opts.js {
        return Err(FetchError::BrowserNotFound(
            "js-rendering feature required — rebuild with `--features js-rendering`".into(),
        ));
    }

    // SECURITY: Local CLI only. TOCTOU gap between DNS check and reqwest connect
    // is acceptable here; a network service would need a custom resolver that
    // enforces the allowlist at connect time.
    //
    // The returned `ValidatedUrl` is the only constructor for SSRF-checked URLs;
    // `download` requires `&ValidatedUrl` so the redirect loop cannot bypass it.
    let validated = ssrf_check(url, resolver.as_ref()).await?;

    #[cfg(feature = "js-rendering")]
    let (final_url, mut html) =
        download(client, &validated, MAX_REDIRECTS, resolver.as_ref()).await?;
    #[cfg(not(feature = "js-rendering"))]
    let (final_url, html) = download(client, &validated, MAX_REDIRECTS, resolver.as_ref()).await?;

    let need_js = if opts.js {
        info!("--js flag set, requesting JS rendering");
        true
    } else if is_js_dependent(&html) {
        warn!("JS-dependent page detected, trying JS rendering fallback");
        true
    } else {
        false
    };

    if need_js {
        #[cfg(feature = "js-rendering")]
        {
            match fetch_with_cdp(&final_url, Arc::clone(&resolver), cancel).await {
                Ok(js_html) => {
                    debug!("JS rendering succeeded via CDP");
                    html = js_html;
                }
                Err(e) if opts.js => {
                    return Err(FetchError::from(e));
                }
                Err(e) => {
                    warn!(error = %e, "JS rendering failed, using original HTML");
                }
            }
        }
        #[cfg(not(feature = "js-rendering"))]
        {
            warn!(
                "JS rendering unavailable (js-rendering feature not enabled), using original HTML"
            );
        }
    }

    let article = if opts.raw {
        extract_raw(&html)
    } else {
        extract_article(&html, Some(final_url.as_str()))
    };

    let need_thin_fallback = !opts.raw && !need_js && is_thin_extract(&article);
    #[cfg(feature = "js-rendering")]
    let article = if need_thin_fallback {
        warn!(url = %RedactedLogUrl(final_url.as_str()), "extraction yielded too little content, trying JS rendering fallback");
        match fetch_with_cdp(&final_url, Arc::clone(&resolver), cancel).await {
            Ok(js_html) => {
                let re_extracted = extract_article(&js_html, Some(final_url.as_str()));
                if is_thin_extract(&re_extracted) {
                    debug!(url = %RedactedLogUrl(final_url.as_str()), "JS re-extraction still thin, returning best-effort result");
                } else {
                    debug!(url = %RedactedLogUrl(final_url.as_str()), "JS rendering fallback succeeded (post-extraction)");
                }
                re_extracted
            }
            Err(e) => {
                warn!(url = %RedactedLogUrl(final_url.as_str()), error = %e, "JS rendering fallback failed, using original extraction");
                article
            }
        }
    } else {
        article
    };
    #[cfg(not(feature = "js-rendering"))]
    if need_thin_fallback {
        warn!(url = %RedactedLogUrl(final_url.as_str()), "extraction yielded too little content but JS rendering unavailable");
    }

    debug!(url = %RedactedLogUrl(final_url.as_str()), bytes = html.len(), "page fetched");
    Ok(to_fetch_result(&article, final_url.as_str().to_owned()))
}

/// Raw fallback is always thin because shell text (nav, footer) inflates
/// the count but the article body is missing.
fn is_thin_extract(article: &extractor::ExtractedArticle) -> bool {
    article.used_raw_fallback
        || visible_text_len(&article.content_html, EXTRACT_TEXT_THRESHOLD) < EXTRACT_TEXT_THRESHOLD
}

fn visible_text_len(html: &str, limit: usize) -> usize {
    let mut count = 0usize;
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag || ch.is_whitespace() => {}
            _ => {
                count += 1;
                if count >= limit {
                    return count;
                }
            }
        }
    }
    count
}

const BODY_TEXT_THRESHOLD: usize = 100;

const SPA_ROOT_IDS: &[&str] = &[
    r#"id="root""#,
    r#"id="app""#,
    r#"id="__next""#,
    r#"id="__nuxt""#,
];

fn is_js_dependent(html: &str) -> bool {
    if !has_thin_body(html) {
        return false;
    }
    html.contains("<script") || SPA_ROOT_IDS.iter().any(|p| html.contains(p))
}

fn has_thin_body(html: &str) -> bool {
    let lower = html.as_bytes();
    let body_start = lower
        .windows(5)
        .position(|w| w.eq_ignore_ascii_case(b"<body"));
    let body = if let Some(start) = body_start {
        let after_tag = html[start..]
            .find('>')
            .map(|i| start + i + 1)
            .unwrap_or(start);
        let body_end = lower[after_tag..]
            .windows(7)
            .position(|w| w.eq_ignore_ascii_case(b"</body>"))
            .map(|i| after_tag + i)
            .unwrap_or(html.len());
        &html[after_tag..body_end]
    } else {
        html
    };

    let mut visible_bytes = 0usize;
    let mut in_tag = false;
    let mut skip_text = false;
    let mut tag_buf = [0u8; 16];
    let mut tag_len = 0usize;
    let mut reading_name = false;
    let mut in_whitespace = true;

    for ch in body.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_len = 0;
                reading_name = true;
            }
            '>' if in_tag => {
                in_tag = false;
                reading_name = false;
                let name = &tag_buf[..tag_len];
                if name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"style") {
                    skip_text = true;
                } else if name.eq_ignore_ascii_case(b"/script")
                    || name.eq_ignore_ascii_case(b"/style")
                {
                    skip_text = false;
                }
            }
            _ if in_tag => {
                if reading_name {
                    if ch.is_ascii_alphanumeric() || ch == '/' {
                        if tag_len < tag_buf.len() {
                            tag_buf[tag_len] = ch as u8;
                            tag_len += 1;
                        }
                    } else {
                        reading_name = false;
                    }
                }
            }
            _ if skip_text => {}
            _ if ch.is_whitespace() => {
                if !in_whitespace && visible_bytes > 0 {
                    visible_bytes += 1;
                    in_whitespace = true;
                }
            }
            _ => {
                visible_bytes += ch.len_utf8();
                in_whitespace = false;
                if visible_bytes >= BODY_TEXT_THRESHOLD {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
#[derive(Debug, thiserror::Error)]
enum BrowserError {
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
fn resolve_browser_binary() -> Result<PathBuf, BrowserError> {
    static CACHE: OnceLock<Result<PathBuf, String>> = OnceLock::new();

    let cached = CACHE.get_or_init(|| {
        let path_commands: &[&str] = if cfg!(target_os = "macos") {
            &["chromium"]
        } else {
            &[
                "google-chrome-stable",
                "google-chrome",
                "chromium-browser",
                "chromium",
            ]
        };
        let known_paths: &[&Path] = if cfg!(target_os = "macos") {
            &[
                Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
                Path::new("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            ]
        } else {
            &[]
        };
        resolve_browser_binary_from(path_commands, known_paths).map_err(|e| e.to_string())
    });

    cached.clone().map_err(|_| BrowserError::NotFound)
}

/// See spec.md Chrome Launch Flags table for rationale.
#[cfg(feature = "js-rendering")]
fn build_launch_args() -> Vec<&'static str> {
    vec![
        "--headless=new",
        "--disable-webrtc",
        "--disable-background-networking",
        "--disable-features=DnsOverHttps",
        "--disable-domain-reliability",
        "--no-pings",
        "--disable-extensions",
        "--no-first-run",
        "--disable-default-apps",
    ]
}

/// SSRF check for a browser-initiated subrequest URL (CDP `Fetch.RequestPaused`).
///
/// Scheme handling rationale:
/// - `http`/`https`: passed directly to `ssrf::ssrf_check`
/// - `ws`/`wss`: WebSocket can reach internal services; rewritten to http(s) for SSRF allowlist check
/// - `data:`/`about:`/`chrome:`/`blob:`: synthetic browser schemes with no external egress, allowed without SSRF check
/// - Unrecognized scheme: blocked (warn + return false) because the scheme cannot be classified
///
/// See ADR-0001 for the SSRF defense architecture.
#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
pub(crate) async fn check_browser_request(url: &str, resolver: &dyn ssrf::DnsResolver) -> bool {
    let check_url = if url.starts_with("http://") || url.starts_with("https://") {
        Cow::Borrowed(url)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        Cow::Owned(format!("http://{rest}"))
    } else if let Some(rest) = url.strip_prefix("wss://") {
        Cow::Owned(format!("https://{rest}"))
    } else if url.starts_with("data:")
        || url.starts_with("about:")
        || url.starts_with("chrome:")
        || url.starts_with("blob:")
    {
        return true;
    } else {
        warn!(url = %RedactedLogUrl(url), "SSRF: blocked browser subrequest with unrecognized scheme");
        return false;
    };
    ssrf::ssrf_check(&check_url, resolver).await.is_ok()
}

#[cfg(feature = "js-rendering")]
const CDP_TIMEOUT: Duration = Duration::from_secs(60);

/// Grace period between SIGTERM and SIGKILL when reaping the chromium pgroup.
/// 50 ms is enough for chromium subprocess (Helper Renderer, GPU, Network) to
/// observe the signal after `browser.close()` already drove the graceful path.
#[cfg(feature = "js-rendering")]
const PGROUP_SIGTERM_GRACE: Duration = Duration::from_millis(50);

#[cfg(feature = "js-rendering")]
async fn fetch_with_cdp(
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
fn spawn_chromium_pgroup(
    browser_path: &Path,
) -> Result<(TokioChild, Pid, BufReader<ChildStderr>), BrowserError> {
    use std::env::temp_dir;
    use std::process::{Stdio, id};

    // PID suffix prevents `SingletonLock` failure when two scout processes
    // run --js concurrently (chromium refuses to share a profile dir).
    let user_data_dir = temp_dir().join(format!("scout-chromium-{}", id()));
    let mut cmd = TokioCommand::new(browser_path);
    cmd.arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .args(build_launch_args())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| BrowserError::ProcessFailed(format!("spawn chromium: {e}")))?;
    let pid = child
        .id()
        .ok_or_else(|| BrowserError::ProcessFailed("chromium pid unavailable".into()))?;
    let pgid = Pid::from_raw(
        i32::try_from(pid)
            .map_err(|_| BrowserError::ProcessFailed("chromium pid out of i32 range".into()))?,
    );

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| BrowserError::ProcessFailed("chromium stderr missing".into()))?;
    Ok((child, pgid, BufReader::new(stderr)))
}

/// Read chromium stderr line-by-line until `DevTools listening on ws://...`.
///
/// Mirrors chromiumoxide 0.9's `ws_url_from_output` — the marker has been
/// stable in Chrome/Chromium for years. Generic over `AsyncBufRead` so unit
/// tests can drive it with an in-memory cursor.
#[cfg(feature = "js-rendering")]
async fn parse_ws_url_from_lines<R>(reader: R) -> Result<String, BrowserError>
where
    R: AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut lines = reader.lines();
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("stderr read: {e}")))?;
        let Some(line) = line else {
            return Err(BrowserError::ProcessFailed(
                "chromium exited before announcing DevTools URL".into(),
            ));
        };
        if let Some((_, ws)) = line.rsplit_once("listening on ")
            && ws.starts_with("ws")
            && ws.contains("devtools/browser")
        {
            return Ok(ws.trim().to_owned());
        }
    }
}

/// Send SIGTERM to the pgroup, wait a short grace, then SIGKILL.
///
/// `ESRCH` from the first killpg means the group already exited (the common
/// case after a successful `browser.close()`); skip the grace + SIGKILL in
/// that branch to avoid a 50 ms cleanup penalty on every `--js` fetch. The
/// `Child` is awaited unconditionally so the kernel can reap the parent and
/// we don't leave a zombie pid behind.
#[cfg(feature = "js-rendering")]
async fn reap_pgroup(pgid: Pid, child: &mut TokioChild) {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};

    let term = killpg(pgid, Signal::SIGTERM);
    let already_gone = matches!(term, Err(Errno::ESRCH));
    if let Err(e) = term
        && e != Errno::ESRCH
    {
        warn!(error = %e, pgid = %pgid, "killpg SIGTERM failed");
    }
    if !already_gone {
        sleep(PGROUP_SIGTERM_GRACE).await;
        if let Err(e) = killpg(pgid, Signal::SIGKILL)
            && e != Errno::ESRCH
        {
            warn!(error = %e, pgid = %pgid, "killpg SIGKILL failed");
        }
    }
    // Reap so the kernel can release the parent slot. `Ok(Err)` means waitpid
    // itself failed (rare; e.g. ECHILD if a prior wait already reaped); `Err`
    // means the 2s budget elapsed before chromium exited, which is the zombie
    // path scout must surface so SHUTDOWN_DRAIN_TIMEOUT calibration stays
    // honest (issue #152).
    match timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, pgid = %pgid, "chromium child.wait() failed during reap"),
        Err(_) => warn!(
            timeout_secs = 2,
            pgid = %pgid,
            "chromium child did not exit within timeout after SIGKILL"
        ),
    }
}

/// Borrows browser so the caller retains ownership for cleanup on timeout.
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

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
fn resolve_browser_binary_from(
    path_commands: &[&str],
    known_paths: &[&Path],
) -> Result<PathBuf, BrowserError> {
    for cmd in path_commands {
        if let Ok(output) = Command::new("which").arg(cmd).output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            return Ok(PathBuf::from(path));
        }
    }

    for path in known_paths {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
    }

    Err(BrowserError::NotFound)
}

/// Caller MUST pass a [`Client`] with [`reqwest::redirect::Policy::none()`].
///
/// `reqwest::redirect::Policy::limited(n)` is not acceptable: it follows
/// redirects before the application can re-check the resolved URL against
/// the SSRF allowlist. Manual per-hop validation is the only way to enforce
/// the SSRF contract. See ADR-0001 for the contract details.
///
/// `&ValidatedUrl` here closes that gap at the type level — the manual
/// redirect loop cannot accept an unchecked URL.
async fn download(
    client: &Client,
    url: &ValidatedUrl,
    max_redirects: usize,
    resolver: &dyn DnsResolver,
) -> Result<(ValidatedUrl, String), FetchError> {
    let mut current_url = url.clone();

    for _hop in 0..=max_redirects {
        let response = client
            .get(current_url.as_str())
            .header("User-Agent", crate::USER_AGENT)
            .send()
            .await?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::RedirectMissingLocation)?;

            let base = url::Url::parse(current_url.as_str())?;
            let next_url = base.join(location)?.to_string();

            let next_validated = ssrf_check(&next_url, resolver).await?;

            debug!(
                from = %RedactedLogUrl(current_url.as_str()),
                to = %RedactedLogUrl(next_validated.as_str()),
                "following redirect"
            );
            current_url = next_validated;
            continue;
        }

        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }

        let mut charset = None;
        match response.headers().get("content-type") {
            None => {
                debug!(url = %RedactedLogUrl(current_url.as_str()), "no Content-Type header, proceeding as text")
            }
            Some(ct) => match ct.to_str() {
                Ok(ct_str) => {
                    check_content_type(ct_str)?;
                    charset = extract_charset(ct_str);
                }
                Err(_) => {
                    warn!(url = %RedactedLogUrl(current_url.as_str()), "Content-Type header is not valid ASCII, proceeding as text")
                }
            },
        }

        let content_length = response.content_length();
        if let Some(len) = content_length
            && usize::try_from(len).unwrap_or(usize::MAX) > MAX_RESPONSE_BYTES
        {
            return Err(FetchError::TooLarge);
        }

        let capacity = content_length
            .map(|len| {
                usize::try_from(len)
                    .unwrap_or(usize::MAX)
                    .min(MAX_RESPONSE_BYTES)
            })
            .unwrap_or(8192);
        let mut body = Vec::with_capacity(capacity);
        let mut stream = response;
        while let Some(chunk) = stream.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_RESPONSE_BYTES {
                return Err(FetchError::TooLarge);
            }
        }
        let html = decode_body(&body, charset.as_deref());
        return Ok((current_url, html));
    }

    // CALIBRATION (issue #145 / #148 follow-up): structured fields below let
    // callers sample empirical retry-success rate via `RUST_LOG=scout=warn`.
    // Flip from DataError(65) to TempFailure(75) once rate > 10%.
    let chain_length = max_redirects + 1;
    warn!(
        redirect_chain_length = chain_length,
        max_redirects,
        final_url = %RedactedLogUrl(current_url.as_str()),
        "redirect cap exceeded"
    );
    Err(FetchError::TooManyRedirects(max_redirects))
}

fn extract_charset(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let param = param.trim();
        let lower = param.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("charset=") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
        None
    })
}

fn decode_body(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.unwrap_or("utf-8");
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or_else(|| {
        warn!(
            charset = label,
            "unknown charset label, falling back to UTF-8"
        );
        encoding_rs::UTF_8
    });
    if encoding == encoding_rs::UTF_8 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        warn!(
            charset = label,
            "lossy decoding: some bytes could not be decoded"
        );
    }
    decoded.into_owned()
}

fn check_content_type(content_type: &str) -> Result<(), FetchError> {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    if !mime.is_empty()
        && !mime.starts_with("text/")
        && mime != "application/xhtml+xml"
        && mime != "application/xml"
    {
        return Err(FetchError::UnsupportedContentType(mime.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod charset_tests {
    use super::*;

    /// [T-F001] extracts_charset_from_content_type
    #[test]
    fn extracts_charset_from_content_type() {
        assert_eq!(
            extract_charset("text/html; charset=utf-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            extract_charset("text/html; charset=Shift_JIS").as_deref(),
            Some("shift_jis")
        );
        assert_eq!(
            extract_charset("text/html; charset=\"EUC-KR\"").as_deref(),
            Some("euc-kr")
        );
    }

    /// [T-F002] returns_none_when_no_charset
    #[test]
    fn returns_none_when_no_charset() {
        assert!(extract_charset("text/html").is_none());
        assert!(extract_charset("text/plain; boundary=something").is_none());
    }

    /// [T-F003] decode_body_handles_utf8
    #[test]
    fn decode_body_handles_utf8() {
        let bytes = "こんにちは".as_bytes();
        assert_eq!(decode_body(bytes, Some("utf-8")), "こんにちは");
        assert_eq!(decode_body(bytes, None), "こんにちは");
    }

    /// [T-F004] decode_body_handles_shift_jis
    #[test]
    fn decode_body_handles_shift_jis() {
        let encoding = encoding_rs::SHIFT_JIS;
        let (bytes, _, _) = encoding.encode("テスト");
        assert_eq!(decode_body(&bytes, Some("shift_jis")), "テスト");
    }

    /// [T-F005] decode_body_handles_euc_jp
    #[test]
    fn decode_body_handles_euc_jp() {
        let encoding = encoding_rs::EUC_JP;
        let (bytes, _, _) = encoding.encode("日本語");
        assert_eq!(decode_body(&bytes, Some("euc-jp")), "日本語");
    }

    /// [T-F006] decode_body_falls_back_to_utf8_for_unknown
    #[test]
    fn decode_body_falls_back_to_utf8_for_unknown() {
        let bytes = "hello".as_bytes();
        assert_eq!(decode_body(bytes, Some("unknown-encoding")), "hello");
    }
}

#[cfg(test)]
mod content_type_tests {
    use super::*;

    /// [T-F007] accepts_textual_content_types
    #[test]
    fn accepts_textual_content_types() {
        for ct in [
            "text/html; charset=utf-8",
            "text/plain",
            "application/xhtml+xml",
            "application/xml",
            "; charset=utf-8", // edge: empty mime before semicolon → permissive
        ] {
            assert!(check_content_type(ct).is_ok(), "should accept: {ct}");
        }
    }

    /// [T-F008] rejects_non_textual_content_types
    #[test]
    fn rejects_non_textual_content_types() {
        for ct in ["application/pdf", "image/png", "application/json"] {
            assert!(
                matches!(
                    check_content_type(ct),
                    Err(FetchError::UnsupportedContentType(ref m)) if m == ct
                ),
                "should reject: {ct}"
            );
        }
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use crate::test_support::try_spawn_mock_server;
    use reqwest::redirect::Policy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn no_redirect_client() -> Client {
        Client::builder().redirect(Policy::none()).build().unwrap()
    }

    fn validated(url: &str) -> ValidatedUrl {
        ValidatedUrl::for_test(url)
    }

    fn public_resolver() -> ssrf::StaticDnsResolver {
        ssrf::StaticDnsResolver::single("8.8.8.8")
    }

    /// [T-F009] download_success_returns_html
    #[tokio::test]
    async fn download_success_returns_html() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body><p>hello</p></body></html>"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let (final_url, html) = download(
            &client,
            &validated(&format!("{}/page", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await
        .unwrap();

        assert!(final_url.as_str().contains("/page"));
        assert!(html.contains("hello"));
    }

    /// [T-F010] download_non_success_returns_status_error
    #[tokio::test]
    async fn download_non_success_returns_status_error() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/404"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/500"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = no_redirect_client();
        assert!(matches!(
            download(
                &client,
                &validated(&format!("{}/404", server.uri())),
                MAX_REDIRECTS,
                &public_resolver()
            )
            .await,
            Err(FetchError::Status(404))
        ));
        assert!(matches!(
            download(
                &client,
                &validated(&format!("{}/500", server.uri())),
                MAX_REDIRECTS,
                &public_resolver()
            )
            .await,
            Err(FetchError::Status(500))
        ));
    }

    /// [T-F011] download_too_large_body_rejected
    #[tokio::test]
    async fn download_too_large_body_rejected() {
        let oversized = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/huge"))
            .respond_with(ResponseTemplate::new(200).set_body_string(oversized))
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/huge", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(matches!(result, Err(FetchError::TooLarge)));
    }

    /// [T-F012] download_rejects_non_html_content_type
    #[tokio::test]
    async fn download_rejects_non_html_content_type() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/binary"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(b"fake pdf".to_vec()),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/binary", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::UnsupportedContentType(ref ct)) if ct == "application/pdf"),
            "got: {result:?}"
        );
    }

    /// [T-F013] redirect_to_private_ip_blocked
    #[tokio::test]
    async fn redirect_to_private_ip_blocked() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://127.0.0.1/secret"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "redirect to 127.0.0.1 should be blocked, got: {result:?}"
        );
    }

    /// [T-F014] redirect_to_dns_private_ip_blocked
    #[tokio::test]
    async fn redirect_to_dns_private_ip_blocked() {
        let private_resolver = ssrf::StaticDnsResolver::single("10.0.0.1");

        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://evil.com/internal"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            MAX_REDIRECTS,
            &private_resolver,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "redirect to domain resolving to private IP should be blocked, got: {result:?}"
        );
    }

    /// [T-F015] too_many_redirects_returns_error
    #[tokio::test]
    async fn too_many_redirects_returns_error() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://example.com/next"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            0, // max_redirects = 0
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::TooManyRedirects(0))),
            "should error on too many redirects, got: {result:?}"
        );
    }

    /// [T-F056] redirect_cap_exceeded_emits_calibration_warn — `redirect cap
    /// exceeded` warn must carry structured fields (`redirect_chain_length`,
    /// `max_redirects`, `final_url`) so caller logs can sample retry-success
    /// rate for the DataError vs TempFailure flip decision (issue #145).
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn redirect_cap_exceeded_emits_calibration_warn() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://example.com/next"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            0, // max_redirects = 0
            &public_resolver(),
        )
        .await;
        assert!(matches!(result, Err(FetchError::TooManyRedirects(0))));
        assert!(logs_contain("redirect cap exceeded"));
        assert!(logs_contain("redirect_chain_length"));
        assert!(logs_contain("max_redirects"));
        assert!(logs_contain("final_url"));
    }

    /// [T-F016] redirect_missing_location_header_returns_error
    #[tokio::test]
    async fn redirect_missing_location_header_returns_error() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/bad-redir"))
            .respond_with(ResponseTemplate::new(302))
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/bad-redir", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::RedirectMissingLocation)),
            "missing Location header should error, got: {result:?}"
        );
    }
}

#[cfg(test)]
mod fetch_page_tests {
    use super::*;
    use crate::test_support::try_spawn_mock_server;
    use reqwest::redirect::Policy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn no_redirect_client() -> Client {
        Client::builder().redirect(Policy::none()).build().unwrap()
    }

    fn real_resolver() -> Arc<dyn DnsResolver> {
        Arc::new(TokioDnsResolver)
    }

    /// [T-F017] blocks_ssrf_to_localhost
    #[tokio::test]
    async fn blocks_ssrf_to_localhost() {
        let client = no_redirect_client();
        let (cancel, _) = watch::channel(false);
        let result = fetch_page(
            &client,
            "http://127.0.0.1/secret",
            FetchOptions::default(),
            real_resolver(),
            &cancel,
        )
        .await;
        assert!(matches!(result, Err(FetchError::InternalHost)));
    }

    /// [T-F052] fetch_does_not_log_userinfo_credentials_on_blocked_url
    ///
    /// Adversarial: even when SSRF blocks the fetch, the `warn!` line emitted
    /// by `ssrf_check` MUST flow through `redact_url_credentials` so no
    /// password fragment ever appears in stderr / `tracing` output.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn fetch_does_not_log_userinfo_credentials_on_blocked_url() {
        let client = no_redirect_client();
        let (cancel, _) = watch::channel(false);
        let result = fetch_page(
            &client,
            "http://user:supersecret@127.0.0.1/private",
            FetchOptions::default(),
            real_resolver(),
            &cancel,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "should be blocked as InternalHost, got: {result:?}"
        );
        // Positive anchor: a future refactor that drops the warn! line
        // entirely would silently make the userinfo asserts vacuous.
        assert!(
            logs_contain("blocked fetch to internal/private host"),
            "expected the SSRF block warning to fire",
        );
        assert!(
            !logs_contain("supersecret"),
            "password fragment must not appear in logs",
        );
        assert!(
            !logs_contain("user:"),
            "userinfo must be stripped from logs",
        );
    }

    /// [T-F018] js_flag_attempts_rendering_on_rich_body
    #[tokio::test]
    async fn js_flag_attempts_rendering_on_rich_body() {
        let content = "x".repeat(200);
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/rich"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body><p>{content}</p></body></html>")),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let opts = FetchOptions {
            js: true,
            ..Default::default()
        };
        let (cancel, _) = watch::channel(false);
        let result = fetch_page(
            &client,
            &format!("{}/rich", server.uri()),
            opts,
            real_resolver(),
            &cancel,
        )
        .await;

        assert!(
            result.is_err(),
            "js=true should error when browser unavailable"
        );
    }

    /// [T-F019] t010_js_flag_errors_when_feature_disabled
    #[cfg(not(feature = "js-rendering"))]
    #[tokio::test]
    async fn t010_js_flag_errors_when_feature_disabled() {
        let client = no_redirect_client();
        let opts = FetchOptions {
            js: true,
            ..Default::default()
        };
        let (cancel, _) = watch::channel(false);
        let result = fetch_page(
            &client,
            "https://example.com/page",
            opts,
            real_resolver(),
            &cancel,
        )
        .await;

        assert!(
            matches!(&result, Err(FetchError::BrowserNotFound(msg)) if msg.contains("js-rendering")),
            "expected BrowserNotFound error with feature hint, got: {result:?}"
        );
    }
}

#[cfg(test)]
mod js_dependent_tests {
    use super::*;

    /// [T-F020] all_spa_frameworks_detected
    #[test]
    fn all_spa_frameworks_detected() {
        for id in SPA_ROOT_IDS {
            let html = format!(
                r#"<html><head><script src="app.js"></script></head>
                <body><div {id}></div></body></html>"#
            );
            assert!(is_js_dependent(&html), "should detect SPA with {id}");
        }
    }

    /// [T-F021] normal_html_not_detected
    #[test]
    fn normal_html_not_detected() {
        let html = r#"<html><body><article>
        <h1>Title</h1><p>Long paragraph with enough content to exceed
        the threshold of one hundred characters easily.</p>
        </article></body></html>"#;
        assert!(!is_js_dependent(html));
    }

    /// [T-F022] script_without_spa_pattern_but_empty_body
    #[test]
    fn script_without_spa_pattern_but_empty_body() {
        let html = r#"<html><head><script src="bundle.js"></script></head>
        <body><div class="app"></div></body></html>"#;
        assert!(is_js_dependent(html));
    }

    /// [T-F023] spa_pattern_without_script_but_empty_body
    #[test]
    fn spa_pattern_without_script_but_empty_body() {
        let html = r#"<html><body><div id="root"></div></body></html>"#;
        assert!(is_js_dependent(html));
    }

    /// [T-F024] rich_body_with_scripts_not_detected
    #[test]
    fn rich_body_with_scripts_not_detected() {
        let content = "x".repeat(200);
        let html = format!(
            r#"<html><head><script src="app.js"></script></head>
            <body><div id="root"><p>{content}</p></div></body></html>"#
        );
        assert!(!is_js_dependent(&html));
    }

    /// [T-F025] thin_body_without_script_or_spa_pattern_not_detected
    #[test]
    fn thin_body_without_script_or_spa_pattern_not_detected() {
        let html = "<html><body><p>short</p></body></html>";
        assert!(!is_js_dependent(html));
    }

    /// [T-F026] no_body_tag_falls_back_to_full_html
    #[test]
    fn no_body_tag_falls_back_to_full_html() {
        let html = r#"<div id="root"></div><script src="app.js"></script>"#;
        assert!(is_js_dependent(html));
    }
}

#[cfg(test)]
mod thin_body_tests {
    use super::*;

    /// [T-F027] style_content_excluded_from_visible_text
    #[test]
    fn style_content_excluded_from_visible_text() {
        let html = "<html><body><style>.big{font-size:9999px;color:red;margin:0 auto;padding:10px 20px 30px 40px}</style><p>hi</p></body></html>";
        assert!(has_thin_body(html));
    }

    /// [T-F028] uppercase_script_tag_excluded
    #[test]
    fn uppercase_script_tag_excluded() {
        let html = "<html><body><SCRIPT>var x = 'lots of javascript code that should be ignored by the parser';</SCRIPT><p>hi</p></body></html>";
        assert!(has_thin_body(html));
    }

    /// [T-F029] uppercase_body_tag_found
    #[test]
    fn uppercase_body_tag_found() {
        let content = "x".repeat(200);
        let html = format!("<html><BODY><p>{content}</p></BODY></html>");
        assert!(!has_thin_body(&html));
    }

    /// [T-F030] exactly_at_threshold_is_not_thin (body)
    #[test]
    fn exactly_at_threshold_is_not_thin() {
        let content = "x".repeat(BODY_TEXT_THRESHOLD);
        let html = format!("<html><body><p>{content}</p></body></html>");
        assert!(!has_thin_body(&html));
    }

    /// [T-F031] just_below_threshold_is_thin (body)
    #[test]
    fn just_below_threshold_is_thin() {
        let content = "x".repeat(BODY_TEXT_THRESHOLD - 1);
        let html = format!("<html><body><p>{content}</p></body></html>");
        assert!(has_thin_body(&html));
    }

    /// [T-F032] whitespace_only_body_is_thin
    #[test]
    fn whitespace_only_body_is_thin() {
        let html = "<html><body>   \n\t  \n   </body></html>";
        assert!(has_thin_body(html));
    }
}

#[cfg(test)]
mod thin_extract_tests {
    use super::*;
    use extractor::ExtractedArticle;

    fn article(content_html: &str, used_raw_fallback: bool) -> ExtractedArticle {
        ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: content_html.to_owned(),
            used_raw_fallback,
        }
    }

    /// [T-F033] raw_fallback_with_short_content_is_thin
    #[test]
    fn raw_fallback_with_short_content_is_thin() {
        assert!(is_thin_extract(&article("<p>short</p>", true)));
    }

    /// [T-F034] raw_fallback_with_rich_content_still_thin
    #[test]
    fn raw_fallback_with_rich_content_still_thin() {
        let content = format!("<p>{}</p>", "x".repeat(100));
        assert!(is_thin_extract(&article(&content, true)));
    }

    /// [T-F035] short_content_is_thin
    #[test]
    fn short_content_is_thin() {
        assert!(is_thin_extract(&article("<p>hi</p>", false)));
    }

    /// [T-F036] sufficient_content_is_not_thin
    #[test]
    fn sufficient_content_is_not_thin() {
        let content = format!("<p>{}</p>", "x".repeat(100));
        assert!(!is_thin_extract(&article(&content, false)));
    }

    /// [T-F037] exactly_at_threshold_is_not_thin (extract)
    #[test]
    fn exactly_at_threshold_is_not_thin() {
        let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD));
        assert!(!is_thin_extract(&article(&content, false)));
    }

    /// [T-F038] just_below_threshold_is_thin (extract)
    #[test]
    fn just_below_threshold_is_thin() {
        let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD - 1));
        assert!(is_thin_extract(&article(&content, false)));
    }

    /// [T-F039] html_tags_excluded_from_count
    #[test]
    fn html_tags_excluded_from_count() {
        let content = r#"<div class="very-long-class-name"><span>ab</span></div>"#;
        assert!(is_thin_extract(&article(content, false)));
    }

    /// [T-F040] whitespace_excluded_from_count
    #[test]
    fn whitespace_excluded_from_count() {
        let content = format!("<p>{}</p>", " x ".repeat(30));
        assert!(is_thin_extract(&article(&content, false)));
    }
}

#[cfg(test)]
mod browser_binary_tests {
    use super::*;
    use std::env;

    /// [T-F041] t001_returns_error_when_chrome_not_found
    #[test]
    fn t001_returns_error_when_chrome_not_found() {
        let result = resolve_browser_binary_from(&[], &[]);
        assert!(
            matches!(result, Err(BrowserError::NotFound)),
            "expected NotFound, got: {result:?}"
        );
    }

    /// [T-F042] finds_binary_at_known_path
    #[test]
    fn finds_binary_at_known_path() {
        let existing = env::current_exe().unwrap();
        let result = resolve_browser_binary_from(&[], &[existing.as_path()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), existing);
    }
}

#[cfg(test)]
#[cfg(feature = "js-rendering")]
mod cdp_launch_tests {
    use super::*;

    /// [T-F043] t009_launch_args_contain_security_flags
    #[test]
    fn t009_launch_args_contain_security_flags() {
        let args = build_launch_args();
        for flag in [
            "--disable-webrtc",
            "--disable-background-networking",
            "--disable-features=DnsOverHttps",
            "--disable-domain-reliability",
            "--no-pings",
        ] {
            assert!(args.contains(&flag), "missing security flag: {flag}");
        }
    }
}

#[cfg(test)]
mod browser_request_tests {
    use super::ssrf::StaticDnsResolver;
    use super::*;

    fn private_dns() -> StaticDnsResolver {
        StaticDnsResolver::single("10.0.0.1")
    }

    fn public_dns() -> StaticDnsResolver {
        StaticDnsResolver::single("93.184.216.34")
    }

    /// [T-F044] t004_blocks_dns_resolving_to_private_ip
    #[tokio::test]
    async fn t004_blocks_dns_resolving_to_private_ip() {
        let resolver = private_dns();
        assert!(
            !check_browser_request("https://evil.example/secret", &resolver).await,
            "must block when DNS resolves to private IP"
        );
    }

    /// [T-F045] t004_blocks_internal_ip_literal
    #[tokio::test]
    async fn t004_blocks_internal_ip_literal() {
        let resolver = public_dns();
        assert!(
            !check_browser_request("http://127.0.0.1/secret", &resolver).await,
            "must block loopback IP"
        );
    }

    /// [T-F046] t004_allows_public_url
    #[tokio::test]
    async fn t004_allows_public_url() {
        let resolver = public_dns();
        assert!(
            check_browser_request("https://example.com/page", &resolver).await,
            "must allow public URL"
        );
    }

    /// [T-F047] t004_allows_non_network_urls
    #[tokio::test]
    async fn t004_allows_non_network_urls() {
        let resolver = public_dns();
        for url in [
            "data:text/html,<p>test</p>",
            "about:blank",
            "chrome://settings",
            "blob:https://example.com/uuid",
        ] {
            assert!(
                check_browser_request(url, &resolver).await,
                "must allow non-network URL: {url}"
            );
        }
    }

    /// [T-F048] t004_blocks_unknown_schemes
    #[tokio::test]
    async fn t004_blocks_unknown_schemes() {
        let resolver = public_dns();
        for url in ["file:///etc/passwd", "ftp://internal/data", "gopher://x"] {
            assert!(
                !check_browser_request(url, &resolver).await,
                "must block unknown scheme: {url}"
            );
        }
    }

    /// [T-F049] t004_blocks_websocket_to_internal
    #[tokio::test]
    async fn t004_blocks_websocket_to_internal() {
        let resolver = public_dns();
        assert!(
            !check_browser_request("ws://127.0.0.1:8080/ws", &resolver).await,
            "must block ws:// to loopback"
        );
        assert!(
            !check_browser_request("wss://localhost/ws", &resolver).await,
            "must block wss:// to localhost"
        );
    }

    /// [T-F050] t004_blocks_websocket_dns_to_private
    #[tokio::test]
    async fn t004_blocks_websocket_dns_to_private() {
        let resolver = private_dns();
        assert!(
            !check_browser_request("ws://evil.example/ws", &resolver).await,
            "must block ws:// when DNS resolves to private IP"
        );
    }
}

#[cfg(test)]
#[cfg(feature = "js-rendering")]
mod ws_url_parse_tests {
    use super::*;

    /// [T-F052] parse_ws_url_extracts_first_matching_line
    #[tokio::test]
    async fn parse_ws_url_extracts_first_matching_line() {
        let stderr = b"[chromium] starting up\n\
                       DevTools listening on ws://127.0.0.1:54321/devtools/browser/abc-123\n\
                       DevTools listening on ws://127.0.0.1:54321/devtools/browser/def-456\n";
        let url = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect("first match should win");
        assert_eq!(url, "ws://127.0.0.1:54321/devtools/browser/abc-123");
    }

    /// [T-F053] parse_ws_url_skips_unrelated_lines_until_match
    #[tokio::test]
    async fn parse_ws_url_skips_unrelated_lines_until_match() {
        let stderr = b"[8765:0x110000000] preference manifest unparseable\n\
                       [warn] hardware acceleration unavailable\n\
                       random listening on something else\n\
                       DevTools listening on ws://localhost:1234/devtools/browser/xyz\n";
        let url = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect("should match after unrelated prefix");
        assert_eq!(url, "ws://localhost:1234/devtools/browser/xyz");
    }

    /// [T-F054] parse_ws_url_eof_before_match_errors
    #[tokio::test]
    async fn parse_ws_url_eof_before_match_errors() {
        let stderr = b"chromium crashed before opening port\n";
        let err = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect_err("EOF without match must surface as error");
        let msg = err.to_string();
        assert!(
            msg.contains("chromium exited before announcing DevTools URL"),
            "expected EOF message, got: {msg}"
        );
    }

    /// [T-F055] parse_ws_url_rejects_non_browser_devtools_url
    ///
    /// chromium also prints `DevTools listening on ws://.../page/<id>` for
    /// per-page debuggers — we must only accept the browser-level URL.
    #[tokio::test]
    async fn parse_ws_url_rejects_non_browser_devtools_url() {
        let stderr = b"DevTools listening on ws://127.0.0.1:9999/devtools/page/something\n";
        let err = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect_err("page-level URL must not match");
        assert!(
            err.to_string()
                .contains("chromium exited before announcing DevTools URL")
        );
    }
}

#[cfg(test)]
#[cfg(feature = "js-rendering")]
mod cdp_integration_tests {
    use super::*;

    fn chrome_available() -> bool {
        resolve_browser_binary().is_ok()
    }

    /// [T-F051] t005_cdp_renders_public_url
    #[tokio::test]
    async fn t005_cdp_renders_public_url() {
        if !chrome_available() {
            eprintln!("SKIP: Chrome not found");
            return;
        }
        let (cancel, _) = watch::channel(false);
        let html = fetch_with_cdp(
            &ValidatedUrl::for_test("https://example.com"),
            Arc::new(TokioDnsResolver),
            &cancel,
        )
        .await
        .expect("fetch_with_cdp should succeed for public URL");
        assert!(
            html.contains("Example Domain") || html.contains("example"),
            "rendered HTML should contain page content, got {} bytes",
            html.len()
        );
    }
}
