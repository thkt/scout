//! Web page fetching with SSRF defense-in-depth.
//!
//! URL validation → DNS pre-check → download → post-redirect recheck → content extraction.

mod cdp;
pub(crate) mod converter;
mod download;
mod extractor;
mod ssrf;

use ssrf::ssrf_check;
pub(crate) use ssrf::{
    DnsResolver, EgressMode, RedactedLogUrl, SsrfResolver, TokioDnsResolver, detect_egress_mode,
};
#[cfg(test)]
pub(crate) use ssrf::{FailingDnsResolver, StaticDnsResolver};

use std::sync::Arc;

use reqwest::Client;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::classify::Classification;
use crate::envelope::ErrorCode;

#[cfg(feature = "js-rendering")]
use cdp::fetch_with_cdp;
use converter::{FetchResult, to_fetch_result};
use download::download;
use extractor::{extract_article, extract_raw};

/// Options for [`fetch_page`] that control rendering, output, and egress.
///
/// Not `Copy`: `egress`'s `Proxied` variant owns a `String`, so callers move or
/// clone the whole `FetchOptions`.
#[derive(Debug, Clone, Default)]
pub(crate) struct FetchOptions {
    /// Force JS rendering via CDP (skip auto-detection). Requires `js-rendering` feature.
    pub(crate) js: bool,
    /// Skip Readability extraction; return full HTML converted to Markdown.
    pub(crate) raw: bool,
    /// Egress routing for this fetch. `Direct` (the default) runs scout's DNS
    /// pre-check and dials the host directly; `Proxied` skips the pre-check and
    /// routes via the configured HTTP proxy (which resolves and dials instead).
    pub(crate) egress: EgressMode,
}

const MAX_RESPONSE_BYTES: usize = 10_000_000;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error("invalid URL: must be HTTP(S)")]
    InvalidScheme,

    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("blocked: internal/private host not allowed")]
    InternalHost,

    #[error("fetch failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("too many redirects (>{0})")]
    TooManyRedirects(usize),

    #[error("redirect without Location header")]
    RedirectMissingLocation,

    #[error("DNS resolution failed: {0}")]
    DnsResolution(String),

    #[error("fetch failed: status {0}")]
    Status(u16),

    #[error("unsupported content type: {0} (expected text/HTML)")]
    UnsupportedContentType(String),

    #[error("response too large (>{} bytes)", MAX_RESPONSE_BYTES)]
    TooLarge,

    /// The payload names what did not respond and within what budget; the
    /// phrase itself belongs to this prefix alone. Carrying it in both read as
    /// "fetch timed out: fetch timed out after 30s" (issue #313). `T-C027` pins
    /// the `fetch` call site, `T-SE015` the research one.
    #[error("fetch timed out: {0}")]
    Timeout(String),

    #[error("browser not available: {0}")]
    BrowserNotFound(String),

    #[error("browser rendering failed: {0}")]
    BrowserFailed(String),
}

impl FetchError {
    /// Map each variant to its ADR-0011 priority-table [`Classification`].
    ///
    /// Arm order is load-bearing: specific `Status` codes (401/403, 404,
    /// 408/429) precede the 4xx fallback so a reorder cannot silently demote
    /// them to DataError.
    pub(crate) fn classify(&self) -> Classification {
        match self {
            // Priority 1: USAGE_ERROR
            Self::BrowserNotFound(_) => Classification::new(ErrorCode::UsageError),
            // Priority 2: DATA_ERROR (non-Status variants)
            Self::InvalidScheme => Classification::new(ErrorCode::DataError)
                .with_hint("URL must use http:// or https://"),
            Self::InvalidUrl(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL must include scheme and host"),
            Self::InternalHost => Classification::new(ErrorCode::DataError)
                .with_hint("URL must point to an external host (private IPs are blocked)"),
            Self::UnsupportedContentType(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL must serve HTML or text content"),
            Self::RedirectMissingLocation => Classification::new(ErrorCode::DataError),
            Self::TooLarge => {
                Classification::new(ErrorCode::DataError).with_hint("fetch a smaller resource")
            }
            // Terminal DataError: cap=5 absorbs canonical chains (HTTPS upgrade →
            // trailing slash → final URL), so a breach dominantly indicates a
            // server-side redirect loop or caller URL mistake — both caller-fixable.
            Self::TooManyRedirects(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL has too many redirects; check for a redirect loop"),
            // The ADR-0003 table decides the code; these two arms add the hint
            // only fetch can give, so they sit ahead of the delegating one.
            Self::Status(code @ (401 | 403)) => Classification::from_http_status(*code)
                .with_hint("URL requires authentication that scout does not support"),
            Self::Status(code @ 404) => Classification::from_http_status(*code)
                .with_hint("Check that the URL is correct and the resource exists"),
            Self::Status(code) => Classification::from_http_status(*code),
            // Priority 4: TIMEOUT (transport timeout — long-backoff retry advised)
            Self::Timeout(_) => Classification::timeout_retry(),
            // Priority 4: TEMP_FAILURE (non-Status variants)
            Self::DnsResolution(_) => Classification::new(ErrorCode::TempFailure)
                .with_hint("Check the URL's domain name and your DNS resolver"),
            // Priority 4 (TIMEOUT) and 退避: see `Classification::from_reqwest`
            Self::Http(re) => Classification::from_reqwest(re),
            // Priority 5 sibling: IO_ERROR — external tool failure (browser)
            Self::BrowserFailed(_) => Classification::new(ErrorCode::IoError),
        }
    }
}

/// ~1 sentence; pages below this almost always need JS rendering.
const EXTRACT_TEXT_THRESHOLD: usize = 50;

pub(crate) async fn fetch_page(
    client: &Client,
    url: &str,
    opts: FetchOptions,
    resolver: Arc<dyn DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<FetchResult, FetchError> {
    // `cancel` is only consumed on the CDP path. Silence the unused-arg
    // warning in builds without `js-rendering` instead of duplicating the
    // function signature behind cfg.
    #[cfg(not(feature = "js-rendering"))]
    let _ = cancel;

    #[cfg(not(feature = "js-rendering"))]
    if opts.js {
        return Err(FetchError::BrowserNotFound(
            "js-rendering feature required — rebuild with `--features js-rendering`".into(),
        ));
    }

    // SECURITY: Defense in depth (ADR-0012). `ssrf_check` is a pre-flight that
    // resolves the host and blocks private IPs, but reqwest re-resolves at
    // connect time, leaving a DNS-rebind TOCTOU gap. The `fetch_http` client is
    // built with `SsrfResolver` (ClientBuilder::dns_resolver), which re-applies
    // the private-IP block to the addresses reqwest actually dials and closes
    // that gap.
    //
    // The returned `ValidatedUrl` is the only constructor for SSRF-checked URLs;
    // `download` requires `&ValidatedUrl` so the redirect loop cannot bypass it.
    // `opts.egress` selects the mode: `Direct` runs the DNS pre-check because
    // scout resolves and dials the host itself; `Proxied` skips the pre-check
    // (the proxy resolves and dials) while `ssrf_check` still rejects literal
    // private/loopback hosts. `download` re-checks every redirect hop under the
    // same mode.
    let egress = &opts.egress;
    let validated = ssrf_check(url, resolver.as_ref(), egress).await?;

    // `decode_uncertain` flags a body neither the server charset label nor
    // reliability-gated detection could decode cleanly (issue #241). It is
    // cleared whenever CDP output replaces the body below, because the headless
    // browser re-decodes the page from its own response handling.
    #[cfg(feature = "js-rendering")]
    let (final_url, mut html, mut decode_uncertain) =
        download(client, &validated, MAX_REDIRECTS, resolver.as_ref(), egress).await?;
    #[cfg(not(feature = "js-rendering"))]
    let (final_url, html, decode_uncertain) =
        download(client, &validated, MAX_REDIRECTS, resolver.as_ref(), egress).await?;

    let need_js = if opts.js {
        info!("--js flag set, requesting JS rendering");
        true
    } else if is_js_dependent(&html) {
        warn!("JS-dependent page detected, trying JS rendering fallback");
        true
    } else {
        false
    };

    if need_js {
        #[cfg(feature = "js-rendering")]
        {
            match fetch_with_cdp(&final_url, Arc::clone(&resolver), cancel).await {
                Ok(js_html) => {
                    info!("JS rendering succeeded via CDP");
                    html = js_html;
                    decode_uncertain = false;
                }
                Err(e) if opts.js => {
                    return Err(FetchError::from(e));
                }
                Err(e) => {
                    warn!(error = %e, "JS rendering failed, using original HTML");
                }
            }
        }
        #[cfg(not(feature = "js-rendering"))]
        {
            warn!(
                "JS rendering unavailable (js-rendering feature not enabled), using original HTML"
            );
        }
    }

    let article = if opts.raw {
        extract_raw(&html)
    } else {
        extract_article(&html, Some(final_url.as_str()))
    };

    let need_thin_fallback = !opts.raw && !need_js && is_thin_extract(&article);
    #[cfg(feature = "js-rendering")]
    let article = if need_thin_fallback {
        warn!(url = %RedactedLogUrl(final_url.as_str()), "extraction yielded too little content, trying JS rendering fallback");
        match fetch_with_cdp(&final_url, Arc::clone(&resolver), cancel).await {
            Ok(js_html) => {
                let re_extracted = extract_article(&js_html, Some(final_url.as_str()));
                // CDP re-decoded the page from its own response handling, so the
                // original label/detection uncertainty no longer applies.
                decode_uncertain = false;
                if is_thin_extract(&re_extracted) {
                    debug!(url = %RedactedLogUrl(final_url.as_str()), "JS re-extraction still thin, returning best-effort result");
                } else {
                    debug!(url = %RedactedLogUrl(final_url.as_str()), "JS rendering fallback succeeded (post-extraction)");
                }
                re_extracted
            }
            Err(e) => {
                warn!(url = %RedactedLogUrl(final_url.as_str()), error = %e, "JS rendering fallback failed, using original extraction");
                article
            }
        }
    } else {
        article
    };
    #[cfg(not(feature = "js-rendering"))]
    if need_thin_fallback {
        warn!(url = %RedactedLogUrl(final_url.as_str()), "extraction yielded too little content but JS rendering unavailable");
    }

    debug!(url = %RedactedLogUrl(final_url.as_str()), bytes = html.len(), "page fetched");
    Ok(to_fetch_result(
        &article,
        final_url.as_str().to_owned(),
        decode_uncertain,
    ))
}

/// Raw fallback is always thin because shell text (nav, footer) inflates
/// the count but the article body is missing.
fn is_thin_extract(article: &extractor::ExtractedArticle) -> bool {
    article.used_raw_fallback
        || visible_text_len(&article.content_html, EXTRACT_TEXT_THRESHOLD) < EXTRACT_TEXT_THRESHOLD
}

fn visible_text_len(html: &str, limit: usize) -> usize {
    let mut count = 0usize;
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag || ch.is_whitespace() => {}
            _ => {
                count += 1;
                if count >= limit {
                    return count;
                }
            }
        }
    }
    count
}

const BODY_TEXT_THRESHOLD: usize = 100;

const SPA_ROOT_IDS: &[&str] = &[
    r#"id="root""#,
    r#"id="app""#,
    r#"id="__next""#,
    r#"id="__nuxt""#,
];

/// Case-insensitive substring search over raw bytes, for HTML tag names — which
/// are case-insensitive, unlike the attribute values in [`SPA_ROOT_IDS`].
fn contains_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

fn is_js_dependent(html: &str) -> bool {
    if !has_thin_body(html) {
        return false;
    }
    contains_ignore_ascii_case(html.as_bytes(), b"<script")
        || SPA_ROOT_IDS.iter().any(|p| html.contains(p))
}

fn has_thin_body(html: &str) -> bool {
    let bytes = html.as_bytes();
    let body_start = bytes
        .windows(5)
        .position(|w| w.eq_ignore_ascii_case(b"<body"));
    let body = if let Some(start) = body_start {
        let after_tag = html[start..]
            .find('>')
            .map(|i| start + i + 1)
            .unwrap_or(start);
        let body_end = bytes[after_tag..]
            .windows(7)
            .position(|w| w.eq_ignore_ascii_case(b"</body>"))
            .map(|i| after_tag + i)
            .unwrap_or(html.len());
        &html[after_tag..body_end]
    } else {
        html
    };

    let mut visible_bytes = 0usize;
    let mut in_tag = false;
    let mut skip_text = false;
    let mut tag_buf = [0u8; 16];
    let mut tag_len = 0usize;
    let mut reading_name = false;
    let mut in_whitespace = true;

    for ch in body.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_len = 0;
                reading_name = true;
            }
            '>' if in_tag => {
                in_tag = false;
                reading_name = false;
                let name = &tag_buf[..tag_len];
                if name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"style") {
                    skip_text = true;
                } else if name.eq_ignore_ascii_case(b"/script")
                    || name.eq_ignore_ascii_case(b"/style")
                {
                    skip_text = false;
                }
            }
            _ if in_tag => {
                if reading_name {
                    if ch.is_ascii_alphanumeric() || ch == '/' {
                        if tag_len < tag_buf.len() {
                            tag_buf[tag_len] = ch as u8;
                            tag_len += 1;
                        }
                    } else {
                        reading_name = false;
                    }
                }
            }
            _ if skip_text => {}
            _ if ch.is_whitespace() => {
                if !in_whitespace && visible_bytes > 0 {
                    visible_bytes += 1;
                    in_whitespace = true;
                }
            }
            _ => {
                visible_bytes += ch.len_utf8();
                in_whitespace = false;
                if visible_bytes >= BODY_TEXT_THRESHOLD {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod fetch_page_tests;
#[cfg(test)]
mod js_dependent_tests;
#[cfg(test)]
mod thin_body_tests;
#[cfg(test)]
mod thin_extract_tests;
