use super::*;

/// [T-FS004] ssrf_blocks_dns_resolving_to_private_ip
#[tokio::test]
async fn ssrf_blocks_dns_resolving_to_private_ip() {
    let resolver = StaticDnsResolver::single("127.0.0.1");
    let result = ssrf_check("https://evil.com/secret", &resolver).await;
    assert!(matches!(result, Err(FetchError::InternalHost)));
}

/// [T-FS005] ssrf_allows_dns_resolving_to_public_ip
#[tokio::test]
async fn ssrf_allows_dns_resolving_to_public_ip() {
    let resolver = StaticDnsResolver::single("8.8.8.8");
    let result = ssrf_check("https://example.com/page", &resolver).await;
    assert!(result.is_ok());
}

/// [T-FS006] ssrf_returns_error_on_dns_failure
#[tokio::test]
async fn ssrf_returns_error_on_dns_failure() {
    let resolver = FailingDnsResolver("lookup failed".into());
    let result = ssrf_check("https://example.com/page", &resolver).await;
    assert!(matches!(result, Err(FetchError::DnsResolution(_))));
}

/// [T-FS007] ssrf_skips_dns_for_ip_literals
#[tokio::test]
async fn ssrf_skips_dns_for_ip_literals() {
    let resolver = StaticDnsResolver(vec![]);
    let result = ssrf_check("https://8.8.8.8/page", &resolver).await;
    assert!(result.is_ok());
}

/// [T-FS013] ssrf_rejects_empty_dns_response
///
/// A domain resolving to zero addresses (NOERROR + empty A/AAAA) must fail
/// closed: the `ValidatedUrl` "DNS-checked" invariant is unmet, so the
/// for-loop being skipped must not yield `Ok`. T-FS007 above shows the IP
/// literal path stays `Ok` with the same empty resolver.
#[tokio::test]
async fn ssrf_rejects_empty_dns_response() {
    let resolver = StaticDnsResolver(vec![]);
    let result = ssrf_check("https://evil.com/secret", &resolver).await;
    assert!(
        matches!(result, Err(FetchError::DnsResolution(_))),
        "empty DNS response must fail closed, got: {result:?}"
    );
}

/// [T-FS008] redact_strips_userinfo
#[test]
fn redact_strips_userinfo() {
    let url = "https://user:password@example.com/path";
    let safe = redact_url_credentials(url);
    assert!(!safe.contains("user"));
    assert!(!safe.contains("password"));
    assert!(safe.contains("example.com/path"));
}

/// [T-FS009] redact_preserves_clean_url
#[test]
fn redact_preserves_clean_url() {
    let url = "https://example.com/path";
    assert!(matches!(redact_url_credentials(url), Cow::Borrowed(_)));
}

/// [T-FS010] redact_handles_username_only
#[test]
fn redact_handles_username_only() {
    let url = "https://admin@example.com/";
    let safe = redact_url_credentials(url);
    assert!(!safe.contains("admin"));
    assert!(safe.contains("example.com"));
}

/// [T-FS011] redacted_log_url_display_strips_userinfo
///
/// Guards the `Display` impl directly so a future divergence between
/// `RedactedLogUrl::fmt` and `redact_url_credentials` would be caught.
#[test]
fn redacted_log_url_display_strips_userinfo() {
    let formatted = format!("{}", RedactedLogUrl("https://user:secret@example.com/path"));
    assert!(!formatted.contains("secret"));
    assert!(!formatted.contains("user:"));
    assert!(formatted.contains("example.com/path"));
}

/// [T-FS014] redact_falls_back_when_unparseable
///
/// `url::Url::parse` rejects `file://user:pass@host/path` with `IdnaError`
/// (file scheme has no userinfo grammar, so `user:pass@host` parses as an
/// invalid host). The browser-subrequest log path
/// (`check_browser_request` unrecognized-scheme branch) hands such raw URLs
/// to redaction; returning the input unchanged on parse failure leaks the
/// credentials into the log. Redaction must fail closed to `[redacted-url]`.
#[test]
fn redact_falls_back_when_unparseable() {
    let safe = redact_url_credentials("file://user:pass@host/path");
    assert!(!safe.contains("user"), "username leaked: {safe}");
    assert!(!safe.contains("pass"), "password leaked: {safe}");
    assert_eq!(safe, "[redacted-url]");
}

/// [T-FS015] redact_preserves_at_sign_outside_userinfo
///
/// Fail-closed parse handling (T-FS014) must not over-redact: a `@` in the
/// path parses cleanly with an empty username, so the URL is returned
/// verbatim instead of collapsing to `[redacted-url]`.
#[test]
fn redact_preserves_at_sign_outside_userinfo() {
    let url = "https://example.com/feed@v2";
    assert!(
        matches!(redact_url_credentials(url), Cow::Borrowed(s) if s == url),
        "URL without userinfo must be returned verbatim"
    );
}
