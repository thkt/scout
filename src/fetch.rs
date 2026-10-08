//! Web page fetching with SSRF defense-in-depth.
//!
//! URL validation → DNS pre-check → download → post-redirect recheck → content extraction.

mod cdp;
pub(crate) mod converter;
mod download;
mod extractor;
mod ssrf;

/// Test-only re-export for the `tools::config` timeout invariant.
#[cfg(all(test, feature = "js-rendering"))]
pub(crate) use cdp::CDP_TIMEOUT;
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
use converter::{FetchResult, plain_text_result, to_fetch_result};
use download::{DownloadedPage, MediaType, download};
use extractor::{extract_article, extract_raw};

/// Options for [`fetch_page`] that control rendering, output, and egress.
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

/// Manual redirect hop limit; every hop repeats the SSRF check.
/// The service clients use their own independent reqwest redirect policy.
const FETCH_MAX_REDIRECTS: usize = 5;

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

    #[error("markdown conversion failed: {0}")]
    MarkdownConversion(String),

    #[error("response too large (>{} bytes)", MAX_RESPONSE_BYTES)]
    TooLarge,

    /// Payload names the operation and budget without repeating this error prefix.
    /// T-C024 and T-SE015 cover the fetch and research call sites.
    #[error("fetch timed out: {0}")]
    Timeout(String),

    #[error("browser not available: {0}")]
    BrowserNotFound(String),

    #[error("browser rendering failed: {0}")]
    BrowserFailed(String),
}

impl FetchError {
    /// Map variants to the ADR-0011 classification table.
    /// Specific status hints must precede the generic `Status` delegation.
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
            Self::MarkdownConversion(_) => Classification::new(ErrorCode::DataError),
            Self::RedirectMissingLocation => Classification::new(ErrorCode::DataError),
            Self::TooLarge => {
                Classification::new(ErrorCode::DataError).with_hint("fetch a smaller resource")
            }
            // Redirect loops and caller URL mistakes are terminal DataError.
            Self::TooManyRedirects(_) => Classification::new(ErrorCode::DataError)
                .with_hint("URL has too many redirects; check for a redirect loop"),
            // Specific status arms add fetch hints; classification stays in ADR-0003.
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
            // Priority 4 (TIMEOUT or TEMP_FAILURE) or the retreat slot, by error kind
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
    // `cancel` is used only by CDP; keep one signature across feature builds.
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

    // CDP replaces the decoded body and clears its original decoding uncertainty.
    #[cfg(feature = "js-rendering")]
    let DownloadedPage {
        url: final_url,
        text: mut html,
        mut decode_uncertain,
        media_type,
    } = download(
        client,
        &validated,
        FETCH_MAX_REDIRECTS,
        resolver.as_ref(),
        egress,
    )
    .await?;
    #[cfg(not(feature = "js-rendering"))]
    let DownloadedPage {
        url: final_url,
        text: html,
        decode_uncertain,
        media_type,
    } = download(
        client,
        &validated,
        FETCH_MAX_REDIRECTS,
        resolver.as_ref(),
        egress,
    )
    .await?;

    // An explicit --js still requests browser output. Otherwise accepted non-HTML
    // text is source content, even when it contains HTML-looking bytes.
    if matches!(media_type, MediaType::PlainText | MediaType::OtherText) && !opts.js {
        return Ok(plain_text_result(
            &html,
            final_url.as_str().to_owned(),
            decode_uncertain,
        ));
    }
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
    to_fetch_result(&article, final_url.as_str().to_owned(), decode_uncertain)
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

/// Count characters, not bytes, so CJK and Latin text use the same threshold.
const BODY_TEXT_THRESHOLD: usize = 100;

/// Double-quoted app-root markers provide evidence for thin, script-free shells
/// (T-F023). Attribute values remain case-sensitive. Single/unquoted forms are
/// not detected here; a `<script` tag provides an independent heuristic.
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

    let mut visible_chars = 0usize;
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
                if !in_whitespace && visible_chars > 0 {
                    visible_chars += 1;
                    in_whitespace = true;
                }
            }
            _ => {
                visible_chars += 1;
                in_whitespace = false;
                if visible_chars >= BODY_TEXT_THRESHOLD {
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
