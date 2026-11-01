use crate::fetch::extractor::ExtractedArticle;

/// Hand-authored HTML fixture bypasses Readability. Class-driven behavior here
/// therefore also applies to raw fetch; normal extraction strips classes.
pub(super) fn article(html: &str) -> ExtractedArticle {
    ExtractedArticle {
        title: None,
        byline: None,
        published_time: None,
        content_html: html.into(),
        used_raw_fallback: false,
    }
}
