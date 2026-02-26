use std::fmt::Write;

use super::extractor::ExtractedArticle;

/// Fetched page content converted to Markdown.
#[derive(Debug)]
pub struct FetchResult {
    pub url: String,
    pub markdown: String,
    pub used_raw_fallback: bool,
}

pub(super) fn to_fetch_result(
    article: ExtractedArticle,
    url: String,
    include_meta: bool,
) -> FetchResult {
    let markdown = html2md::rewrite_html(&article.content_html, false);

    let output = if include_meta {
        format_with_frontmatter(&article, &markdown)
    } else {
        markdown
    };

    FetchResult {
        url,
        markdown: output,
        used_raw_fallback: article.used_raw_fallback,
    }
}

fn format_with_frontmatter(article: &ExtractedArticle, markdown: &str) -> String {
    let mut fm = String::from("---\n");

    if let Some(title) = &article.title {
        let _ = writeln!(fm, "title: \"{}\"", escape_yaml(title));
    }
    if let Some(author) = &article.byline {
        let _ = writeln!(fm, "author: \"{}\"", escape_yaml(author));
    }
    if let Some(date) = &article.published_time {
        let _ = writeln!(fm, "date: \"{}\"", escape_yaml(date));
    }

    fm.push_str("---\n\n");
    fm.push_str(markdown);
    fm
}

fn escape_yaml(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\0', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fetch_result_without_meta() {
        let article = ExtractedArticle {
            title: Some("Test".into()),
            byline: Some("Author".into()),
            published_time: None,
            content_html: "<p>Content</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(article, "https://example.com".into(), false);

        assert!(!result.markdown.contains("---"));
        assert!(result.markdown.contains("Content"));
    }

    #[test]
    fn to_fetch_result_with_meta() {
        let article = ExtractedArticle {
            title: Some("My Title".into()),
            byline: Some("Jane Doe".into()),
            published_time: Some("2026-01-15".into()),
            content_html: "<p>Body text</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(article, "https://example.com".into(), true);

        assert!(result.markdown.contains("---"));
        assert!(result.markdown.contains("title: \"My Title\""));
        assert!(result.markdown.contains("author: \"Jane Doe\""));
        assert!(result.markdown.contains("date: \"2026-01-15\""));
        assert!(result.markdown.contains("Body text"));
    }

    #[test]
    fn frontmatter_omits_missing_fields() {
        let article = ExtractedArticle {
            title: Some("Only Title".into()),
            byline: None,
            published_time: None,
            content_html: "<p>Text</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(article, "https://example.com".into(), true);

        assert!(result.markdown.contains("title: \"Only Title\""));
        assert!(!result.markdown.contains("author:"));
        assert!(!result.markdown.contains("date:"));
    }

    #[test]
    fn escapes_yaml_special_chars() {
        assert_eq!(escape_yaml(r#"He said "hello""#), r#"He said \"hello\""#);
        assert_eq!(escape_yaml(r"back\slash"), r"back\\slash");
        assert_eq!(escape_yaml("line\nbreak"), "line\\nbreak");
        assert_eq!(escape_yaml("cr\rreturn"), "cr\\rreturn");
        assert_eq!(escape_yaml("tab\there"), "tab\\there");
        assert_eq!(escape_yaml("null\0byte"), "nullbyte");
    }
}
