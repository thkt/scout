use super::*;

/// [T-FS001] validate_url_accepts_valid
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

/// [T-FS002] validate_url_rejects_bad_scheme
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
        "http://[fe80::1]/secret",
        "http://[fd00::1]/secret",
        "http://[fc00::1]/secret",
        "http://100.64.0.1/internal",
        "http://100.127.255.254/cgn",
        "http://0.1.0.0/test",
        "http://0.255.255.255/test",
        "http://evil.in-addr.arpa/ptr",
        "http://test.home.arpa/local",
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
