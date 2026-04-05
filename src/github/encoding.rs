use base64::{Engine as _, engine::general_purpose::STANDARD};
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;

use super::GitHubError;

/// How the encoding was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionSource {
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
pub struct DecodeResult {
    /// The decoded Unicode text.
    pub text: String,
    /// The encoding label in lowercase (e.g. "shift_jis", "utf-8").
    pub encoding: String,
    /// How the encoding was determined.
    pub source: DetectionSource,
}

/// Decode a base64-encoded string into raw bytes.
///
/// This is the first half of the old `decode_content`: base64 → bytes.
/// Encoding detection is handled separately by `decode_bytes`.
pub fn decode_base64(encoded: &str) -> Result<Vec<u8>, GitHubError> {
    let clean: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD
        .decode(&clean)
        .map_err(|e| GitHubError::Decode(e.to_string()))
}

/// Decode raw bytes into Unicode text.
///
/// Detection priority (BR-003 > BR-002 > BR-001):
/// 1. If `hint` is Some, use the explicit encoding (BR-003)
/// 2. If a BOM is found, use the BOM-identified encoding (BR-002)
/// 3. Run chardetng on full content; if decode succeeds, use detected encoding (BR-001)
/// 4. Fall back to strict UTF-8 validation (AssumedUtf8)
/// 5. If all fail, return NonUtf8 error with retry hint
pub fn decode_bytes(bytes: &[u8], hint: Option<&str>) -> Result<DecodeResult, GitHubError> {
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
    let (decoded, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
    Some(DecodeResult {
        text: decoded.into_owned(),
        encoding: encoding.name().to_ascii_lowercase(),
        source: DetectionSource::Bom,
    })
}

fn decode_detect(bytes: &[u8]) -> Result<DecodeResult, GitHubError> {
    // BR-001: chardetng runs BEFORE UTF-8 check to prevent silent mojibake
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);

    let (decoded, had_errors) = encoding.decode_without_bom_handling(bytes);
    if !had_errors {
        return Ok(DecodeResult {
            text: decoded.into_owned(),
            encoding: encoding.name().to_ascii_lowercase(),
            source: DetectionSource::Detected,
        });
    }

    // FR-007: chardetng had errors; try strict UTF-8 as fallback
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(DecodeResult {
            text: s.to_owned(),
            encoding: encoding_rs::UTF_8.name().to_ascii_lowercase(),
            source: DetectionSource::AssumedUtf8,
        });
    }

    // FR-008: All paths failed; include retry hint with chardetng's best guess
    Err(GitHubError::NonUtf8(format!(
        "File encoding could not be decoded. Retry with --encoding {}. \
        Use --encoding to specify the encoding (e.g., --encoding shift_jis, --encoding euc-jp).",
        encoding.name().to_ascii_lowercase()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── T-001: Explicit Shift_JIS decoding ──

    #[test]
    fn t_001_decode_bytes_with_shift_jis_hint_returns_explicit_result() {
        // [T-001] FR-001, BR-003
        // "テスト" in Shift_JIS
        let bytes: &[u8] = &[0x83, 0x65, 0x83, 0x58, 0x83, 0x67];

        let result = decode_bytes(bytes, Some("shift_jis")).unwrap();

        assert_eq!(result.text, "テスト");
        assert_eq!(result.encoding, "shift_jis");
        assert_eq!(result.source, DetectionSource::Explicit);
    }

    // ── T-002: Explicit EUC-JP decoding ──

    #[test]
    fn t_002_decode_bytes_with_euc_jp_hint_returns_explicit_result() {
        // [T-002] FR-001, BR-003
        // "日本語" in EUC-JP
        let bytes: &[u8] = &[0xC6, 0xFC, 0xCB, 0xDC, 0xB8, 0xEC];

        let result = decode_bytes(bytes, Some("euc-jp")).unwrap();

        assert_eq!(result.text, "日本語");
        assert_eq!(result.encoding, "euc-jp");
        assert_eq!(result.source, DetectionSource::Explicit);
    }

    // ── T-003: Invalid encoding hint ──

    #[test]
    fn t_003_decode_bytes_with_invalid_hint_returns_non_utf8_error() {
        // [T-003] FR-002
        let result = decode_bytes(b"any", Some("zzz-invalid-encoding"));

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            matches!(err, GitHubError::NonUtf8(_)),
            "expected NonUtf8 variant, got: {err:?}"
        );
        assert!(
            msg.contains("zzz-invalid-encoding"),
            "error should contain the invalid label, got: {msg}"
        );
        assert!(
            msg.contains("shift_jis"),
            "error should contain valid example 'shift_jis', got: {msg}"
        );
    }

    // ── T-004: Auto-detect Shift_JIS (chardetng) ──

    #[test]
    fn t_004_decode_bytes_without_hint_detects_shift_jis() {
        // [T-004] FR-004, FR-005
        // "テスト" in Shift_JIS — enough Japanese bytes for chardetng to detect
        let bytes: &[u8] = &[0x83, 0x65, 0x83, 0x58, 0x83, 0x67];

        let result = decode_bytes(bytes, None).unwrap();

        assert_eq!(result.text, "テスト");
        assert_eq!(result.encoding, "shift_jis");
        assert_eq!(result.source, DetectionSource::Detected);
    }

    // ── T-005: ASCII-heavy Shift_JIS detected before UTF-8 (BR-001) ──

    #[test]
    fn t_005_decode_bytes_ascii_heavy_shift_jis_detected_before_utf8() {
        // [T-005] BR-001: chardetng runs BEFORE UTF-8 check
        // Simulate a source file with English comments and Japanese string literals.
        // The ASCII portion is valid UTF-8, but the Shift_JIS bytes are not.
        // chardetng must detect Shift_JIS before UTF-8 is tried.
        let mut bytes = Vec::new();
        // ASCII header (valid UTF-8)
        bytes.extend_from_slice(b"// Copyright 2026 Example Corp.\n");
        bytes.extend_from_slice(b"// Licensed under MIT\n");
        bytes.extend_from_slice(b"fn main() {\n");
        bytes.extend_from_slice(b"    let msg = \"");
        // "テスト" in Shift_JIS (NOT valid UTF-8)
        bytes.extend_from_slice(&[0x83, 0x65, 0x83, 0x58, 0x83, 0x67]);
        bytes.extend_from_slice(b"\";\n");
        bytes.extend_from_slice(b"}\n");

        let result = decode_bytes(&bytes, None).unwrap();

        // The key assertion: source must be Detected (chardetng), not AssumedUtf8.
        // If UTF-8 were tried first, the invalid bytes would cause it to fail
        // and potentially produce a different source or error.
        assert_eq!(
            result.source,
            DetectionSource::Detected,
            "chardetng should detect encoding before UTF-8 fallback"
        );
        assert!(
            result.text.contains("テスト"),
            "decoded text should contain the Japanese string, got: {}",
            result.text
        );
    }

    // ── T-006: UTF-16 BE BOM detection ──

    #[test]
    fn t_006_decode_bytes_with_utf16be_bom_returns_bom_source() {
        // [T-006] FR-003, BR-002
        // UTF-16 BE BOM (FE FF) followed by "AB" in UTF-16 BE
        let bytes: &[u8] = &[
            0xFE, 0xFF, // BOM
            0x00, 0x41, // 'A'
            0x00, 0x42, // 'B'
        ];

        let result = decode_bytes(bytes, None).unwrap();

        assert_eq!(result.source, DetectionSource::Bom);
        assert_eq!(result.encoding, "utf-16be");
        assert!(
            result.text.contains("AB"),
            "decoded text should contain 'AB', got: {:?}",
            result.text
        );
    }

    // ── T-007: Bytes invalid in the specified encoding produce NonUtf8 error ──
    // Spec Evolution: original design tested auto-detect failure, but chardetng always
    // falls back to windows-1252 which encoding_rs decodes without errors (maps undefined
    // bytes to C1 control characters, not U+FFFD). The NonUtf8 path is reliably reached
    // via decode_explicit: user specifies --encoding but the file has invalid bytes for it.

    #[test]
    fn t_007_decode_bytes_random_bytes_returns_non_utf8_with_encoding_hint() {
        // [T-007] FR-002 (bytes invalid for specified encoding → NonUtf8 with retry hint)
        // 0x83 is a valid Shift_JIS lead byte; 0x3F ('?') is NOT a valid trail byte
        // (trail must be 0x40-0x7E or 0x80-0xFC; 0x3F < 0x40).
        // Every pair is an invalid Shift_JIS 2-byte sequence → had_errors=true.
        let bytes: &[u8] = &[
            0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F,
            0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F,
        ];

        let result = decode_bytes(bytes, Some("shift_jis"));

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            matches!(err, GitHubError::NonUtf8(_)),
            "expected NonUtf8 variant, got: {err:?}"
        );
        assert!(
            msg.contains("--encoding"),
            "error should suggest --encoding flag, got: {msg}"
        );
    }

    // ── T-011: NonUtf8 maps to exit code 1 (user_error) ──
    // Note: This test validates the error variant exists and its Display output.
    // The ScoutError mapping test belongs in tools/errors.rs (Phase 2).

    #[test]
    fn t_011_non_utf8_error_contains_descriptive_message() {
        // [T-011] FR-011
        let err = GitHubError::NonUtf8("shift_jis not forced".into());
        let msg = err.to_string();
        assert!(
            msg.contains("shift_jis not forced"),
            "NonUtf8 Display should include the inner message, got: {msg}"
        );
    }

    // ── T-012: Decode (base64) error remains distinct ──

    #[test]
    fn t_012_decode_error_is_distinct_from_non_utf8() {
        // [T-012] FR-012
        let decode_err = GitHubError::Decode("bad base64".into());
        let non_utf8_err = GitHubError::NonUtf8("encoding failed".into());

        // Verify they are different variants with different Display output
        let decode_msg = decode_err.to_string();
        let non_utf8_msg = non_utf8_err.to_string();

        assert!(
            decode_msg.contains("decode error"),
            "Decode variant should contain 'decode error', got: {decode_msg}"
        );
        assert!(
            !non_utf8_msg.contains("decode error"),
            "NonUtf8 variant should NOT contain 'decode error', got: {non_utf8_msg}"
        );
    }

    // ── decode_base64 tests ──

    #[test]
    fn decode_base64_valid_input_returns_bytes() {
        // Validates the base64 → bytes path that was split from decode_content
        let encoded = base64_encode(b"hello world");
        let bytes = decode_base64(&encoded).unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn decode_base64_with_whitespace_succeeds() {
        // GitHub API returns base64 with line breaks
        let encoded = "aGVs\nbG8g\nd29y\nbGQ=\n";
        let bytes = decode_base64(encoded).unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn decode_base64_invalid_input_returns_decode_error() {
        let result = decode_base64("!!!not-base64!!!");

        let err = result.unwrap_err();
        assert!(
            matches!(err, GitHubError::Decode(_)),
            "base64 failure should produce Decode variant, got: {err:?}"
        );
    }

    // ── Helper ──

    fn base64_encode(input: &[u8]) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        STANDARD.encode(input)
    }
}
