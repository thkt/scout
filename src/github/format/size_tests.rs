use super::*;

/// [T-GF001] format_size returns byte suffix for values under 1 KiB
#[test]
fn format_size_bytes() {
    assert_eq!(format_size(500), "500 B");
}

/// [T-GF002] format_size returns KB suffix with one decimal place
#[test]
fn format_size_kilobytes() {
    assert_eq!(format_size(1536), "1.5 KB");
}

/// [T-GF003] format_size returns MB suffix with one decimal place
#[test]
fn format_size_megabytes() {
    assert_eq!(format_size(2_621_440), "2.5 MB");
}

/// [T-GF037] format_size renders zero as "0 B" (the empty-file boundary).
#[test]
fn format_size_zero_bytes() {
    assert_eq!(format_size(0), "0 B");
}

/// [T-GF038] format_size keeps the byte suffix at the top of the byte tier
/// (1023 = 1 KiB − 1, the value just below the KB boundary).
#[test]
fn format_size_byte_tier_upper_boundary() {
    assert_eq!(format_size(1023), "1023 B");
}

/// [T-GF039] format_size switches to the KB suffix at exactly 1 KiB (1024),
/// the lower edge of the KB tier where `bytes < 1024` first turns false.
#[test]
fn format_size_kilobyte_tier_lower_boundary() {
    assert_eq!(format_size(1024), "1.0 KB");
}

/// [T-GF040] At 1 MiB − 1 (1_048_575) the value is still in the KB tier, and
/// one-decimal rounding surfaces "1024.0 KB" rather than rolling over to MB:
/// 1_048_575 / 1024 = 1023.999… rounds up to 1024.0. The MB tier begins only
/// at `bytes >= 1024 * 1024`.
#[test]
fn format_size_kilobyte_tier_upper_boundary_rounds_to_1024() {
    assert_eq!(format_size(1_048_575), "1024.0 KB");
}

/// [T-GF041] format_size switches to the MB suffix at exactly 1 MiB
/// (1_048_576), the lower edge of the MB tier.
#[test]
fn format_size_megabyte_tier_lower_boundary() {
    assert_eq!(format_size(1_048_576), "1.0 MB");
}
