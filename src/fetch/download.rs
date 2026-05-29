//! HTTP download with per-hop SSRF re-validation and charset-aware decoding.

use reqwest::Client;
use reqwest::header::LOCATION;
use tracing::{debug, warn};

use super::ssrf::{DnsResolver, RedactedLogUrl, ValidatedUrl, ssrf_check};
use super::{FetchError, MAX_RESPONSE_BYTES};

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
) -> Result<(ValidatedUrl, String), FetchError> {
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

            let next_validated = ssrf_check(&next_url, resolver).await?;

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

        let content_length = response.content_length();
        if let Some(len) = content_length
            && usize::try_from(len).unwrap_or(usize::MAX) > MAX_RESPONSE_BYTES
        {
            return Err(FetchError::TooLarge);
        }

        let capacity = content_length
            .map(|len| {
                usize::try_from(len)
                    .unwrap_or(usize::MAX)
                    .min(MAX_RESPONSE_BYTES)
            })
            .unwrap_or(8192);
        let mut body = Vec::with_capacity(capacity);
        let mut stream = response;
        while let Some(chunk) = stream.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_RESPONSE_BYTES {
                return Err(FetchError::TooLarge);
            }
        }
        let html = decode_body(&body, charset.as_deref());
        return Ok((current_url, html));
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

fn decode_body(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.unwrap_or("utf-8");
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or_else(|| {
        warn!(
            charset = label,
            "unknown charset label, falling back to UTF-8"
        );
        encoding_rs::UTF_8
    });
    if encoding == encoding_rs::UTF_8 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        warn!(
            charset = label,
            "lossy decoding: some bytes could not be decoded"
        );
    }
    decoded.into_owned()
}

fn check_content_type(content_type: &str) -> Result<(), FetchError> {
    let mime = content_type.split(';').next().unwrap_or("").trim();
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
