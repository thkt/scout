//! Shared charset-detection helpers used by both the GitHub and fetch paths.

/// Returns true for encodings that chardetng can reliably detect.
///
/// Multi-byte encodings have strict byte-pattern constraints, so a successful decode
/// is meaningful evidence. Single-byte encodings (windows-1252, windows-1251, iso-8859-*,
/// etc.) accept nearly every byte, making `had_errors == false` an unreliable signal.
pub(crate) fn is_reliable_detection(encoding: &'static encoding_rs::Encoding) -> bool {
    [
        encoding_rs::UTF_8,
        encoding_rs::SHIFT_JIS,
        encoding_rs::EUC_JP,
        encoding_rs::ISO_2022_JP,
        encoding_rs::BIG5,
        encoding_rs::GBK,
        encoding_rs::GB18030,
        encoding_rs::EUC_KR,
    ]
    .contains(&encoding)
}
