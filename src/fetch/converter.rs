use super::extractor::ExtractedArticle;

#[derive(Debug)]
pub struct FetchResult {
    pub url: String,
    pub markdown: String,
    pub used_raw_fallback: bool,
}

struct Metadata {
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
}

pub fn to_fetch_result(
    article: ExtractedArticle,
    url: String,
    include_meta: bool,
) -> FetchResult {
    let markdown = html2md::rewrite_html(&article.content_html, false);

    let output = if include_meta {
        let meta = Metadata {
            title: article.title,
            author: article.byline,
            date: article.published_time,
        };
        format_with_frontmatter(&meta, &markdown)
    } else {
        markdown
    };

    FetchResult {
        url,
        markdown: output,
        used_raw_fallback: article.used_raw_fallback,
    }
}

fn format_with_frontmatter(meta: &Metadata, markdown: &str) -> String {
    let mut fm = String::from("---\n");

    if let Some(title) = &meta.title {
        fm.push_str(&format!("title: \"{}\"\n", escape_yaml(title)));
    }
    if let Some(author) = &meta.author {
        fm.push_str(&format!("author: \"{}\"\n", escape_yaml(author)));
    }
    if let Some(date) = &meta.date {
        fm.push_str(&format!("date: \"{}\"\n", escape_yaml(date)));
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
    }
}
