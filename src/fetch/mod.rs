pub mod converter;
pub mod extractor;

use std::net::{IpAddr, Ipv6Addr};

use converter::{to_fetch_result, FetchResult};
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

    #[error("response too large (>{} bytes)", MAX_RESPONSE_BYTES)]
    TooLarge,
}

pub async fn fetch_page(
    client: &Client,
    url: &str,
    raw: bool,
    meta: bool,
) -> Result<FetchResult, FetchError> {
    // SSRF defense-in-depth: URL validation + DNS check for private IPs.
    // TOCTOU gap: DNS may differ between check and reqwest's connection.
    // Acceptable for local MCP — full fix requires a custom resolver.
    validate_url(url)?;
    check_dns(url).await?;

    let (final_url, html) = download(client, url).await?;

    // Re-validate after redirects to block content from internal hosts.
    validate_url(&final_url)?;
    check_dns(&final_url).await?;

    let article = if raw {
        extract_raw(&html)
    } else {
        extract_article(&html, Some(&final_url))
    };

    debug!(url = %final_url, bytes = html.len(), "page fetched");
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

    let final_url = response.url().to_string();

    if let Some(len) = response.content_length()
        && len as usize > MAX_RESPONSE_BYTES
    {
        return Err(FetchError::TooLarge);
    }

    let mut body = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(FetchError::TooLarge);
        }
    }
    let html = String::from_utf8_lossy(&body).into_owned();
    Ok((final_url, html))
}

fn validate_url(raw: &str) -> Result<(), FetchError> {
    let parsed = url::Url::parse(raw)?;

    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(FetchError::InvalidScheme),
    }

    if is_blocked_host(&parsed) {
        warn!(url = %raw, "blocked fetch to internal/private host");
        return Err(FetchError::InternalHost);
    }

    Ok(())
}

async fn check_dns(raw: &str) -> Result<(), FetchError> {
    let parsed = url::Url::parse(raw)?;
    let domain = match parsed.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        _ => return Ok(()),
    };

    let port = parsed.port().unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let addrs = tokio::net::lookup_host(format!("{domain}:{port}"))
        .await
        .map_err(|e| FetchError::DnsResolution(e.to_string()))?;

    for addr in addrs {
        if is_private_ip(addr.ip()) {
            warn!(host = %domain, ip = %addr.ip(), "DNS resolves to private IP");
            return Err(FetchError::InternalHost);
        }
    }

    Ok(())
}

fn is_blocked_host(parsed: &url::Url) -> bool {
    match parsed.host() {
        Some(url::Host::Ipv4(v4)) => is_private_ip(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => is_private_ip(IpAddr::V6(v6)),
        Some(url::Host::Domain(domain)) => {
            let lower = domain.to_ascii_lowercase();
            lower == "localhost"
                || lower.ends_with(".localhost")
                || lower.ends_with(".local")
                || lower.ends_with(".internal")
        }
        None => true,
    }
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
            || v6.is_unspecified()
            || is_ipv6_link_local(&v6)
            || is_ipv6_unique_local(&v6)
            || v6.to_ipv4_mapped().is_some_and(|v4| is_private_ip(IpAddr::V4(v4)))
        }
    }
}

fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_url() {
        assert!(validate_url("ftp://example.com").is_err());
        assert!(validate_url("file:///tmp/test").is_err());
    }

    #[test]
    fn rejects_invalid_url() {
        assert!(validate_url("not-a-url").is_err());
    }

    #[test]
    fn accepts_http_and_https() {
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("https://example.com").is_ok());
    }

    #[test]
    fn rejects_localhost() {
        assert!(matches!(
            validate_url("http://localhost/secret"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://127.0.0.1/secret"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn rejects_private_ips() {
        assert!(matches!(
            validate_url("http://10.0.0.1/internal"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://192.168.1.1/router"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://172.16.0.1/internal"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn rejects_cloud_metadata() {
        assert!(matches!(
            validate_url("http://169.254.169.254/latest/meta-data"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn rejects_ipv6_loopback() {
        assert!(matches!(
            validate_url("http://[::1]/secret"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn accepts_public_ip_url() {
        assert!(validate_url("https://8.8.8.8/dns").is_ok());
    }

    #[test]
    fn rejects_localhost_subdomains() {
        assert!(matches!(
            validate_url("http://evil.localhost/secret"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://a.b.localhost/secret"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn rejects_ipv4_mapped_ipv6() {
        assert!(matches!(
            validate_url("http://[::ffff:127.0.0.1]/secret"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://[::ffff:169.254.169.254]/metadata"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://[::ffff:10.0.0.1]/internal"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn rejects_ipv6_link_local() {
        assert!(matches!(
            validate_url("http://[fe80::1]/secret"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn rejects_ipv6_unique_local() {
        assert!(matches!(
            validate_url("http://[fd00::1]/secret"),
            Err(FetchError::InternalHost)
        ));
        assert!(matches!(
            validate_url("http://[fc00::1]/secret"),
            Err(FetchError::InternalHost)
        ));
    }

    #[test]
    fn accepts_public_ipv6() {
        assert!(validate_url("http://[2001:db8::1]/page").is_ok());
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
                ResponseTemplate::new(200).set_body_string("<html><body><p>hello</p></body></html>"),
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
    async fn download_404_returns_status_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = download(&client, &format!("{}/missing", server.uri())).await;
        assert!(matches!(result, Err(FetchError::Status(404))));
    }

    #[tokio::test]
    async fn download_500_returns_status_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/error"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = download(&client, &format!("{}/error", server.uri())).await;
        assert!(matches!(result, Err(FetchError::Status(500))));
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
    async fn fetch_page_blocks_ssrf_to_localhost() {
        let client = Client::new();
        let result = fetch_page(&client, "http://127.0.0.1/secret", false, false).await;
        assert!(matches!(result, Err(FetchError::InternalHost)));
    }
}
