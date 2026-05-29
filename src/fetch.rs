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

use crate::envelope::ErrorCode;
use crate::retry::is_transient_network;
use crate::tools::Classification;

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

impl FetchError {
    /// Map each variant to its ADR-0011 priority-table [`Classification`].
    ///
    /// Arm order is load-bearing: specific `Status` codes (401/403, 404,
    /// 408/429) precede the 4xx fallback so a reorder cannot silently demote
    /// them to DataError.
    pub(crate) fn classify(&self) -> Classification {
        match self {
            // Priority 1: USAGE_ERROR
            Self::BrowserNotFound(_) => Classification::new(ErrorCode::UsageError),
            // Priority 2: DATA_ERROR (non-Status variants)
            Self::InvalidScheme => Classification::new(ErrorCode::DataError)
                .with_hint("URL must use http:// or https://"),
            Self::InvalidUrl(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL must include scheme and host"),
            Self::InternalHost => Classification::new(ErrorCode::DataError)
                .with_hint("URL must point to an external host (private IPs are blocked)"),
            Self::UnsupportedContentType(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL must serve HTML or text content"),
            Self::RedirectMissingLocation => Classification::new(ErrorCode::DataError),
            Self::TooLarge => {
                Classification::new(ErrorCode::DataError).with_hint("fetch a smaller resource")
            }
            // Terminal DataError: cap=5 absorbs canonical chains (HTTPS upgrade →
            // trailing slash → final URL), so a breach dominantly indicates a
            // server-side redirect loop or caller URL mistake — both caller-fixable.
            Self::TooManyRedirects(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL has too many redirects; check for a redirect loop"),
            // Priority 1: USAGE_ERROR (specific HTTP codes before 4xx fallback)
            Self::Status(401 | 403) => Classification::new(ErrorCode::UsageError)
                .with_hint("URL requires authentication that scout does not support"),
            // Priority 3: NOT_FOUND
            Self::Status(404) => Classification::new(ErrorCode::NotFound)
                .with_hint("Check that the URL is correct and the resource exists"),
            // Priority 4: TEMP_FAILURE
            Self::Status(408 | 429) => Classification::transient_retry(),
            // Priority 2: DATA_ERROR (4xx body)
            Self::Status(code) if (400..500).contains(code) => {
                Classification::new(ErrorCode::DataError)
            }
            // Priority 4: TEMP_FAILURE (5xx and other unmatched)
            Self::Status(_) => Classification::transient_retry(),
            // Priority 4: TIMEOUT (transport timeout — long-backoff retry advised)
            Self::Timeout(_) => Classification::timeout_retry(),
            // Priority 4: TEMP_FAILURE (non-Status variants)
            Self::DnsResolution(_) => Classification::new(ErrorCode::TempFailure)
                .with_hint("Check the URL's domain name and your DNS resolver"),
            // `is_transient_network` covers connect, timeout, and mid-stream
            // body drop (issue #113), but ADR-0002 splits timeout into 124.
            // Check `is_timeout()` first.
            Self::Http(re) if re.is_timeout() => Classification::timeout_retry(),
            Self::Http(re) if is_transient_network(re) => Classification::transient_network(),
            // Priority 5 sibling: IO_ERROR — external tool failure (browser)
            Self::BrowserFailed(_) => Classification::new(ErrorCode::IoError),
            // Unknown — reqwest errors that do not match transient network patterns
            Self::Http(_) => Classification::new(ErrorCode::Unknown),
        }
    }
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
                    info!("JS rendering succeeded via CDP");
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
mod tests;
