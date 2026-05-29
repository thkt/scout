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
