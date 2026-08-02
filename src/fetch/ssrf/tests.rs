use super::*;

/// [T-FS001]
#[test]
fn validate_url_accepts_valid() {
    for url in [
        "http://example.com",
        "https://example.com",
        "https://8.8.8.8/dns",
        "http://[2001:db8::1]/page",
    ] {
        assert!(validate_url_sync(url).is_ok(), "should accept: {url}");
    }
}

/// [T-FS002]
#[test]
fn validate_url_rejects_bad_scheme() {
    for url in ["ftp://example.com", "file:///tmp/test", "not-a-url"] {
        assert!(validate_url_sync(url).is_err(), "should reject: {url}");
    }
}

/// [T-FS003] validate_url_rejects_internal_hosts
#[test]
fn validate_url_rejects_internal_hosts() {
    for url in [
        "http://localhost/secret",
        "http://127.0.0.1/secret",
        "http://10.0.0.1/internal",
        "http://192.168.1.1/router",
        "http://172.16.0.1/internal",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]/secret",
        "http://evil.localhost/secret",
        "http://a.b.localhost/secret",
        "http://[::ffff:127.0.0.1]/secret",
        "http://[::ffff:169.254.169.254]/metadata",
        "http://[::ffff:10.0.0.1]/internal",
        "http://[::7f00:1]/secret", // IPv4-compatible ::127.0.0.1 (SEC-001)
        "http://[::a9fe:a9fe]/metadata", // IPv4-compatible ::169.254.169.254
        "http://[fe80::1]/secret",
        "http://[fd00::1]/secret",
        "http://[fc00::1]/secret",
        "http://100.64.0.1/internal",
        "http://100.127.255.254/cgn",
        "http://0.1.0.0/test",
        "http://0.255.255.255/test",
        "http://evil.in-addr.arpa/ptr",
        "http://test.home.arpa/local",
        // FQDN form: `url` keeps the trailing dot in the host, so a suffix
        // comparison against the raw domain misses these.
        "http://localhost./secret",
        "http://evil.localhost./secret",
        "http://svc.internal./config",
        "http://printer.local./status",
        "http://evil.in-addr.arpa./ptr",
    ] {
        assert!(
            matches!(validate_url_sync(url), Err(FetchError::InternalHost)),
            "should block as InternalHost: {url}"
        );
    }
}

/// [T-FS012] validate_url_allows_cgn_boundary_neighbors
///
/// CGN block is 100.64.0.0/10 (100.64.0.0–100.127.255.255). T-FS003 covers
/// the inside of the range; this test guards the fence-post on both ends so
/// a one-octet shift in `is_cgn` cannot pass silently. The neighbors are
/// public IPs and must be allowed by `validate_url_sync` (DNS-free pre-check).
#[test]
fn validate_url_allows_cgn_boundary_neighbors() {
    for url in [
        "http://100.63.255.255/", // one below CGN start (100.64.0.0)
        "http://100.128.0.0/",    // one above CGN end (100.127.255.255)
    ] {
        assert!(
            validate_url_sync(url).is_ok(),
            "should allow public IP outside CGN: {url}"
        );
    }
}

/// [T-FS018]
#[tokio::test]
async fn proxied_mode_validates_a_public_domain_url_without_consulting_the_dns_resolver_failingdnsresolver_still_returns_ok()
 {
    let resolver = FailingDnsResolver("lookup failed".into());
    let result = ssrf_check(
        "https://example.com/page",
        &resolver,
        &EgressMode::Proxied("http://proxy.example:8080".to_owned()),
    )
    .await;
    assert!(
        result.is_ok(),
        "proxied mode must skip the DNS pre-check and validate the public domain, got: {result:?}"
    );
}

/// [T-FS019] proxied mode rejects a literal private-IP URL with InternalHost
#[tokio::test]
async fn proxied_mode_rejects_a_literal_private_ip_url_with_internalhost() {
    let resolver = FailingDnsResolver("lookup failed".into());
    let result = ssrf_check(
        "http://127.0.0.1/secret",
        &resolver,
        &EgressMode::Proxied("http://proxy.example:8080".to_owned()),
    )
    .await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "proxied mode must still reject a literal private IP, got: {result:?}"
    );
}

/// [T-FS020]
#[tokio::test]
async fn proxied_mode_rejects_a_localhost_domain_url_with_internalhost() {
    let resolver = FailingDnsResolver("lookup failed".into());
    let result = ssrf_check(
        "http://localhost/secret",
        &resolver,
        &EgressMode::Proxied("http://proxy.example:8080".to_owned()),
    )
    .await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "proxied mode must still reject a localhost domain, got: {result:?}"
    );
}

/// [T-FS021]
#[tokio::test]
async fn direct_mode_still_returns_dnsresolution_when_the_resolver_fails_pre_check_behavior_unchanged()
 {
    let resolver = FailingDnsResolver("lookup failed".into());
    let result = ssrf_check("https://example.com/page", &resolver, &EgressMode::Direct).await;
    assert!(
        matches!(result, Err(FetchError::DnsResolution(_))),
        "direct mode must still run the DNS pre-check, got: {result:?}"
    );
}
