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
use tracing::{debug, warn};

const MAX_RESPONSE_BYTES: usize = 10_000_000;

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
}

/// Fetch a web page and extract its content.
///
/// Includes SSRF defense (URL validation + DNS check + post-redirect recheck).
/// - `raw`: skip Readability extraction, return full HTML converted to Markdown
/// - `meta`: include YAML frontmatter (title, author, date)
pub async fn fetch_page(
    client: &Client,
    url: &str,
    raw: bool,
    meta: bool,
    resolver: &impl DnsResolver,
) -> Result<FetchResult, FetchError> {
    // SSRF defense-in-depth: URL validation + DNS check for private IPs.
    // TOCTOU gap: DNS may differ between check and reqwest's connection.
    // Acceptable for local MCP — full fix requires a custom resolver.
    //
    // SECURITY ASSUMPTION: This server runs over local stdio transport only.
    // If exposed over network (SSE/WebSocket), implement a custom DNS resolver
    // that enforces the IP allowlist at connect time, and add per-tool rate limiting.
    ssrf_check(url, resolver).await?;

    let (final_url, html) = download(client, url).await?;

    // Re-validate after redirects to block content from internal hosts.
    ssrf_check(&final_url, resolver).await?;

    let article = if raw {
        extract_raw(&html)
    } else {
        extract_article(&html, Some(&final_url))
    };

    debug!(url = %redact_url_credentials(&final_url), bytes = html.len(), "page fetched");
    Ok(to_fetch_result(article, final_url, meta))
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
    async fn download_extracts_readability_content() {
        let html = r#"
            <html><head><title>Test</title></head>
            <body><article>
                <h1>Article Title</h1>
                <p>Paragraph one with enough text for readability to consider it real content.</p>
                <p>Paragraph two with more text to make it sufficiently long and article-like.</p>
                <p>Paragraph three continues adding content so the extraction works properly.</p>
            </article></body></html>"#;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;

        let client = Client::new();
        let (_, body) = download(&client, &format!("{}/article", server.uri()))
            .await
            .unwrap();

        assert!(body.contains("Article Title"));
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

    #[tokio::test]
    async fn fetch_page_blocks_ssrf_to_localhost() {
        let client = Client::new();
        let result = fetch_page(
            &client,
            "http://127.0.0.1/secret",
            false,
            false,
            &TokioDnsResolver,
        )
        .await;
        assert!(matches!(result, Err(FetchError::InternalHost)));
    }
}
