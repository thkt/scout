//! Web page fetching with SSRF defense-in-depth.
//!
//! URL validation → DNS pre-check → download → post-redirect recheck → content extraction.

pub(crate) mod converter;
mod extractor;
mod ssrf;

pub(crate) use ssrf::{DnsResolver, TokioDnsResolver};
use ssrf::{redact_url_credentials, ssrf_check};

use converter::{FetchResult, to_fetch_result};
use extractor::{extract_article, extract_raw};
use reqwest::Client;
use reqwest::header::LOCATION;

use tracing::{debug, info, warn};

/// Options for [`fetch_page`] that control rendering and output.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchOptions {
    /// Force JS rendering via CDP (skip auto-detection). Requires `js-rendering` feature.
    pub js: bool,
    /// Skip Readability extraction; return full HTML converted to Markdown.
    pub raw: bool,
}

const MAX_RESPONSE_BYTES: usize = 10_000_000;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
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

pub async fn fetch_page(
    client: &Client,
    url: &str,
    opts: FetchOptions,
    resolver: &impl DnsResolver,
) -> Result<FetchResult, FetchError> {
    #[cfg(not(feature = "js-rendering"))]
    if opts.js {
        return Err(FetchError::BrowserNotFound(
            "js-rendering feature required — rebuild with `--features js-rendering`".into(),
        ));
    }

    // SECURITY: Local CLI only. TOCTOU gap between DNS check and reqwest connect
    // is acceptable here; a network service would need a custom resolver that
    // enforces the allowlist at connect time.
    ssrf_check(url, resolver).await?;

    #[cfg(feature = "js-rendering")]
    let (final_url, mut html) = download(client, url, MAX_REDIRECTS, resolver).await?;
    #[cfg(not(feature = "js-rendering"))]
    let (final_url, html) = download(client, url, MAX_REDIRECTS, resolver).await?;

    // Defense-in-depth: catch bugs in the manual redirect loop.
    ssrf_check(&final_url, resolver).await?;

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
            match fetch_with_cdp(&final_url, Clone::clone(resolver)).await {
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
        extract_article(&html, Some(&final_url))
    };

    let need_thin_fallback = !opts.raw && !need_js && is_thin_extract(&article);
    #[cfg(feature = "js-rendering")]
    let article = if need_thin_fallback {
        warn!(url = %redact_url_credentials(&final_url), "extraction yielded too little content, trying JS rendering fallback");
        match fetch_with_cdp(&final_url, Clone::clone(resolver)).await {
            Ok(js_html) => {
                let re_extracted = extract_article(&js_html, Some(&final_url));
                if is_thin_extract(&re_extracted) {
                    debug!(url = %redact_url_credentials(&final_url), "JS re-extraction still thin, returning best-effort result");
                } else {
                    debug!(url = %redact_url_credentials(&final_url), "JS rendering fallback succeeded (post-extraction)");
                }
                re_extracted
            }
            Err(e) => {
                warn!(url = %redact_url_credentials(&final_url), error = %e, "JS rendering fallback failed, using original extraction");
                article
            }
        }
    } else {
        article
    };
    #[cfg(not(feature = "js-rendering"))]
    if need_thin_fallback {
        warn!(url = %redact_url_credentials(&final_url), "extraction yielded too little content but JS rendering unavailable");
    }

    debug!(url = %redact_url_credentials(&final_url), bytes = html.len(), "page fetched");
    Ok(to_fetch_result(article, final_url))
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
}

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
impl From<BrowserError> for FetchError {
    fn from(e: BrowserError) -> Self {
        match e {
            BrowserError::NotFound => Self::BrowserNotFound(e.to_string()),
            BrowserError::ProcessFailed(msg) => Self::BrowserFailed(msg),
            BrowserError::TimedOut => Self::Timeout(e.to_string()),
        }
    }
}

#[cfg(feature = "js-rendering")]
fn resolve_browser_binary() -> Result<std::path::PathBuf, BrowserError> {
    static CACHE: std::sync::OnceLock<Result<std::path::PathBuf, String>> =
        std::sync::OnceLock::new();

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
        let known_paths: &[&std::path::Path] = if cfg!(target_os = "macos") {
            &[
                std::path::Path::new(
                    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                ),
                std::path::Path::new("/Applications/Chromium.app/Contents/MacOS/Chromium"),
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

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
pub(crate) async fn check_browser_request(url: &str, resolver: &impl ssrf::DnsResolver) -> bool {
    let check_url = if url.starts_with("http://") || url.starts_with("https://") {
        std::borrow::Cow::Borrowed(url)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        std::borrow::Cow::Owned(format!("http://{rest}"))
    } else if let Some(rest) = url.strip_prefix("wss://") {
        std::borrow::Cow::Owned(format!("https://{rest}"))
    } else if url.starts_with("data:")
        || url.starts_with("about:")
        || url.starts_with("chrome:")
        || url.starts_with("blob:")
    {
        return true;
    } else {
        warn!(url = %url, "SSRF: blocked browser subrequest with unrecognized scheme");
        return false;
    };
    ssrf::ssrf_check(&check_url, resolver).await.is_ok()
}

#[cfg(feature = "js-rendering")]
const CDP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(feature = "js-rendering")]
async fn fetch_with_cdp(
    url: &str,
    resolver: impl ssrf::DnsResolver,
) -> Result<String, BrowserError> {
    use chromiumoxide::Browser;
    use chromiumoxide::browser::BrowserConfig;
    use futures::StreamExt;

    let browser_path = resolve_browser_binary()?;

    let mut config_builder = BrowserConfig::builder().chrome_executable(browser_path);
    for arg in build_launch_args() {
        config_builder = config_builder.arg(arg);
    }
    let config = config_builder
        .build()
        .map_err(|e| BrowserError::ProcessFailed(format!("browser config: {e}")))?;

    let (mut browser, mut handler) = tokio::time::timeout(CDP_TIMEOUT, Browser::launch(config))
        .await
        .map_err(|_| BrowserError::TimedOut)?
        .map_err(|e| BrowserError::ProcessFailed(format!("browser launch: {e}")))?;

    let handler_task = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if h.is_err() {
                break;
            }
        }
    });

    // unwrap_or (not ?) so cleanup below runs even on timeout.
    let result = tokio::time::timeout(CDP_TIMEOUT, cdp_navigate(&mut browser, url, resolver))
        .await
        .unwrap_or(Err(BrowserError::TimedOut));

    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), browser.close()).await;
    handler_task.abort();

    result
}

/// Borrows browser so the caller retains ownership for cleanup on timeout.
#[cfg(feature = "js-rendering")]
async fn cdp_navigate(
    browser: &mut chromiumoxide::Browser,
    url: &str,
    resolver: impl ssrf::DnsResolver,
) -> Result<String, BrowserError> {
    use chromiumoxide::cdp::browser_protocol::fetch::{
        ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams,
    };
    use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
    use futures::StreamExt;

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

    let intercept_page = page.clone();
    let interceptor = tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let req_url = &event.request.url;
            let allowed = check_browser_request(req_url, &resolver).await;
            if allowed {
                if let Ok(cmd) = ContinueRequestParams::builder()
                    .request_id(event.request_id.clone())
                    .build()
                {
                    let _ = intercept_page.execute(cmd).await;
                }
            } else {
                warn!(blocked_url = %req_url, "SSRF: blocked browser subrequest");
                if let Ok(cmd) = FailRequestParams::builder()
                    .request_id(event.request_id.clone())
                    .error_reason(ErrorReason::BlockedByClient)
                    .build()
                {
                    let _ = intercept_page.execute(cmd).await;
                }
            }
        }
    });

    let result = async {
        page.goto(url)
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("navigation: {e}")))?;

        page.content()
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("content: {e}")))
    }
    .await;

    interceptor.abort();
    result
}

#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
fn resolve_browser_binary_from(
    path_commands: &[&str],
    known_paths: &[&std::path::Path],
) -> Result<std::path::PathBuf, BrowserError> {
    for cmd in path_commands {
        if let Ok(output) = std::process::Command::new("which").arg(cmd).output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok(std::path::PathBuf::from(path));
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
async fn download(
    client: &Client,
    url: &str,
    max_redirects: usize,
    resolver: &impl DnsResolver,
) -> Result<(String, String), FetchError> {
    let mut current_url = url.to_string();

    for _hop in 0..=max_redirects {
        let response = client
            .get(&current_url)
            .header("User-Agent", crate::USER_AGENT)
            .send()
            .await?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::RedirectMissingLocation)?;

            let base = url::Url::parse(&current_url)?;
            let next_url = base.join(location)?.to_string();

            ssrf_check(&next_url, resolver).await?;

            debug!(
                from = %redact_url_credentials(&current_url),
                to = %redact_url_credentials(&next_url),
                "following redirect"
            );
            current_url = next_url;
            continue;
        }

        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }

        let mut charset = None;
        match response.headers().get("content-type") {
            None => {
                debug!(url = %redact_url_credentials(&current_url), "no Content-Type header, proceeding as text")
            }
            Some(ct) => match ct.to_str() {
                Ok(ct_str) => {
                    check_content_type(ct_str)?;
                    charset = extract_charset(ct_str);
                }
                Err(_) => {
                    debug!(url = %redact_url_credentials(&current_url), "Content-Type header is not valid ASCII, proceeding as text")
                }
            },
        }

        let content_length = response.content_length();
        if let Some(len) = content_length
            && len as usize > MAX_RESPONSE_BYTES
        {
            return Err(FetchError::TooLarge);
        }

        let capacity = content_length
            .map(|len| (len as usize).min(MAX_RESPONSE_BYTES))
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

    Err(FetchError::TooManyRedirects(max_redirects))
}

fn extract_charset(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let param = param.trim();
        let lower = param.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("charset=") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        None
    })
}

fn decode_body(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.unwrap_or("utf-8");
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
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
        return Err(FetchError::UnsupportedContentType(mime.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod charset_tests {
    use super::*;

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

    #[test]
    fn returns_none_when_no_charset() {
        assert!(extract_charset("text/html").is_none());
        assert!(extract_charset("text/plain; boundary=something").is_none());
    }

    #[test]
    fn decode_body_handles_utf8() {
        let bytes = "こんにちは".as_bytes();
        assert_eq!(decode_body(bytes, Some("utf-8")), "こんにちは");
        assert_eq!(decode_body(bytes, None), "こんにちは");
    }

    #[test]
    fn decode_body_handles_shift_jis() {
        let encoding = encoding_rs::SHIFT_JIS;
        let (bytes, _, _) = encoding.encode("テスト");
        assert_eq!(decode_body(&bytes, Some("shift_jis")), "テスト");
    }

    #[test]
    fn decode_body_handles_euc_jp() {
        let encoding = encoding_rs::EUC_JP;
        let (bytes, _, _) = encoding.encode("日本語");
        assert_eq!(decode_body(&bytes, Some("euc-jp")), "日本語");
    }

    #[test]
    fn decode_body_falls_back_to_utf8_for_unknown() {
        let bytes = "hello".as_bytes();
        assert_eq!(decode_body(bytes, Some("unknown-encoding")), "hello");
    }
}

#[cfg(test)]
mod content_type_tests {
    use super::*;

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
    use std::net::IpAddr;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn no_redirect_client() -> Client {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    #[derive(Clone, Copy)]
    struct PublicResolver;
    impl DnsResolver for PublicResolver {
        async fn lookup(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, FetchError> {
            Ok(vec!["8.8.8.8".parse().unwrap()])
        }
    }

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
            &format!("{}/page", server.uri()),
            MAX_REDIRECTS,
            &PublicResolver,
        )
        .await
        .unwrap();

        assert!(final_url.contains("/page"));
        assert!(html.contains("hello"));
    }

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
                &format!("{}/404", server.uri()),
                MAX_REDIRECTS,
                &PublicResolver
            )
            .await,
            Err(FetchError::Status(404))
        ));
        assert!(matches!(
            download(
                &client,
                &format!("{}/500", server.uri()),
                MAX_REDIRECTS,
                &PublicResolver
            )
            .await,
            Err(FetchError::Status(500))
        ));
    }

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
            &format!("{}/huge", server.uri()),
            MAX_REDIRECTS,
            &PublicResolver,
        )
        .await;
        assert!(matches!(result, Err(FetchError::TooLarge)));
    }

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
            &format!("{}/binary", server.uri()),
            MAX_REDIRECTS,
            &PublicResolver,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::UnsupportedContentType(ref ct)) if ct == "application/pdf"),
            "got: {result:?}"
        );
    }

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
            &format!("{}/redir", server.uri()),
            MAX_REDIRECTS,
            &PublicResolver,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "redirect to 127.0.0.1 should be blocked, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn redirect_to_dns_private_ip_blocked() {
        #[derive(Clone, Copy)]
        struct PrivateResolver;
        impl DnsResolver for PrivateResolver {
            async fn lookup(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, FetchError> {
                Ok(vec!["10.0.0.1".parse().unwrap()])
            }
        }

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
            &format!("{}/redir", server.uri()),
            MAX_REDIRECTS,
            &PrivateResolver,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "redirect to domain resolving to private IP should be blocked, got: {result:?}"
        );
    }

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
            &format!("{}/redir", server.uri()),
            0, // max_redirects = 0
            &PublicResolver,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::TooManyRedirects(0))),
            "should error on too many redirects, got: {result:?}"
        );
    }

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
            &format!("{}/bad-redir", server.uri()),
            MAX_REDIRECTS,
            &PublicResolver,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn no_redirect_client() -> Client {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn blocks_ssrf_to_localhost() {
        let client = no_redirect_client();
        let result = fetch_page(
            &client,
            "http://127.0.0.1/secret",
            FetchOptions::default(),
            &TokioDnsResolver,
        )
        .await;
        assert!(matches!(result, Err(FetchError::InternalHost)));
    }

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
        let result = fetch_page(
            &client,
            &format!("{}/rich", server.uri()),
            opts,
            &TokioDnsResolver,
        )
        .await;

        assert!(
            result.is_err(),
            "js=true should error when browser unavailable"
        );
    }

    #[cfg(not(feature = "js-rendering"))]
    #[tokio::test]
    async fn t010_js_flag_errors_when_feature_disabled() {
        let client = no_redirect_client();
        let opts = FetchOptions {
            js: true,
            ..Default::default()
        };
        let result = fetch_page(&client, "https://example.com/page", opts, &TokioDnsResolver).await;

        assert!(
            matches!(&result, Err(FetchError::BrowserNotFound(msg)) if msg.contains("js-rendering")),
            "expected BrowserNotFound error with feature hint, got: {result:?}"
        );
    }
}

#[cfg(test)]
mod js_dependent_tests {
    use super::*;

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

    #[test]
    fn normal_html_not_detected() {
        let html = r#"<html><body><article>
        <h1>Title</h1><p>Long paragraph with enough content to exceed
        the threshold of one hundred characters easily.</p>
        </article></body></html>"#;
        assert!(!is_js_dependent(html));
    }

    #[test]
    fn script_without_spa_pattern_but_empty_body() {
        let html = r#"<html><head><script src="bundle.js"></script></head>
        <body><div class="app"></div></body></html>"#;
        assert!(is_js_dependent(html));
    }

    #[test]
    fn spa_pattern_without_script_but_empty_body() {
        let html = r#"<html><body><div id="root"></div></body></html>"#;
        assert!(is_js_dependent(html));
    }

    #[test]
    fn rich_body_with_scripts_not_detected() {
        let content = "x".repeat(200);
        let html = format!(
            r#"<html><head><script src="app.js"></script></head>
            <body><div id="root"><p>{content}</p></div></body></html>"#
        );
        assert!(!is_js_dependent(&html));
    }

    #[test]
    fn thin_body_without_script_or_spa_pattern_not_detected() {
        let html = "<html><body><p>short</p></body></html>";
        assert!(!is_js_dependent(html));
    }

    #[test]
    fn no_body_tag_falls_back_to_full_html() {
        let html = r#"<div id="root"></div><script src="app.js"></script>"#;
        assert!(is_js_dependent(html));
    }
}

#[cfg(test)]
mod thin_body_tests {
    use super::*;

    #[test]
    fn style_content_excluded_from_visible_text() {
        let html = "<html><body><style>.big{font-size:9999px;color:red;margin:0 auto;padding:10px 20px 30px 40px}</style><p>hi</p></body></html>";
        assert!(has_thin_body(html));
    }

    #[test]
    fn uppercase_script_tag_excluded() {
        let html = "<html><body><SCRIPT>var x = 'lots of javascript code that should be ignored by the parser';</SCRIPT><p>hi</p></body></html>";
        assert!(has_thin_body(html));
    }

    #[test]
    fn uppercase_body_tag_found() {
        let content = "x".repeat(200);
        let html = format!("<html><BODY><p>{content}</p></BODY></html>");
        assert!(!has_thin_body(&html));
    }

    #[test]
    fn exactly_at_threshold_is_not_thin() {
        let content = "x".repeat(BODY_TEXT_THRESHOLD);
        let html = format!("<html><body><p>{content}</p></body></html>");
        assert!(!has_thin_body(&html));
    }

    #[test]
    fn just_below_threshold_is_thin() {
        let content = "x".repeat(BODY_TEXT_THRESHOLD - 1);
        let html = format!("<html><body><p>{content}</p></body></html>");
        assert!(has_thin_body(&html));
    }

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
            content_html: content_html.to_string(),
            used_raw_fallback,
        }
    }

    #[test]
    fn raw_fallback_with_short_content_is_thin() {
        assert!(is_thin_extract(&article("<p>short</p>", true)));
    }

    #[test]
    fn raw_fallback_with_rich_content_still_thin() {
        let content = format!("<p>{}</p>", "x".repeat(100));
        assert!(is_thin_extract(&article(&content, true)));
    }

    #[test]
    fn short_content_is_thin() {
        assert!(is_thin_extract(&article("<p>hi</p>", false)));
    }

    #[test]
    fn sufficient_content_is_not_thin() {
        let content = format!("<p>{}</p>", "x".repeat(100));
        assert!(!is_thin_extract(&article(&content, false)));
    }

    #[test]
    fn exactly_at_threshold_is_not_thin() {
        let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD));
        assert!(!is_thin_extract(&article(&content, false)));
    }

    #[test]
    fn just_below_threshold_is_thin() {
        let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD - 1));
        assert!(is_thin_extract(&article(&content, false)));
    }

    #[test]
    fn html_tags_excluded_from_count() {
        let content = r#"<div class="very-long-class-name"><span>ab</span></div>"#;
        assert!(is_thin_extract(&article(content, false)));
    }

    #[test]
    fn whitespace_excluded_from_count() {
        let content = format!("<p>{}</p>", " x ".repeat(30));
        assert!(is_thin_extract(&article(&content, false)));
    }
}

#[cfg(test)]
mod browser_binary_tests {
    use super::*;

    #[test]
    fn t001_returns_error_when_chrome_not_found() {
        let result = resolve_browser_binary_from(&[], &[]);
        assert!(
            matches!(result, Err(BrowserError::NotFound)),
            "expected NotFound, got: {result:?}"
        );
    }

    #[test]
    fn finds_binary_at_known_path() {
        let existing = std::env::current_exe().unwrap();
        let result = resolve_browser_binary_from(&[], &[existing.as_path()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), existing);
    }
}

#[cfg(test)]
#[cfg(feature = "js-rendering")]
mod cdp_launch_tests {
    use super::*;

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
    use super::ssrf::DnsResolver;
    use super::*;
    use std::net::IpAddr;

    #[derive(Clone, Copy)]
    struct MockPrivateDns;
    impl DnsResolver for MockPrivateDns {
        async fn lookup(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, FetchError> {
            Ok(vec!["10.0.0.1".parse().unwrap()])
        }
    }

    #[derive(Clone, Copy)]
    struct MockPublicDns;
    impl DnsResolver for MockPublicDns {
        async fn lookup(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, FetchError> {
            Ok(vec!["93.184.216.34".parse().unwrap()])
        }
    }

    #[tokio::test]
    async fn t004_blocks_dns_resolving_to_private_ip() {
        let resolver = MockPrivateDns;
        assert!(
            !check_browser_request("https://evil.example/secret", &resolver).await,
            "must block when DNS resolves to private IP"
        );
    }

    #[tokio::test]
    async fn t004_blocks_internal_ip_literal() {
        let resolver = MockPublicDns;
        assert!(
            !check_browser_request("http://127.0.0.1/secret", &resolver).await,
            "must block loopback IP"
        );
    }

    #[tokio::test]
    async fn t004_allows_public_url() {
        let resolver = MockPublicDns;
        assert!(
            check_browser_request("https://example.com/page", &resolver).await,
            "must allow public URL"
        );
    }

    #[tokio::test]
    async fn t004_allows_non_network_urls() {
        let resolver = MockPublicDns;
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

    #[tokio::test]
    async fn t004_blocks_unknown_schemes() {
        let resolver = MockPublicDns;
        for url in ["file:///etc/passwd", "ftp://internal/data", "gopher://x"] {
            assert!(
                !check_browser_request(url, &resolver).await,
                "must block unknown scheme: {url}"
            );
        }
    }

    #[tokio::test]
    async fn t004_blocks_websocket_to_internal() {
        let resolver = MockPublicDns;
        assert!(
            !check_browser_request("ws://127.0.0.1:8080/ws", &resolver).await,
            "must block ws:// to loopback"
        );
        assert!(
            !check_browser_request("wss://localhost/ws", &resolver).await,
            "must block wss:// to localhost"
        );
    }

    #[tokio::test]
    async fn t004_blocks_websocket_dns_to_private() {
        let resolver = MockPrivateDns;
        assert!(
            !check_browser_request("ws://evil.example/ws", &resolver).await,
            "must block ws:// when DNS resolves to private IP"
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

    #[tokio::test]
    async fn t005_cdp_renders_public_url() {
        if !chrome_available() {
            eprintln!("SKIP: Chrome not found");
            return;
        }
        let html = fetch_with_cdp("https://example.com", TokioDnsResolver)
            .await
            .expect("fetch_with_cdp should succeed for public URL");
        assert!(
            html.contains("Example Domain") || html.contains("example"),
            "rendered HTML should contain page content, got {} bytes",
            html.len()
        );
    }
}
