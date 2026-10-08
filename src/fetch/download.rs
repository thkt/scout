//! HTTP download with per-hop SSRF re-validation and charset-aware decoding.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use reqwest::Client;
use reqwest::header::{CONTENT_TYPE, LOCATION};
use tracing::{debug, warn};

use super::ssrf::{DnsResolver, EgressMode, RedactedLogUrl, ValidatedUrl, ssrf_check};
use super::{FetchError, MAX_RESPONSE_BYTES};
use crate::body_limit::read_body_capped;
use crate::charset::is_reliable_detection;

/// Decoded final response. Every redirect hop passed `ssrf_check`; `url` is
/// the base for relative links. Decoding uncertainty travels with the body.
#[derive(Debug)]
pub(super) struct DownloadedPage {
    pub(super) url: ValidatedUrl,
    pub(super) text: String,
    pub(super) decode_uncertain: bool,
    pub(super) media_type: MediaType,
}

/// Unknown headers retain the historical HTML path; only an explicit media
/// type can suppress HTML's automatic rendering heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MediaType {
    Unknown,
    Html,
    PlainText,
    OtherText,
}

/// Requires a client with [`reqwest::redirect::Policy::none()`]: automatic
/// redirects would bypass per-hop SSRF validation (ADR-0001). The starting URL
/// must also be validated. Production clients supply scout's User-Agent.
pub(super) async fn download(
    client: &Client,
    url: &ValidatedUrl,
    max_redirects: usize,
    resolver: &dyn DnsResolver,
    mode: &EgressMode,
) -> Result<DownloadedPage, FetchError> {
    let mut current_url = url.clone();

    for _hop in 0..=max_redirects {
        let response = client.get(current_url.as_str()).send().await?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::RedirectMissingLocation)?;

            let next_url = current_url.join(location)?.to_string();

            let next_validated = ssrf_check(&next_url, resolver, mode).await?;

            debug!(
                from = %RedactedLogUrl(current_url.as_str()),
                to = %RedactedLogUrl(next_validated.as_str()),
                "following redirect"
            );
            current_url = next_validated;
            continue;
        }

        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }

        let mut charset = None;
        let mut media_type = MediaType::Unknown;
        match response.headers().get(CONTENT_TYPE) {
            None => {
                debug!(url = %RedactedLogUrl(current_url.as_str()), "no Content-Type header, proceeding as text")
            }
            Some(ct) => match ct.to_str() {
                Ok(ct_str) => {
                    media_type = check_content_type(ct_str)?;
                    charset = extract_charset(ct_str);
                }
                Err(_) => {
                    warn!(url = %RedactedLogUrl(current_url.as_str()), "Content-Type header is not valid ASCII, proceeding as text")
                }
            },
        }

        let body = read_body_capped(
            response,
            MAX_RESPONSE_BYTES,
            || FetchError::TooLarge,
            FetchError::from,
        )
        .await?;
        let decoded = decode_body(&body, charset.as_deref());
        return Ok(DownloadedPage {
            url: current_url,
            text: decoded.text,
            decode_uncertain: decoded.uncertain,
            media_type,
        });
    }

    // Redirect cap breaches are terminal DataError: retrying a loop or incorrect
    // URL does not fix it. Log the chain length for diagnosis.
    let chain_length = max_redirects + 1;
    warn!(
        redirect_chain_length = chain_length,
        max_redirects,
        final_url = %RedactedLogUrl(current_url.as_str()),
        "redirect cap exceeded"
    );
    Err(FetchError::TooManyRedirects(max_redirects))
}

fn extract_charset(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let param = param.trim();
        let lower = param.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("charset=") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
        None
    })
}

/// Outcome of decoding a response body. `uncertain` is true when neither the
/// server-labeled encoding nor reliability-gated detection produced a clean
/// decode, so `text` is a best-effort lossy rendering the caller surfaces via
/// `DegradedReason::DecodeUncertain`. The body is still returned at
/// exit 0; the AI caller decides whether to trust it.
struct DecodedBody {
    text: String,
    uncertain: bool,
}

/// Decode a response body label-first, recovering mislabeled multi-byte content
/// via chardetng before giving up. Detection-recovered text is
/// returned as trusted, not uncertain.
fn decode_body(bytes: &[u8], charset: Option<&str>) -> DecodedBody {
    let label = charset.unwrap_or("utf-8");
    match encoding_rs::Encoding::for_label(label.as_bytes()) {
        Some(encoding) => {
            let (decoded, _, had_errors) = encoding.decode(bytes);
            if !had_errors {
                return DecodedBody {
                    text: decoded.into_owned(),
                    uncertain: false,
                };
            }
            debug!(
                charset = label,
                "labeled decode produced replacement characters, trying detection"
            );
        }
        None => warn!(
            charset = label,
            "unknown charset label, trying detection then UTF-8"
        ),
    }

    if let Some(text) = detect_decode(bytes) {
        return DecodedBody {
            text,
            uncertain: false,
        };
    }

    warn!(
        charset = label,
        "decode uncertain: returning best-effort lossy body (DECODE_UNCERTAIN)"
    );
    DecodedBody {
        text: String::from_utf8_lossy(bytes).into_owned(),
        uncertain: true,
    }
}

/// Reliability-gated chardetng detection. Returns a clean decode only when the
/// guessed encoding is a multi-byte one (strict byte-pattern constraints, see
/// [`crate::charset::is_reliable_detection`]) and it decodes without errors.
/// Single-byte guesses and lossy decodes return `None` so the caller treats the
/// body as uncertain rather than silently trusting mojibake.
fn detect_decode(bytes: &[u8]) -> Option<String> {
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);
    if !is_reliable_detection(encoding) {
        return None;
    }
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return None;
    }
    Some(decoded.into_owned())
}

/// Accept text and application XML, including RFC 6839 `+xml` suffixes.
/// `image/svg+xml` remains excluded; an empty media type declares no restriction.
fn check_content_type(content_type: &str) -> Result<MediaType, FetchError> {
    let mime = content_type
        .split_once(';')
        .map_or(content_type, |(mime, _params)| mime)
        .trim();
    // Media type tokens are case-insensitive (RFC 9110 section 8.3.1).
    // Keep `mime` for diagnostics so rejection still reports the actual header.
    let normalized = mime.to_ascii_lowercase();
    let accepted = normalized.is_empty()
        || normalized.starts_with("text/")
        || normalized == "application/xml"
        || (normalized.starts_with("application/") && normalized.ends_with("+xml"));
    if !accepted {
        return Err(FetchError::UnsupportedContentType(mime.to_owned()));
    }
    Ok(match normalized.as_str() {
        "" => MediaType::Unknown,
        "text/html" | "application/xhtml+xml" => MediaType::Html,
        "text/plain" => MediaType::PlainText,
        _ => MediaType::OtherText,
    })
}

#[cfg(test)]
mod charset_tests;
#[cfg(test)]
mod content_type_tests;
#[cfg(test)]
mod download_tests;
