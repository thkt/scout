use super::*;

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
