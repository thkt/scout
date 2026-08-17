//! SSRF defense-in-depth: URL validation and DNS pre-check.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::net::lookup_host;
use tokio::time::timeout;
use tracing::warn;

use super::FetchError;

const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Egress routing decided from the environment: direct connection or via an
/// HTTP(S) proxy at the given URL. Mirrors the proxy env vars reqwest users
/// expect (see [`detect_egress_mode`]).
///
/// `ScoutBuilder::from_env` detects the mode once via [`detect_egress_mode`],
/// builds `fetch_http` to match (proxied client vs. connect-time SSRF guard),
/// and carries the mode to `fetch_page` through `FetchOptions.egress`.
/// `ssrf_check` takes `&EgressMode` to gate its DNS pre-check, which `Proxied`
/// skips (the proxy resolves and dials, not scout).
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) enum EgressMode {
    #[default]
    Direct,
    Proxied(String),
}

/// Detect the egress mode from an environment map (data in, data out — never
/// reads process env, so callers control the inputs and tests stay pure).
///
/// Reads the same proxy vars reqwest users set. Precedence as pinned by the
/// egress test scenarios: `HTTPS_PROXY` before `HTTP_PROXY`, uppercase before
/// lowercase, first match wins; absence yields [`EgressMode::Direct`]. The case
/// order (upper before lower) is a chosen convention: reqwest 0.13's env-var
/// precedence is undocumented on
/// <https://docs.rs/reqwest/0.13/reqwest/struct.Proxy.html> and no test pins
/// the case order — unverified.
/// A present-but-empty value counts as unset, matching reqwest: `Proxy::all("")`
/// is a relative-URL parse error, so treating it as `Proxied("")` would fail
/// client construction and take down every command, not just proxied fetches.
pub(crate) fn detect_egress_mode(env: &HashMap<String, String>) -> EgressMode {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Some(url) = env.get(key).filter(|u| !u.trim().is_empty()) {
            return EgressMode::Proxied(url.clone());
        }
    }
    EgressMode::Direct
}

/// Object-safe boxed future returned by [`DnsResolver::lookup`].
type DnsLookupFuture<'a> =
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

/// A URL's userinfo is a credential, so it must not reach a log line.
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
pub(super) struct ValidatedUrl(url::Url);

impl ValidatedUrl {
    pub(super) fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Resolve `relative` against this URL, for following a `Location` header.
    /// Reuses the parse [`ssrf_check`] already did — resolving from `as_str()`
    /// would re-parse a string that came out of a `url::Url` in the first place,
    /// adding an error branch nothing can reach. The result is a plain `Url`, so
    /// the caller still has to run it back through `ssrf_check`.
    pub(super) fn join(&self, relative: &str) -> Result<url::Url, url::ParseError> {
        self.0.join(relative)
    }

    /// Bypasses SSRF validation. Test-only — production code must go through
    /// [`ssrf_check`] so the type system enforces the SSRF contract.
    #[cfg(test)]
    pub(super) fn for_test(raw: &str) -> Self {
        Self(url::Url::parse(raw).expect("test URL must parse"))
    }
}

impl fmt::Display for ValidatedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub(super) async fn ssrf_check(
    raw: &str,
    resolver: &dyn DnsResolver,
    mode: &EgressMode,
) -> Result<ValidatedUrl, FetchError> {
    // `validate_url_sync` is unconditional in every mode: the scheme allowlist
    // and the literal private-IP / loopback / blocked-suffix rejection guard the
    // URL itself, independent of who does the DNS. Only the resolver.lookup
    // pre-check below is mode-gated.
    let parsed = validate_url_sync(raw).map_err(|e| {
        if matches!(e, FetchError::InternalHost) {
            warn!(url = %redact_url_credentials(raw), "blocked fetch to internal/private host");
        }
        e
    })?;

    // Proxied egress skips the local DNS pre-check: scout does not resolve or
    // dial the host — the proxy does — so a local lookup validates addresses
    // scout never connects to and can wrongly reject hosts only the proxy can
    // resolve. Literal-IP/suffix rejection above still applies on this URL and,
    // via the download redirect loop, on every hop.
    if matches!(mode, EgressMode::Proxied(_)) {
        return Ok(ValidatedUrl(parsed));
    }

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

        if first_blocked_ip("preflight", domain, &addrs).is_some() {
            return Err(FetchError::InternalHost);
        }
    }

    Ok(ValidatedUrl(parsed))
}

/// Fail-closed check over a resolved address set: if any address is private, the
/// whole connection is refused — the public members are never dialed, because a
/// host that answers with both is exactly the rebind shape ADR-0012 blocks.
///
/// Returns the offending address so callers that name it in an error can, and
/// logs the block here so all three stages report it identically. `stage`
/// distinguishes them: `preflight` (before the request), `connect` (reqwest's
/// resolver), `proxy` (the CDP SOCKS5 hop).
pub(super) fn first_blocked_ip(stage: &'static str, host: &str, ips: &[IpAddr]) -> Option<IpAddr> {
    let blocked = ips.iter().copied().find(|ip| is_private_ip(*ip))?;
    warn!(stage, host = %host, ip = %blocked, "blocked connect to private IP");
    Some(blocked)
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
            // `url` keeps the FQDN trailing dot in the host, so `localhost.`
            // and `svc.internal.` reach here undotted-suffix-matched unless it
            // is stripped first. Both forms resolve to the same host.
            let lower = domain.to_ascii_lowercase();
            let lower = lower.strip_suffix('.').unwrap_or(&lower);
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
                // `to_ipv4` (not `to_ipv4_mapped`) so both IPv4-mapped
                // (`::ffff:a.b.c.d`) and IPv4-compatible (`::a.b.c.d`, e.g.
                // `::7f00:1` = `::127.0.0.1`) embeddings are unwrapped and
                // re-checked. `::1`/`::` map to `0.0.0.1`/`0.0.0.0`, already
                // caught by the loopback/unspecified arms — no false positives.
                || v6
                    .to_ipv4()
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

/// reqwest [`Resolve`] implementation that re-validates resolved IPs at connect
/// time, closing the SSRF DNS-rebind TOCTOU gap (ADR-0012).
///
/// [`ssrf_check`] validates the IPs resolved during the pre-flight lookup, but
/// reqwest re-resolves the host when it opens the connection. A name server that
/// returns a public IP for the pre-flight query and a private IP at connect time
/// (DNS rebinding) would slip past the pre-flight check. Injecting this resolver
/// via `ClientBuilder::dns_resolver` makes reqwest reuse the same private-IP
/// block for the addresses it actually dials.
pub(crate) struct SsrfResolver {
    inner: Arc<dyn DnsResolver>,
}

impl SsrfResolver {
    pub(crate) fn new(inner: impl DnsResolver + 'static) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }
}

impl Resolve for SsrfResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let inner = self.inner.clone();
        Box::pin(async move {
            let host = name.as_str();
            // Port 0: reqwest substitutes the scheme default before dialing.
            let ips = inner.lookup(host, 0).await?;
            if let Some(ip) = first_blocked_ip("connect", host, &ips) {
                return Err(format!("blocked connect to private IP {ip}").into());
            }
            let addrs: Addrs = Box::new(
                ips.into_iter()
                    .map(|ip| SocketAddr::new(ip, 0))
                    .collect::<Vec<_>>()
                    .into_iter(),
            );
            Ok(addrs)
        })
    }
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

#[cfg(test)]
mod egress_tests;
