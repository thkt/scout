use crate::fetch::extractor::ExtractedArticle;

/// Hand-authored HTML fixture bypasses Readability.
pub(super) fn article(html: &str) -> ExtractedArticle {
    ExtractedArticle {
        title: None,
        byline: None,
        published_time: None,
        content_html: html.into(),
        used_raw_fallback: false,
    }
}
