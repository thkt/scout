use std::str;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use tracing::debug;

use super::GitHubError;
use crate::charset::is_reliable_detection;

/// How the encoding was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectionSource {
    /// User supplied `--encoding <label>`.
    Explicit,
    /// Byte-order mark found at the start of the content.
    Bom,
    /// chardetng auto-detected the encoding.
    Detected,
    /// chardetng was inconclusive but content is valid strict UTF-8.
    AssumedUtf8,
}

/// Result of decoding raw bytes into Unicode text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodeResult {
    pub text: String,
    /// The encoding label in lowercase (e.g. "shift_jis", "utf-8").
    pub encoding: String,
    pub source: DetectionSource,
}

/// Decode a base64-encoded string into raw bytes.
///
/// Encoding detection is handled separately by `decode_bytes`.
pub(super) fn decode_base64(encoded: &str) -> Result<Vec<u8>, GitHubError> {
    // GitHub wraps base64 at 60 chars, but base64 v0.22 has no whitespace-tolerant
    // decode (GeneralPurposeConfig exposes only padding/trailing-bit options), so the
    // newlines must be stripped into a contiguous buffer first. This transient copy is
    // accepted over a streaming DecoderReader: decode_base64 runs once per file fetch
    // (network-bound, not a hot path despite the #190 lineage) and `clean` is freed
    // immediately after decode. See #233.
    let clean: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD
        .decode(&clean)
        .map_err(|e| GitHubError::Decode(e.to_string()))
}

/// Decode raw bytes into Unicode text.
///
/// Detection priority (highest first; see ADR-0013 for rationale):
/// 1. If `hint` is Some, use the explicit encoding
/// 2. If a BOM is found, use the BOM-identified encoding
/// 3. Run chardetng on full content; if decode succeeds, use detected encoding
/// 4. Fall back to strict UTF-8 validation (AssumedUtf8)
/// 5. If all fail, return NonUtf8 error with retry hint
pub(super) fn decode_bytes(bytes: &[u8], hint: Option<&str>) -> Result<DecodeResult, GitHubError> {
    if let Some(label) = hint {
        return decode_explicit(bytes, label);
    }
    if let Some(result) = decode_bom(bytes) {
        return Ok(result);
    }
    decode_detect(bytes)
}

fn decode_explicit(bytes: &[u8], label: &str) -> Result<DecodeResult, GitHubError> {
    let encoding = Encoding::for_label(label.as_bytes()).ok_or_else(|| {
        GitHubError::NonUtf8(format!(
            "Unknown encoding: '{}'. Valid examples: utf-8, shift_jis, euc-jp, gbk",
            label
        ))
    })?;
    let (decoded, had_errors) = encoding.decode_without_bom_handling(bytes);
    if had_errors {
        return Err(GitHubError::NonUtf8(format!(
            "Could not decode content as '{}'. Retry with --encoding <label>.",
            label
        )));
    }
    Ok(DecodeResult {
        text: decoded.into_owned(),
        encoding: encoding.name().to_ascii_lowercase(),
        source: DetectionSource::Explicit,
    })
}

fn decode_bom(bytes: &[u8]) -> Option<DecodeResult> {
    let (encoding, bom_len) = Encoding::for_bom(bytes)?;
    let (decoded, had_errors) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
    if had_errors {
        debug!(
            encoding = encoding.name(),
            had_errors, "BOM-identified encoding produced replacement characters during decode"
        );
    }
    Some(DecodeResult {
        text: decoded.into_owned(),
        encoding: encoding.name().to_ascii_lowercase(),
        source: DetectionSource::Bom,
    })
}

fn is_likely_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn decode_detect(bytes: &[u8]) -> Result<DecodeResult, GitHubError> {
    // Binary heuristic: null bytes appear in binary files but not in any text encoding
    // (UTF-16 with BOM is already handled by decode_bom before reaching here)
    if is_likely_binary(bytes) {
        return Err(GitHubError::NonUtf8(
            "File appears to be binary (contains null bytes). \
            Use --encoding utf-16le or --encoding utf-16be if this is a UTF-16 file without a BOM."
                .to_owned(),
        ));
    }

    // chardetng runs BEFORE UTF-8 check to prevent silent mojibake (ADR-0013)
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);

    // Only trust chardetng for multi-byte encodings that have strict byte-pattern constraints
    // (Shift_JIS, EUC-JP, GBK, etc.) or UTF-8. Single-byte encodings (windows-1252,
    // windows-1251, iso-8859-*, etc.) accept nearly every byte without errors, so
    // `had_errors == false` carries no reliability signal for them. Fall through to the
    // UTF-8 check for those; if UTF-8 also fails, return a NonUtf8 error.
    if is_reliable_detection(encoding) {
        let (decoded, had_errors) = encoding.decode_without_bom_handling(bytes);
        if !had_errors {
            return Ok(DecodeResult {
                text: decoded.into_owned(),
                encoding: encoding.name().to_ascii_lowercase(),
                source: DetectionSource::Detected,
            });
        }
    }

    // chardetng inconclusive or had errors; try strict UTF-8.
    // Note: with `Utf8Detection::Allow`, chardetng already guesses UTF-8 for any
    // valid-UTF-8 bytes, so this branch is unreachable in practice; it remains as a
    // defensive backstop for the documented detection priority (ADR-0013).
    if let Ok(s) = str::from_utf8(bytes) {
        return Ok(DecodeResult {
            text: s.to_owned(),
            encoding: encoding_rs::UTF_8.name().to_ascii_lowercase(),
            source: DetectionSource::AssumedUtf8,
        });
    }

    // All paths failed; include retry hint with chardetng's best guess (ADR-0013)
    Err(GitHubError::NonUtf8(format!(
        "File encoding could not be decoded. Retry with --encoding {}. \
        Use --encoding to specify the encoding (e.g., --encoding shift_jis, --encoding euc-jp).",
        encoding.name().to_ascii_lowercase()
    )))
}

#[cfg(test)]
mod tests;
