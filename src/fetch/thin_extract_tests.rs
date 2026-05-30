use super::*;
use extractor::ExtractedArticle;

fn article(content_html: &str, used_raw_fallback: bool) -> ExtractedArticle {
    ExtractedArticle {
        title: None,
        byline: None,
        published_time: None,
        content_html: content_html.to_owned(),
        used_raw_fallback,
    }
}

/// [T-F033] raw_fallback_with_short_content_is_thin
#[test]
fn raw_fallback_with_short_content_is_thin() {
    assert!(is_thin_extract(&article("<p>short</p>", true)));
}

/// [T-F034] raw_fallback_with_rich_content_still_thin
#[test]
fn raw_fallback_with_rich_content_still_thin() {
    let content = format!("<p>{}</p>", "x".repeat(100));
    assert!(is_thin_extract(&article(&content, true)));
}

/// [T-F035] short_content_is_thin
#[test]
fn short_content_is_thin() {
    assert!(is_thin_extract(&article("<p>hi</p>", false)));
}

/// [T-F036] sufficient_content_is_not_thin
#[test]
fn sufficient_content_is_not_thin() {
    let content = format!("<p>{}</p>", "x".repeat(100));
    assert!(!is_thin_extract(&article(&content, false)));
}

/// [T-F037] exactly_at_threshold_is_not_thin (extract)
#[test]
fn exactly_at_threshold_is_not_thin() {
    let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD));
    assert!(!is_thin_extract(&article(&content, false)));
}

/// [T-F038] just_below_threshold_is_thin (extract)
#[test]
fn just_below_threshold_is_thin() {
    let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD - 1));
    assert!(is_thin_extract(&article(&content, false)));
}

/// [T-F039] html_tags_excluded_from_count
#[test]
fn html_tags_excluded_from_count() {
    let content = r#"<div class="very-long-class-name"><span>ab</span></div>"#;
    assert!(is_thin_extract(&article(content, false)));
}

/// [T-F040] whitespace_excluded_from_count
#[test]
fn whitespace_excluded_from_count() {
    let content = format!("<p>{}</p>", " x ".repeat(30));
    assert!(is_thin_extract(&article(&content, false)));
}
