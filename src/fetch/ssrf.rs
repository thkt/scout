//! SSRF defense-in-depth: URL validation and DNS pre-check.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::time::Duration;

use tokio::net::lookup_host;
use tokio::time::timeout;
use tracing::warn;

use super::FetchError;

const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Object-safe boxed future returned by [`DnsResolver::lookup`].
pub(crate) type DnsLookupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, FetchError>> + Send + 'a>>;

/// Resolves a host to one or more IP addresses. `Send + Sync` so implementations
/// can sit behind an `Arc<dyn DnsResolver>` shared across async tasks.
pub(crate) trait DnsResolver: Send + Sync {
    fn lookup(&self, host: &str, port: u16) -> DnsLookupFuture<'_>;
}

/// Production resolver: `tokio::net::lookup_host` with a 5s timeout.
pub(crate) struct TokioDnsResolver;

impl DnsResolver for TokioDnsResolver {
    fn lookup(&self, host: &str, port: u16) -> DnsLookupFuture<'_> {
        let target = format!("{host}:{port}");
        Box::pin(async move {
            let addrs = timeout(DNS_LOOKUP_TIMEOUT, lookup_host(target))
                .await
                .map_err(|_| FetchError::DnsResolution("DNS lookup timed out".to_owned()))?
                .map_err(|e| FetchError::DnsResolution(e.to_string()))?;
            Ok(addrs.map(|a| a.ip()).collect())
        })
    }
}

/// SEC-003: strip userinfo before logging.
fn redact_url_credentials(raw: &str) -> Cow<'_, str> {
    if !raw.contains('@') {
        return Cow::Borrowed(raw);
    }
    // Fail closed: a '@'-bearing URL that `url` cannot parse (e.g.
    // `file://user:pass@host`, rejected with IdnaError) would otherwise be
    // logged with credentials intact.
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return Cow::Borrowed("[redacted-url]");
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        // Defense in depth: set_* fails when the parsed URL has no settable
        // host (non-special scheme, empty host); the `file:` case is already
        // caught by the parse Err arm above. No reachable post-parse input is
        // known, but fail closed rather than risk emitting userinfo.
        if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
            return Cow::Borrowed("[redacted-url]");
        }
        return Cow::Owned(parsed.to_string());
    }
    Cow::Borrowed(raw)
}

/// Display wrapper that redacts URL credentials on each format.
///
/// Centralizes redaction so every `info!`/`warn!` that logs a URL flows through
/// the same code path. Constructed at the log call site to keep the bare `&str`
/// out of the `tracing` field unless redacted.
pub(crate) struct RedactedLogUrl<'a>(pub &'a str);

impl fmt::Display for RedactedLogUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redact_url_credentials(self.0))
    }
}

/// A URL that has passed SSRF validation (scheme allowlist + private-IP block).
///
/// Constructed only by [`ssrf_check`]; downstream consumers (`download`,
/// `reqwest::Client::get`) accept `&ValidatedUrl` so the type system forces
/// every fetch path through the SSRF check.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedUrl(url::Url);

impl ValidatedUrl {
    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Bypasses SSRF validation. Test-only — production code must go through
    /// [`ssrf_check`] so the type system enforces the SSRF contract.
    #[cfg(test)]
    pub(crate) fn for_test(raw: &str) -> Self {
        Self(url::Url::parse(raw).expect("test URL must parse"))
    }
}

impl fmt::Display for ValidatedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub(crate) async fn ssrf_check(
    raw: &str,
    resolver: &dyn DnsResolver,
) -> Result<ValidatedUrl, FetchError> {
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
        if addrs.is_empty() {
            return Err(FetchError::DnsResolution(format!(
                "DNS lookup for {domain} returned no addresses"
            )));
        }

        for ip in addrs {
            if is_private_ip(ip) {
                warn!(host = %domain, ip = %ip, "DNS resolves to private IP");
                return Err(FetchError::InternalHost);
            }
        }
    }

    Ok(ValidatedUrl(parsed))
}

pub(crate) fn validate_url_sync(raw: &str) -> Result<url::Url, FetchError> {
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

fn is_cgn(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

pub(crate) fn is_private_ip(ip: IpAddr) -> bool {
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
mod tests;

/// Resolver test double returning a fixed IP list.
#[cfg(test)]
pub(crate) struct StaticDnsResolver(pub Vec<IpAddr>);

#[cfg(test)]
impl StaticDnsResolver {
    /// Construct from a single IP literal. Panics if `ip` is not a valid `IpAddr`.
    pub(crate) fn single(ip: &str) -> Self {
        Self(vec![ip.parse().expect("test IP must parse")])
    }
}

#[cfg(test)]
impl DnsResolver for StaticDnsResolver {
    fn lookup(&self, _host: &str, _port: u16) -> DnsLookupFuture<'_> {
        let addrs = self.0.clone();
        Box::pin(async move { Ok(addrs) })
    }
}

/// Resolver test double that always fails with the given message.
#[cfg(test)]
pub(crate) struct FailingDnsResolver(pub String);

#[cfg(test)]
impl DnsResolver for FailingDnsResolver {
    fn lookup(&self, _host: &str, _port: u16) -> DnsLookupFuture<'_> {
        let message = self.0.clone();
        Box::pin(async move { Err(FetchError::DnsResolution(message)) })
    }
}

#[cfg(test)]
mod dns_tests;
