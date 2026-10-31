use super::ssrf::StaticDnsResolver;
use super::*;

fn private_dns() -> StaticDnsResolver {
    StaticDnsResolver::single("10.0.0.1")
}

/// [T-F044]
#[tokio::test]
async fn blocks_host_resolving_to_private_ip() {
    let resolver = private_dns();
    assert!(
        !check_browser_request("https://evil.example/secret", &resolver).await,
        "must block when DNS resolves to private IP"
    );
}

/// [T-F045]
#[tokio::test]
async fn blocks_loopback_ip_literal() {
    let resolver = StaticDnsResolver::public();
    assert!(
        !check_browser_request("http://127.0.0.1/secret", &resolver).await,
        "must block loopback IP"
    );
}

/// [T-F046]
#[tokio::test]
async fn allows_host_resolving_to_public_ip() {
    let resolver = StaticDnsResolver::public();
    assert!(
        check_browser_request("https://example.com/page", &resolver).await,
        "must allow public URL"
    );
}

/// [T-F047]
#[tokio::test]
async fn allows_non_network_schemes() {
    let resolver = StaticDnsResolver::public();
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

/// [T-F048]
#[tokio::test]
async fn blocks_unknown_schemes() {
    let resolver = StaticDnsResolver::public();
    for url in ["file:///etc/passwd", "ftp://internal/data", "gopher://x"] {
        assert!(
            !check_browser_request(url, &resolver).await,
            "must block unknown scheme: {url}"
        );
    }
}

/// [T-F049]
#[tokio::test]
async fn blocks_websocket_to_loopback_host() {
    let resolver = StaticDnsResolver::public();
    assert!(
        !check_browser_request("ws://127.0.0.1:8080/ws", &resolver).await,
        "must block ws:// to loopback"
    );
    assert!(
        !check_browser_request("wss://localhost/ws", &resolver).await,
        "must block wss:// to localhost"
    );
}

/// [T-F050]
#[tokio::test]
async fn blocks_websocket_host_resolving_to_private_ip() {
    let resolver = private_dns();
    assert!(
        !check_browser_request("ws://evil.example/ws", &resolver).await,
        "must block ws:// when DNS resolves to private IP"
    );
}
