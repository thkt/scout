use dom_smoothie::{Config, Readability};
use tracing::warn;

pub(super) struct ExtractedArticle {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub published_time: Option<String>,
    pub content_html: String,
    /// True when readability extraction failed and raw HTML was used as fallback.
    /// False for both successful extraction and explicit raw mode.
    pub used_raw_fallback: bool,
}

pub(super) fn extract_article(html: &str, url: Option<&str>) -> ExtractedArticle {
    let mut readability = match Readability::new(html, url, Some(Config::default())) {
        Ok(r) => r,
        Err(e) => {
            warn!(%e, "readability init failed, using raw fallback");
            return raw_fallback(html);
        }
    };

    let readable = readability.is_probably_readable();

    match readability.parse() {
        Ok(article) => {
            let title = if article.title.is_empty() {
                None
            } else {
                Some(article.title.to_string())
            };

            if readable {
                ExtractedArticle {
                    title,
                    byline: article.byline.map(|b| b.to_string()),
                    published_time: article.published_time.map(|t| t.to_string()),
                    content_html: article.content.to_string(),
                    used_raw_fallback: false,
                }
            } else {
                ExtractedArticle {
                    title,
                    byline: None,
                    published_time: None,
                    content_html: html.to_string(),
                    used_raw_fallback: true,
                }
            }
        }
        Err(e) => {
            warn!(%e, "readability parse failed, using raw fallback");
            raw_fallback(html)
        }
    }
}

pub(super) fn extract_raw(html: &str) -> ExtractedArticle {
    make_raw(html, false)
}

fn raw_fallback(html: &str) -> ExtractedArticle {
    make_raw(html, true)
}

fn make_raw(html: &str, used_raw_fallback: bool) -> ExtractedArticle {
    ExtractedArticle {
        title: extract_title_from_html(html),
        byline: None,
        published_time: None,
        content_html: html.to_string(),
        used_raw_fallback,
    }
}

/// Simple `<title>` tag extraction via string search.
/// Only used as fallback when dom_smoothie fails to parse the HTML.
fn extract_title_from_html(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let tag_start = lower.find("<title")?;
    let content_start = tag_start + lower[tag_start..].find('>')? + 1;
    let content_end = content_start + lower[content_start..].find("</title>")?;
    let title = html[content_start..content_end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOG_HTML: &str = r#"
<!DOCTYPE html>
<html>
<head><title>Test Blog Post</title></head>
<body>
<nav>Navigation links here</nav>
<article>
    <h1>Understanding Rust Ownership</h1>
    <p class="author">By John Doe</p>
    <p>Rust's ownership system is one of its most unique features.
    It enables memory safety without garbage collection.
    The ownership rules are checked at compile time.</p>
    <p>Each value in Rust has a variable that's called its owner.
    There can only be one owner at a time.
    When the owner goes out of scope, the value will be dropped.</p>
    <p>This is a fundamental concept that every Rust programmer must understand.
    It affects how you write functions, handle data structures, and manage memory.</p>
    <p>Let's explore the three rules of ownership in detail and see how they
    work together to make Rust programs safe and efficient.</p>
    <p>The borrow checker enforces these rules at compile time, ensuring that
    references are always valid and that data races are impossible.</p>
</article>
<footer>Site footer</footer>
</body>
</html>"#;

    #[test]
    fn extracts_article_content() {
        let result = extract_article(BLOG_HTML, None);

        assert!(!result.used_raw_fallback);
        assert!(result.content_html.contains("ownership"));
    }

    #[test]
    fn raw_mode_returns_full_html() {
        let result = extract_raw(BLOG_HTML);

        assert!(!result.used_raw_fallback);
        assert!(result.content_html.contains("<nav>"));
        assert!(result.content_html.contains("<footer>"));
    }

    #[test]
    fn falls_back_to_raw_on_minimal_html() {
        let minimal = "<html><body><p>hi</p></body></html>";
        let result = extract_article(minimal, None);

        assert!(result.used_raw_fallback);
        assert!(result.content_html.contains("hi"));
    }

    #[test]
    fn extracts_title_from_html_tag() {
        let html = "<html><head><title>My Page</title></head><body></body></html>";
        assert_eq!(extract_title_from_html(html), Some("My Page".to_string()));
    }

    #[test]
    fn title_extraction_returns_none_for_empty() {
        let html = "<html><head><title></title></head><body></body></html>";
        assert_eq!(extract_title_from_html(html), None);
    }

    #[test]
    fn title_extraction_returns_none_when_missing() {
        let html = "<html><head></head><body></body></html>";
        assert_eq!(extract_title_from_html(html), None);
    }

    #[test]
    fn title_extraction_handles_attributes() {
        let html = r#"<html><head><title lang="en">Attributed Title</title></head></html>"#;
        assert_eq!(
            extract_title_from_html(html),
            Some("Attributed Title".to_string())
        );
    }

    #[test]
    fn fallback_still_extracts_title_from_minimal_html() {
        let html = "<html><head><title>Minimal Page</title></head><body><p>hi</p></body></html>";
        let result = extract_article(html, None);

        assert!(result.used_raw_fallback);
        assert_eq!(result.title, Some("Minimal Page".to_string()));
    }

    #[test]
    fn title_extraction_handles_multibyte() {
        let html = "<html><head><title>日本語タイトル</title></head><body></body></html>";
        assert_eq!(
            extract_title_from_html(html),
            Some("日本語タイトル".to_string())
        );
    }

    #[test]
    fn title_extraction_safe_with_unicode_case_expansion() {
        // Turkish İ (U+0130) expands from 2→3 bytes under full to_lowercase().
        // to_ascii_lowercase preserves byte offsets, preventing panic on slice.
        let html = "<html><head><TITLE>My Title</TITLE></head><body>İİİ</body></html>";
        assert_eq!(extract_title_from_html(html), Some("My Title".to_string()));
    }
}
