use super::*;

// ── Explicit Shift_JIS decoding ──

/// [T-GE001]
#[test]
fn decode_bytes_with_shift_jis_hint_returns_explicit_result() {
    // Priority 1 (highest): an explicit --encoding hint (ADR-0013 detection priority)
    // "テスト" in Shift_JIS
    let bytes: &[u8] = &[0x83, 0x65, 0x83, 0x58, 0x83, 0x67];

    let result = decode_bytes(bytes, Some("shift_jis")).unwrap();

    assert_eq!(result.text, "テスト");
    assert_eq!(result.encoding, "shift_jis");
    assert_eq!(result.source, DetectionSource::Explicit);
}

// ── Explicit EUC-JP decoding ──

/// [T-GE002]
#[test]
fn decode_bytes_with_euc_jp_hint_returns_explicit_result() {
    // Priority 1 (highest): an explicit --encoding hint (ADR-0013 detection priority)
    // "日本語" in EUC-JP
    let bytes: &[u8] = &[0xC6, 0xFC, 0xCB, 0xDC, 0xB8, 0xEC];

    let result = decode_bytes(bytes, Some("euc-jp")).unwrap();

    assert_eq!(result.text, "日本語");
    assert_eq!(result.encoding, "euc-jp");
    assert_eq!(result.source, DetectionSource::Explicit);
}

// ── Invalid encoding hint ──

/// [T-GE003] decode_bytes with unknown encoding hint returns NonUtf8 error with retry guidance
#[test]
fn decode_bytes_with_invalid_hint_returns_non_utf8_error() {
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

// ── Auto-detect Shift_JIS (chardetng) ──

/// [T-GE004]
#[test]
fn decode_bytes_without_hint_detects_shift_jis() {
    // "テスト" in Shift_JIS — enough Japanese bytes for chardetng to detect
    let bytes: &[u8] = &[0x83, 0x65, 0x83, 0x58, 0x83, 0x67];

    let result = decode_bytes(bytes, None).unwrap();

    assert_eq!(result.text, "テスト");
    assert_eq!(result.encoding, "shift_jis");
    assert_eq!(result.source, DetectionSource::Detected);
}

// ── ASCII-heavy Shift_JIS detected before UTF-8 ──

/// [T-GE005]
#[test]
fn decode_bytes_ascii_heavy_shift_jis_detected_before_utf8() {
    // Simulate a source file with English comments and Japanese string literals.
    // The ASCII portion is valid UTF-8, but the Shift_JIS bytes are not.
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

    // Source must be Detected (chardetng), not AssumedUtf8.
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

// ── UTF-16 BE BOM detection ──

/// [T-GE006]
#[test]
fn decode_bytes_with_utf16be_bom_returns_bom_source() {
    // Priority 2: a BOM, when no --encoding hint was given (ADR-0013 detection priority)
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

// ── Bytes invalid in the specified encoding produce NonUtf8 error ──
// Spec Evolution: original design tested auto-detect failure, but chardetng always
// falls back to windows-1252 which encoding_rs decodes without errors (maps undefined
// bytes to C1 control characters, not U+FFFD). The NonUtf8 path is reliably reached
// via decode_explicit: user specifies --encoding but the file has invalid bytes for it.

/// [T-GE007] decode_bytes returns NonUtf8 with --encoding hint when explicit encoding fails
#[test]
fn decode_bytes_random_bytes_returns_non_utf8_with_encoding_hint() {
    // bytes invalid for specified encoding → NonUtf8 with retry hint
    // 0x83 is a valid Shift_JIS lead byte; 0x3F ('?') is NOT a valid trail byte
    // (trail must be 0x40-0x7E or 0x80-0xFC; 0x3F < 0x40).
    // Every pair is an invalid Shift_JIS 2-byte sequence → had_errors=true.
    let bytes: &[u8] = &[
        0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83, 0x3F, 0x83,
        0x3F,
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

// ── Binary file (null bytes) returns NonUtf8 error ──

/// [T-GE008]
#[test]
fn decode_bytes_with_null_bytes_returns_non_utf8_error() {
    // Without the null-byte guard, chardetng would guess windows-1252 and return Detected with garbage text
    let bytes: &[u8] = &[0x7F, 0x45, 0x4C, 0x46, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00];

    let result = decode_bytes(bytes, None);

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        matches!(err, GitHubError::NonUtf8(_)),
        "expected NonUtf8 variant for binary input, got: {err:?}"
    );
    assert!(
        msg.contains("binary"),
        "error should mention binary, got: {msg}"
    );
}

// ── Non-NUL random bytes (windows-1252 fallback) return NonUtf8 error ──

/// [T-GE009]
#[test]
fn decode_bytes_non_nul_random_bytes_return_non_utf8_error() {
    // Single-byte encoding guard: chardetng can return windows-1251, windows-1252,
    // or other single-byte encodings for arbitrary non-NUL bytes. Those encodings accept
    // every byte without error, so `had_errors == false` is meaningless as a quality signal.
    // The `is_reliable_detection` guard must reject single-byte encodings and fall through
    // to the UTF-8 check (which fails here), ultimately returning NonUtf8.
    //
    // Byte choice rationale (0xFD/0xFF alternating):
    //   UTF-8:     always invalid (0xFD and 0xFF are permanently unused code points)
    //   Shift_JIS: 0xFD is undefined (lead bytes only go to 0xFC)
    //   GBK:       0xFF is an invalid trail byte (trail range is 0x40-0xFE)
    //   EUC-JP:    0xFF is an invalid second byte (valid range is 0xA1-0xFE)
    //   windows-1251/1252: valid (0xFD='э'/'ý', 0xFF='я'/'ÿ') → chardetng returns one of these
    //
    // Without the guard this would return Ok(Detected) with garbled Cyrillic/Latin text.
    let bytes: &[u8] = &[
        0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD,
        0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF, 0xFD, 0xFF,
    ];

    let result = decode_bytes(bytes, None);

    assert!(
        result.is_err(),
        "non-NUL random bytes should return an error, not Detected"
    );
    assert!(
        matches!(result.unwrap_err(), GitHubError::NonUtf8(_)),
        "error should be NonUtf8 variant"
    );
}

// ── NonUtf8 Display output ──
// The ScoutError mapping test belongs in tools/errors.rs.

/// [T-GE010]
#[test]
fn non_utf8_error_contains_descriptive_message() {
    let err = GitHubError::NonUtf8("shift_jis not forced".into());
    let msg = err.to_string();
    assert!(
        msg.contains("shift_jis not forced"),
        "NonUtf8 Display should include the inner message, got: {msg}"
    );
}

// ── Decode (base64) error remains distinct ──

/// [T-GE011] GitHubError::Decode and NonUtf8 variants produce distinct Display output
#[test]
fn decode_error_is_distinct_from_non_utf8() {
    let decode_err = GitHubError::Decode("bad base64".into());
    let non_utf8_err = GitHubError::NonUtf8("encoding failed".into());

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

/// [T-GE012]
#[test]
fn decode_base64_valid_input_returns_bytes() {
    // Validates the base64 → bytes path that was split from decode_content
    let encoded = base64_encode(b"hello world");
    let bytes = decode_base64(&encoded).unwrap();
    assert_eq!(bytes, b"hello world");
}

/// [T-GE013]
#[test]
fn decode_base64_with_whitespace_succeeds() {
    // GitHub API returns base64 with line breaks
    let encoded = "aGVs\nbG8g\nd29y\nbGQ=\n";
    let bytes = decode_base64(encoded).unwrap();
    assert_eq!(bytes, b"hello world");
}

/// [T-GE014]
#[test]
fn decode_base64_invalid_input_returns_decode_error() {
    let result = decode_base64("!!!not-base64!!!");

    let err = result.unwrap_err();
    assert!(
        matches!(err, GitHubError::Decode(_)),
        "base64 failure should produce Decode variant, got: {err:?}"
    );
}

// ── Fallback logging ──

/// [T-GE015] a BOM whose bytes do not decode is an error, not a lossy success
///
/// The three decode paths can disagree about `had_errors`: `decode_explicit`
/// fails on it, `decode_detect` falls through to the next strategy, and
/// `decode_bom` returns the replacement characters under
/// `DetectionSource::Bom`. Left to disagree, the weakest declaration (a BOM in
/// the file) becomes the most permissive, and the caller reads a settled
/// encoding off a mojibake body — the GitHub path has no counterpart to fetch's
/// `decode_uncertain` to signal otherwise. ADR-0013 ends this path in a
/// `NonUtf8` error with a retry hint.
#[tracing_test::traced_test]
#[test]
fn decode_bom_that_does_not_decode_is_an_error() {
    // UTF-16BE BOM (FE FF) + lone high surrogate (D8 00) with no trailing low
    // surrogate → encoding_rs substitutes U+FFFD and sets had_errors=true.
    let bytes: &[u8] = &[0xFE, 0xFF, 0xD8, 0x00];

    let err = decode_bytes(bytes, None).expect_err("a BOM that does not decode must fail");

    let message = err.to_string();
    assert!(
        message.contains("UTF-16BE") && message.contains("--encoding"),
        "error must name the declared encoding and how to retry, got: {message}"
    );
    assert!(
        logs_contain("BOM-identified encoding produced replacement characters"),
        "expected the BOM replacement-character debug event"
    );
    assert!(logs_contain("DEBUG"), "event level should be DEBUG");
    assert!(
        logs_contain("had_errors=true"),
        "had_errors field should be true"
    );
}

/// [T-GE016] a BOM whose bytes decode cleanly still returns the Bom source
///
/// The companion to T-GE015: tightening the failure case must not turn the
/// ordinary BOM path into an error.
#[test]
fn decode_bom_that_decodes_cleanly_still_succeeds() {
    // UTF-16BE BOM (FE FF) + "Hi" in UTF-16BE.
    let bytes: &[u8] = &[0xFE, 0xFF, 0x00, 0x48, 0x00, 0x69];

    let result = decode_bytes(bytes, None).expect("a well-formed BOM body must decode");

    assert_eq!(result.text, "Hi");
    assert_eq!(result.source, DetectionSource::Bom);
    assert_eq!(result.encoding, "utf-16be");
}

// ── Helper ──

fn base64_encode(input: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.encode(input)
}
