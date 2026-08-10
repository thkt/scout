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

#[cfg(test)]
mod tests {
    use super::is_reliable_detection;

    /// [T-CS001] every encoding the gate trusts, named one by one
    ///
    /// ADR-0013 calls this list the single source of truth and says the two
    /// decode paths pin it indirectly. They pin part of it: `ISO_2022_JP`,
    /// `BIG5` and `GB18030` appear in no test on either path, so dropping them
    /// here would silently turn a clean Japanese, Traditional Chinese or
    /// Simplified Chinese decode into a `DECODE_UNCERTAIN` body without a
    /// single failure.
    #[test]
    fn trusts_exactly_the_eight_multi_byte_encodings() {
        for encoding in [
            encoding_rs::UTF_8,
            encoding_rs::SHIFT_JIS,
            encoding_rs::EUC_JP,
            encoding_rs::ISO_2022_JP,
            encoding_rs::BIG5,
            encoding_rs::GBK,
            encoding_rs::GB18030,
            encoding_rs::EUC_KR,
        ] {
            assert!(
                is_reliable_detection(encoding),
                "{} must stay trusted (ADR-0013)",
                encoding.name()
            );
        }
    }

    /// [T-CS002] single-byte encodings are not trusted
    ///
    /// These accept nearly every byte sequence, so `had_errors == false` says
    /// nothing about whether the guess was right — which is the whole reason the
    /// gate exists rather than trusting chardetng outright.
    #[test]
    fn rejects_single_byte_encodings() {
        for encoding in [
            encoding_rs::WINDOWS_1252,
            encoding_rs::WINDOWS_1251,
            encoding_rs::ISO_8859_2,
            encoding_rs::KOI8_U,
            encoding_rs::MACINTOSH,
        ] {
            assert!(
                !is_reliable_detection(encoding),
                "{} accepts almost any byte and must not be trusted",
                encoding.name()
            );
        }
    }

    /// [T-CS003] UTF-16 is not trusted either
    ///
    /// It is multi-byte but carries no in-band constraint that a mis-guess would
    /// violate, and neither path feeds it: the fetch side decodes by label and
    /// the GitHub side reads a BOM first.
    #[test]
    fn rejects_utf16() {
        assert!(!is_reliable_detection(encoding_rs::UTF_16LE));
        assert!(!is_reliable_detection(encoding_rs::UTF_16BE));
    }
}
