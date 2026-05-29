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
