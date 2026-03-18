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
use std::time::Duration;
use tracing::{debug, info, warn};

/// Options for [`fetch_page`] that control rendering and output.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchOptions {
    /// Force JS rendering via playwright-cli (skip auto-detection).
    pub js: bool,
    /// Skip Readability extraction; return full HTML converted to Markdown.
    pub raw: bool,
}

const MAX_RESPONSE_BYTES: usize = 10_000_000;

const PLAYWRIGHT_TIMEOUT: Duration = Duration::from_secs(60);

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

    #[error("playwright rendering failed: {0}")]
    Playwright(String),
}

/// Minimum extracted text length to consider Readability extraction successful.
const EXTRACT_TEXT_THRESHOLD: usize = 50;

/// Fetch a web page and extract its content.
///
/// Includes SSRF defense (URL validation + DNS check + post-redirect recheck).
/// Unless [`FetchOptions::js`] is set, automatically falls back to playwright-cli
/// for JS rendering when the page appears JS-dependent (SPA with empty body)
/// or when Readability extraction yields too little content.
pub async fn fetch_page(
    client: &Client,
    url: &str,
    opts: FetchOptions,
    resolver: &impl DnsResolver,
) -> Result<FetchResult, FetchError> {
    // SECURITY: Local CLI only. TOCTOU gap between DNS check and reqwest connect
    // is acceptable here; a network service would need a custom resolver that
    // enforces the allowlist at connect time. Playwright widens the gap further
    // (its own DNS resolution) — proxy or disable it in service mode.
    ssrf_check(url, resolver).await?;

    let (final_url, mut html) = download(client, url).await?;

    ssrf_check(&final_url, resolver).await?;

    let need_js = if opts.js {
        info!("--js flag set, using playwright-cli for JS rendering");
        true
    } else if is_js_dependent(&html) {
        warn!("JS-dependent page detected, trying playwright-cli fallback");
        true
    } else {
        false
    };

    if need_js {
        match fetch_with_playwright(&final_url).await {
            Ok(js_html) => {
                debug!("playwright succeeded");
                html = js_html;
            }
            Err(e) if opts.js => {
                return Err(FetchError::Playwright(e.to_string()));
            }
            Err(e) => {
                warn!(error = %e, "playwright fallback failed, using original HTML");
            }
        }
    }

    let article = if opts.raw {
        extract_raw(&html)
    } else {
        extract_article(&html, Some(&final_url))
    };

    let article = if !opts.raw && !need_js && is_thin_extract(&article) {
        warn!(url = %redact_url_credentials(&final_url), "extraction yielded too little content, trying playwright-cli fallback");
        match fetch_with_playwright(&final_url).await {
            Ok(js_html) => {
                let re_extracted = extract_article(&js_html, Some(&final_url));
                if is_thin_extract(&re_extracted) {
                    debug!(url = %redact_url_credentials(&final_url), "playwright re-extraction still thin, returning best-effort result");
                } else {
                    debug!(url = %redact_url_credentials(&final_url), "playwright fallback succeeded (post-extraction)");
                }
                re_extracted
            }
            Err(e) => {
                warn!(url = %redact_url_credentials(&final_url), error = %e, "playwright fallback failed, using original extraction");
                article
            }
        }
    } else {
        article
    };

    debug!(url = %redact_url_credentials(&final_url), bytes = html.len(), "page fetched");
    Ok(to_fetch_result(article, final_url))
}

/// Check whether the extracted article has too little visible text.
///
/// Raw fallback is always thin: shell text (nav, footer) inflates the count
/// but the actual article body is missing. ~50 visible chars ≈ one sentence;
/// pages below this almost always need JS rendering.
fn is_thin_extract(article: &extractor::ExtractedArticle) -> bool {
    article.used_raw_fallback
        || visible_text_len(&article.content_html, EXTRACT_TEXT_THRESHOLD) < EXTRACT_TEXT_THRESHOLD
}

/// Count non-whitespace characters outside HTML tags. Short-circuits at `limit`.
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

/// Skips `<script>`/`<style>` content; short-circuits at [`BODY_TEXT_THRESHOLD`].
fn has_thin_body(html: &str) -> bool {
    let lower = html.as_bytes();
    let body_start = lower
        .windows(5)
        .position(|w| w.eq_ignore_ascii_case(b"<body"));
    let body = if let Some(start) = body_start {
        let after_tag = html[start..].find('>').map(|i| start + i + 1).unwrap_or(start);
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
                } else if name.eq_ignore_ascii_case(b"/script") || name.eq_ignore_ascii_case(b"/style") {
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

#[derive(Debug, thiserror::Error)]
enum PlaywrightError {
    #[error("playwright-cli not installed")]
    NotInstalled,
    #[error("playwright-cli timed out after {0}s")]
    Timeout(u64),
    #[error("playwright-cli failed: {0}")]
    ProcessFailed(String),
}

async fn drain_pipe<R: tokio::io::AsyncRead + Unpin>(mut pipe: R, limit: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match tokio::io::AsyncReadExt::read(&mut pipe, &mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > limit {
                    buf.truncate(limit);
                    break;
                }
            }
        }
    }
    buf
}

async fn resolve_playwright_cli() -> Result<String, PlaywrightError> {
    for bin in ["playwright-cli", "npx"] {
        let ok = tokio::process::Command::new("which")
            .arg(bin)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Ok(if bin == "npx" {
                "npx @playwright/cli".to_string()
            } else {
                bin.to_string()
            });
        }
    }
    Err(PlaywrightError::NotInstalled)
}

async fn fetch_with_playwright(url: &str) -> Result<String, PlaywrightError> {
    let cli = resolve_playwright_cli().await?;

    let escaped_url = shell_escape::escape(url.into());
    let cmd = format!(
        r#"{cli} open {escaped_url} && {cli} run-code "async page => {{ return await page.content(); }}" && {cli} close"#
    );

    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| PlaywrightError::ProcessFailed(e.to_string()))?;

    // Drain pipes concurrently to avoid deadlock from full pipe buffers.
    let stdout_pipe = child.stdout.take().unwrap();
    let stderr_pipe = child.stderr.take().unwrap();
    const MAX_STDERR_BYTES: usize = 65_536;
    let stdout_drain = tokio::spawn(drain_pipe(stdout_pipe, MAX_RESPONSE_BYTES));
    let stderr_drain = tokio::spawn(drain_pipe(stderr_pipe, MAX_STDERR_BYTES));

    // wait() borrows &mut self, so child remains available for kill() on timeout.
    match tokio::time::timeout(PLAYWRIGHT_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => {
            if !status.success() {
                stdout_drain.abort();
                let stderr_buf = stderr_drain.await.unwrap_or_default();
                let stderr = String::from_utf8_lossy(&stderr_buf);
                return Err(PlaywrightError::ProcessFailed(stderr.into_owned()));
            }
            let stdout_buf = stdout_drain.await.unwrap_or_default();
            stderr_drain.abort();
            let stdout = String::from_utf8_lossy(&stdout_buf);
            parse_playwright_output(&stdout)
        }
        Ok(Err(e)) => {
            stdout_drain.abort();
            stderr_drain.abort();
            Err(PlaywrightError::ProcessFailed(e.to_string()))
        }
        Err(_) => {
            let _ = child.start_kill();
            stdout_drain.abort();
            stderr_drain.abort();
            Err(PlaywrightError::Timeout(PLAYWRIGHT_TIMEOUT.as_secs()))
        }
    }
}

fn parse_playwright_output(stdout: &str) -> Result<String, PlaywrightError> {
    // playwright-cli run-code outputs: ### Result\n"<html>..."
    let result_marker = "### Result\n";
    let after_marker = stdout
        .find(result_marker)
        .map(|i| &stdout[i + result_marker.len()..])
        .unwrap_or(stdout);

    let trimmed = after_marker.trim();

    if trimmed.starts_with('"') {
        let json_str = if let Some(end) = trimmed.find("\n###") {
            &trimmed[..end]
        } else {
            trimmed
        };
        if let Ok(html) = serde_json::from_str::<String>(json_str.trim()) {
            return Ok(html);
        }
    }

    if trimmed.contains("<html") || trimmed.contains("<body") {
        return Ok(trimmed.to_string());
    }

    Err(PlaywrightError::ProcessFailed(
        "could not parse playwright output".to_string(),
    ))
}

async fn download(client: &Client, url: &str) -> Result<(String, String), FetchError> {
    let response = client
        .get(url)
        .header("User-Agent", crate::USER_AGENT)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Status(status.as_u16()));
    }

    let mut charset = None;
    match response.headers().get("content-type") {
        None => {
            debug!(url = %redact_url_credentials(url), "no Content-Type header, proceeding as text")
        }
        Some(ct) => match ct.to_str() {
            Ok(ct_str) => {
                check_content_type(ct_str)?;
                charset = extract_charset(ct_str);
            }
            Err(_) => {
                debug!(url = %redact_url_credentials(url), "Content-Type header is not valid ASCII, proceeding as text")
            }
        },
    }

    let final_url = response.url().to_string();

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
    Ok((final_url, html))
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
        && mime != "application/json"
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
    fn accepts_text_html() {
        assert!(check_content_type("text/html; charset=utf-8").is_ok());
    }

    #[test]
    fn accepts_text_plain() {
        assert!(check_content_type("text/plain").is_ok());
    }

    #[test]
    fn accepts_xhtml() {
        assert!(check_content_type("application/xhtml+xml").is_ok());
    }

    #[test]
    fn accepts_xml() {
        assert!(check_content_type("application/xml").is_ok());
    }

    #[test]
    fn accepts_json() {
        assert!(check_content_type("application/json").is_ok());
    }

    #[test]
    fn rejects_pdf() {
        assert!(matches!(
            check_content_type("application/pdf"),
            Err(FetchError::UnsupportedContentType(ref m)) if m == "application/pdf"
        ));
    }

    #[test]
    fn rejects_image() {
        assert!(matches!(
            check_content_type("image/png"),
            Err(FetchError::UnsupportedContentType(_))
        ));
    }

    #[test]
    fn accepts_empty_mime_before_semicolon() {
        // Edge case: "; charset=utf-8" → empty mime → allowed (permissive)
        assert!(check_content_type("; charset=utf-8").is_ok());
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn download_success_returns_html() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body><p>hello</p></body></html>"),
            )
            .mount(&server)
            .await;

        let client = Client::new();
        let (final_url, html) = download(&client, &format!("{}/page", server.uri()))
            .await
            .unwrap();

        assert!(final_url.contains("/page"));
        assert!(html.contains("hello"));
    }

    #[tokio::test]
    async fn download_non_success_returns_status_error() {
        let server = MockServer::start().await;
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

        let client = Client::new();
        assert!(matches!(
            download(&client, &format!("{}/404", server.uri())).await,
            Err(FetchError::Status(404))
        ));
        assert!(matches!(
            download(&client, &format!("{}/500", server.uri())).await,
            Err(FetchError::Status(500))
        ));
    }

    #[tokio::test]
    async fn download_too_large_body_rejected() {
        let oversized = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/huge"))
            .respond_with(ResponseTemplate::new(200).set_body_string(oversized))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = download(&client, &format!("{}/huge", server.uri())).await;
        assert!(matches!(result, Err(FetchError::TooLarge)));
    }

    #[tokio::test]
    async fn download_rejects_non_html_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/binary"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(b"fake pdf".to_vec()),
            )
            .mount(&server)
            .await;

        let client = Client::new();
        let result = download(&client, &format!("{}/binary", server.uri())).await;
        assert!(
            matches!(result, Err(FetchError::UnsupportedContentType(ref ct)) if ct == "application/pdf"),
            "got: {result:?}"
        );
    }

    #[tokio::test]
    async fn download_accepts_text_html_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .set_body_string("<html><body>ok</body></html>"),
            )
            .mount(&server)
            .await;

        let client = Client::new();
        let (_, html) = download(&client, &format!("{}/html", server.uri()))
            .await
            .unwrap();
        assert!(html.contains("ok"));
    }

}

#[cfg(test)]
mod fetch_page_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn blocks_ssrf_to_localhost() {
        let client = Client::new();
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
    async fn js_flag_attempts_playwright_on_rich_body() {
        // Serve a page with enough visible text that auto-detection would NOT trigger.
        let content = "x".repeat(200);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rich"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body><p>{content}</p></body></html>")),
            )
            .mount(&server)
            .await;

        let client = Client::new();
        let opts = FetchOptions { js: true, ..Default::default() };
        let result = fetch_page(
            &client,
            &format!("{}/rich", server.uri()),
            opts,
            &TokioDnsResolver,
        )
        .await;

        // playwright-cli is likely not installed in CI — the --js path should
        // return an error rather than silently falling back.
        assert!(result.is_err(), "js=true should error when playwright unavailable");
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
        // 100 bytes of ASCII = 100 visible bytes = threshold reached
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
        assert!(has_thin_body(&html));
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
        // Readability gave up → raw HTML has shell text but no article body.
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
        // Many tags but only 2 visible chars
        let content = r#"<div class="very-long-class-name"><span>ab</span></div>"#;
        assert!(is_thin_extract(&article(content, false)));
    }

    #[test]
    fn whitespace_excluded_from_count() {
        let content = format!("<p>{}</p>", " x ".repeat(30));
        // 30 non-whitespace chars < threshold
        assert!(is_thin_extract(&article(&content, false)));
    }
}

#[cfg(test)]
mod playwright_output_tests {
    use super::*;

    #[test]
    fn parses_playwright_html_output() {
        let stdout = "### Result\n\"<html><body><p>rendered</p></body></html>\"\n### Ran Playwright code\n```js\n...\n```";
        let html = parse_playwright_output(stdout).unwrap();
        assert!(html.contains("rendered"));
    }

    #[test]
    fn parses_output_without_result_marker() {
        let stdout = "\"<html><body>hello</body></html>\"";
        let html = parse_playwright_output(stdout).unwrap();
        assert!(html.contains("hello"));
    }

    #[test]
    fn parses_raw_html_fallback() {
        let stdout = "<html><body><p>direct html</p></body></html>";
        let html = parse_playwright_output(stdout).unwrap();
        assert!(html.contains("direct html"));
    }

    #[test]
    fn malformed_json_falls_back_to_html_check() {
        let stdout = "\"not valid json <html><body>fallback</body></html>";
        let html = parse_playwright_output(stdout).unwrap();
        assert!(html.contains("fallback"));
    }

    #[test]
    fn rejects_unparseable_output() {
        let stdout = "some random text without html";
        assert!(parse_playwright_output(stdout).is_err());
    }
}
