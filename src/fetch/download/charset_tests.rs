use super::*;

/// [T-F001]
#[test]
fn extracts_charset_from_content_type() {
    assert_eq!(
        extract_charset("text/html; charset=utf-8").as_deref(),
        Some("utf-8")
    );
    assert_eq!(
        extract_charset("text/html; charset=Shift_JIS").as_deref(),
        Some("shift_jis")
    );
    assert_eq!(
        extract_charset("text/html; charset=\"EUC-KR\"").as_deref(),
        Some("euc-kr")
    );
}

/// [T-F002]
#[test]
fn returns_none_when_no_charset() {
    assert!(extract_charset("text/html").is_none());
    assert!(extract_charset("text/plain; boundary=something").is_none());
}

/// [T-F003]
#[test]
fn decode_body_handles_utf8() {
    let labeled = decode_body("こんにちは".as_bytes(), Some("utf-8"));
    assert_eq!(labeled.text, "こんにちは");
    assert!(!labeled.uncertain);

    let unlabeled = decode_body("こんにちは".as_bytes(), None);
    assert_eq!(unlabeled.text, "こんにちは");
    assert!(!unlabeled.uncertain);
}

/// [T-F004]
#[test]
fn decode_body_handles_shift_jis() {
    let encoding = encoding_rs::SHIFT_JIS;
    let (bytes, _, _) = encoding.encode("テスト");
    let decoded = decode_body(&bytes, Some("shift_jis"));
    assert_eq!(decoded.text, "テスト");
    assert!(!decoded.uncertain);
}

/// [T-F005]
#[test]
fn decode_body_handles_euc_jp() {
    let encoding = encoding_rs::EUC_JP;
    let (bytes, _, _) = encoding.encode("日本語");
    let decoded = decode_body(&bytes, Some("euc-jp"));
    assert_eq!(decoded.text, "日本語");
    assert!(!decoded.uncertain);
}

/// [T-F006]
#[test]
fn decode_body_falls_back_to_utf8_for_unknown() {
    let decoded = decode_body(b"hello", Some("unknown-encoding"));
    assert_eq!(decoded.text, "hello");
    assert!(!decoded.uncertain);
}

/// [T-F068] shift_jis body mislabeled as utf-8 is recovered by detection and not uncertain
#[test]
fn decode_body_recovers_shift_jis_mislabeled_as_utf8() {
    let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(
        "これはシフトジスでエンコードされた日本語の本文です。誤ったラベルでも復元されます。",
    );
    let decoded = decode_body(&bytes, Some("utf-8"));
    assert_eq!(
        decoded.text,
        "これはシフトジスでエンコードされた日本語の本文です。誤ったラベルでも復元されます。"
    );
    assert!(!decoded.uncertain);
}

/// [T-F061] correctly labeled single-byte iso-8859-1 decodes without regression
#[test]
fn decode_body_decodes_correctly_labeled_iso_8859_1() {
    // In ISO-8859-1 each byte maps to the same Unicode scalar: 0xE9 to e-acute, 0xF9 to u-grave.
    let bytes = [b'c', b'a', b'f', 0xE9, b' ', b'o', b'u', 0xF9];
    let decoded = decode_body(&bytes, Some("iso-8859-1"));
    assert_eq!(decoded.text, "caf\u{E9} ou\u{F9}");
    assert!(!decoded.uncertain);
}

/// [T-F062] valid UTF-8 (labeled and unlabeled) decodes cleanly and is not uncertain
#[test]
fn decode_body_decodes_valid_utf8_clean() {
    let labeled = decode_body("hello world".as_bytes(), Some("utf-8"));
    assert_eq!(labeled.text, "hello world");
    assert!(!labeled.uncertain);

    let unlabeled = decode_body("hello world".as_bytes(), None);
    assert_eq!(unlabeled.text, "hello world");
    assert!(!unlabeled.uncertain);
}

/// [T-F063] windows-1252 single-byte content mislabeled utf-8 returns lossy body and is uncertain
#[test]
fn decode_body_marks_undecodable_bytes_uncertain() {
    // Windows-1252 smart quotes/dashes (0x92/0x93/0x94/0x97) are invalid UTF-8 and
    // carry no BOM, so the labeled decode fails. chardetng then guesses a single-byte
    // encoding, which the reliability gate refuses, so the body is flagged uncertain
    // rather than silently trusted as mojibake.
    let bytes =
        b"It\x92s a nice day, isn\x92t it? \x93quoted\x94 and an \x97 em dash, plus more text.";
    let decoded = decode_body(bytes, Some("utf-8"));
    assert!(decoded.uncertain);
    // The body is still returned (lossy), never empty (issue #241: exit 0 + body).
    assert!(!decoded.text.is_empty());
}

/// [T-F064] mostly-valid UTF-8 with a few corrupt bytes returns a lossy body, uncertain, no hard fail
#[test]
fn decode_body_incidental_corruption_is_lossy_not_fatal() {
    let mut bytes = "valid prefix text that is mostly fine ".as_bytes().to_vec();
    bytes.push(0xFF); // a single torn byte
    bytes.extend_from_slice(" and a valid suffix".as_bytes());
    let decoded = decode_body(&bytes, Some("utf-8"));
    assert!(decoded.uncertain);
    assert!(decoded.text.contains("valid prefix text"));
    assert!(decoded.text.contains("and a valid suffix"));
}

/// [T-F065] unknown charset label with recoverable multi-byte body is detected and not uncertain
#[test]
fn decode_body_recovers_multibyte_under_unknown_label() {
    let (bytes, _, _) = encoding_rs::EUC_JP
        .encode("この本文は未知のラベルが付いていますが、検知によって正しく復元されます。日本語の文章です。");
    let decoded = decode_body(&bytes, Some("x-unknown-charset"));
    assert_eq!(
        decoded.text,
        "この本文は未知のラベルが付いていますが、検知によって正しく復元されます。日本語の文章です。"
    );
    assert!(!decoded.uncertain);
}

/// [T-F066] correctly labeled euc-jp decodes cleanly and is not uncertain (regression guard)
#[test]
fn decode_body_decodes_correctly_labeled_euc_jp() {
    let (bytes, _, _) = encoding_rs::EUC_JP.encode("日本語テキスト");
    let decoded = decode_body(&bytes, Some("euc-jp"));
    assert_eq!(decoded.text, "日本語テキスト");
    assert!(!decoded.uncertain);
}
