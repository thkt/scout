//! SSRF defense-in-depth: URL validation and DNS pre-check.

use std::borrow::Cow;
use std::net::{IpAddr, Ipv6Addr};

use tracing::warn;

use super::FetchError;

const DNS_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// DNS resolver abstraction for SSRF defense. Enables mock-based testing
/// of the DNS-resolves-to-private-IP path without real network lookups.
pub(crate) trait DnsResolver {
    async fn lookup(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, FetchError>;
}

/// Production DNS resolver using tokio's async DNS lookup.
pub(crate) struct TokioDnsResolver;

impl DnsResolver for TokioDnsResolver {
    async fn lookup(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, FetchError> {
        let addrs = tokio::time::timeout(
            DNS_LOOKUP_TIMEOUT,
            tokio::net::lookup_host(format!("{host}:{port}")),
        )
        .await
        .map_err(|_| FetchError::DnsResolution("DNS lookup timed out".to_string()))?
        .map_err(|e| FetchError::DnsResolution(e.to_string()))?;
        Ok(addrs.map(|a| a.ip()).collect())
    }
}

/// Strip userinfo (username:password) from URLs before logging (SEC-003).
pub(super) fn redact_url_credentials(raw: &str) -> Cow<'_, str> {
    if !raw.contains('@') {
        return Cow::Borrowed(raw);
    }
    if let Ok(mut parsed) = url::Url::parse(raw)
        && (!parsed.username().is_empty() || parsed.password().is_some())
    {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
        return Cow::Owned(parsed.to_string());
    }
    Cow::Borrowed(raw)
}

pub(super) async fn ssrf_check(
    raw: &str,
    resolver: &impl DnsResolver,
) -> Result<(), FetchError> {
    let parsed = validate_url_sync(raw).map_err(|e| {
        if matches!(e, FetchError::InternalHost) {
            warn!(url = %redact_url_credentials(raw), "blocked fetch to internal/private host");
        }
        e
    })?;

    if let Some(url::Host::Domain(domain)) = parsed.host() {
        let port = parsed
            .port()
            .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
        let addrs = resolver.lookup(domain, port).await?;

        for ip in addrs {
            if is_private_ip(ip) {
                warn!(host = %domain, ip = %ip, "DNS resolves to private IP");
                return Err(FetchError::InternalHost);
            }
        }
    }

    Ok(())
}

fn validate_url_sync(raw: &str) -> Result<url::Url, FetchError> {
    let parsed = url::Url::parse(raw)?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(FetchError::InvalidScheme),
    }
    if is_blocked_host(&parsed) {
        return Err(FetchError::InternalHost);
    }
    Ok(parsed)
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
                || lower.ends_with(".arpa")
        }
        None => true,
    }
}

fn is_cgn(v4: std::net::Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.octets()[0] == 0
                || v4.is_broadcast()
                || is_cgn(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_link_local(&v6)
                || is_ipv6_unique_local(&v6)
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_private_ip(IpAddr::V4(v4)))
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
    fn validate_url_accepts_valid() {
        for url in [
            "http://example.com",
            "https://example.com",
            "https://8.8.8.8/dns",
            "http://[2001:db8::1]/page",
        ] {
            assert!(
                validate_url_sync(url).map(|_| ()).is_ok(),
                "should accept: {url}"
            );
        }
    }

    #[test]
    fn validate_url_rejects_bad_scheme() {
        for url in ["ftp://example.com", "file:///tmp/test", "not-a-url"] {
            assert!(
                validate_url_sync(url).map(|_| ()).is_err(),
                "should reject: {url}"
            );
        }
    }

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
                matches!(
                    validate_url_sync(url).map(|_| ()),
                    Err(FetchError::InternalHost)
                ),
                "should block as InternalHost: {url}"
            );
        }
    }
}

#[cfg(test)]
mod dns_tests {
    use super::*;

    struct AllowDns(Vec<IpAddr>);

    impl DnsResolver for AllowDns {
        async fn lookup(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, FetchError> {
            Ok(self.0.clone())
        }
    }

    struct FailDns(String);

    impl DnsResolver for FailDns {
        async fn lookup(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, FetchError> {
            Err(FetchError::DnsResolution(self.0.clone()))
        }
    }

    #[tokio::test]
    async fn ssrf_blocks_dns_resolving_to_private_ip() {
        let resolver = AllowDns(vec!["127.0.0.1".parse().unwrap()]);
        let result = ssrf_check("https://evil.com/secret", &resolver).await;
        assert!(matches!(result, Err(FetchError::InternalHost)));
    }

    #[tokio::test]
    async fn ssrf_allows_dns_resolving_to_public_ip() {
        let resolver = AllowDns(vec!["8.8.8.8".parse().unwrap()]);
        let result = ssrf_check("https://example.com/page", &resolver).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ssrf_returns_error_on_dns_failure() {
        let resolver = FailDns("lookup failed".into());
        let result = ssrf_check("https://example.com/page", &resolver).await;
        assert!(matches!(result, Err(FetchError::DnsResolution(_))));
    }

    #[tokio::test]
    async fn ssrf_skips_dns_for_ip_literals() {
        let resolver = AllowDns(vec![]);
        let result = ssrf_check("https://8.8.8.8/page", &resolver).await;
        assert!(result.is_ok());
    }

    #[test]
    fn redact_strips_userinfo() {
        let url = "https://user:password@example.com/path";
        let safe = redact_url_credentials(url);
        assert!(!safe.contains("user"));
        assert!(!safe.contains("password"));
        assert!(safe.contains("example.com/path"));
    }

    #[test]
    fn redact_preserves_clean_url() {
        let url = "https://example.com/path";
        assert!(matches!(redact_url_credentials(url), Cow::Borrowed(_)));
    }

    #[test]
    fn redact_handles_username_only() {
        let url = "https://admin@example.com/";
        let safe = redact_url_credentials(url);
        assert!(!safe.contains("admin"));
        assert!(safe.contains("example.com"));
    }
}
