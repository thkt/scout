//! HTTP download with per-hop SSRF re-validation and charset-aware decoding.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use reqwest::Client;
use reqwest::header::LOCATION;
use tracing::{debug, warn};

use super::ssrf::{DnsResolver, EgressMode, RedactedLogUrl, ValidatedUrl, ssrf_check};
use super::{FetchError, MAX_RESPONSE_BYTES};
use crate::charset::is_reliable_detection;
use crate::retry::read_body_capped;

/// Caller MUST pass a [`Client`] with [`reqwest::redirect::Policy::none()`].
///
/// `reqwest::redirect::Policy::limited(n)` is not acceptable: it follows
/// redirects before the application can re-check the resolved URL against
/// the SSRF allowlist. Manual per-hop validation is the only way to enforce
/// the SSRF contract. See ADR-0001 for the contract details.
///
/// `&ValidatedUrl` here closes that gap at the type level — the manual
/// redirect loop cannot accept an unchecked URL.
pub(super) async fn download(
    client: &Client,
    url: &ValidatedUrl,
    max_redirects: usize,
    resolver: &dyn DnsResolver,
    mode: &EgressMode,
) -> Result<(ValidatedUrl, String, bool), FetchError> {
    let mut current_url = url.clone();

    for _hop in 0..=max_redirects {
        let response = client
            .get(current_url.as_str())
            .header("User-Agent", crate::USER_AGENT)
            .send()
            .await?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::RedirectMissingLocation)?;

            let base = url::Url::parse(current_url.as_str())?;
            let next_url = base.join(location)?.to_string();

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
        match response.headers().get("content-type") {
            None => {
                debug!(url = %RedactedLogUrl(current_url.as_str()), "no Content-Type header, proceeding as text")
            }
            Some(ct) => match ct.to_str() {
                Ok(ct_str) => {
                    check_content_type(ct_str)?;
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
        return Ok((current_url, decoded.text, decoded.uncertain));
    }

    // CALIBRATION (issue #145 / #148 follow-up): structured fields below let
    // callers sample empirical retry-success rate via `RUST_LOG=scout=warn`.
    // Flip from DataError(65) to TempFailure(75) once rate > 10%.
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
/// `DegradedReason::DecodeUncertain` (issue #241). The body is still returned at
/// exit 0; the AI caller decides whether to trust it.
struct DecodedBody {
    text: String,
    uncertain: bool,
}

/// Decode a response body label-first, recovering mislabeled multi-byte content
/// via chardetng before giving up (issue #241).
///
/// 1. Decode with the server charset label (default utf-8). A clean decode
///    (`had_errors == false`) is returned as-is, not uncertain.
/// 2. On a lossy or unknown-label decode, fall back to reliability-gated
///    detection: a multi-byte encoding that decodes cleanly is trusted and the
///    recovered text is returned, not uncertain.
/// 3. If neither succeeds, return the lossy UTF-8 best effort with
///    `uncertain = true`.
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

fn check_content_type(content_type: &str) -> Result<(), FetchError> {
    let mime = content_type
        .split_once(';')
        .map_or(content_type, |(mime, _params)| mime)
        .trim();
    if !mime.is_empty()
        && !mime.starts_with("text/")
        && mime != "application/xhtml+xml"
        && mime != "application/xml"
    {
        return Err(FetchError::UnsupportedContentType(mime.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod charset_tests;
#[cfg(test)]
mod content_type_tests;
#[cfg(test)]
mod download_tests;
