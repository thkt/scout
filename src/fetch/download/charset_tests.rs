use super::*;

/// [T-F001] extracts_charset_from_content_type
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

/// [T-F002] returns_none_when_no_charset
#[test]
fn returns_none_when_no_charset() {
    assert!(extract_charset("text/html").is_none());
    assert!(extract_charset("text/plain; boundary=something").is_none());
}

/// [T-F003] decode_body_handles_utf8
#[test]
fn decode_body_handles_utf8() {
    let bytes = "こんにちは".as_bytes();
    assert_eq!(decode_body(bytes, Some("utf-8")), "こんにちは");
    assert_eq!(decode_body(bytes, None), "こんにちは");
}

/// [T-F004] decode_body_handles_shift_jis
#[test]
fn decode_body_handles_shift_jis() {
    let encoding = encoding_rs::SHIFT_JIS;
    let (bytes, _, _) = encoding.encode("テスト");
    assert_eq!(decode_body(&bytes, Some("shift_jis")), "テスト");
}

/// [T-F005] decode_body_handles_euc_jp
#[test]
fn decode_body_handles_euc_jp() {
    let encoding = encoding_rs::EUC_JP;
    let (bytes, _, _) = encoding.encode("日本語");
    assert_eq!(decode_body(&bytes, Some("euc-jp")), "日本語");
}

/// [T-F006] decode_body_falls_back_to_utf8_for_unknown
#[test]
fn decode_body_falls_back_to_utf8_for_unknown() {
    let bytes = "hello".as_bytes();
    assert_eq!(decode_body(bytes, Some("unknown-encoding")), "hello");
}
