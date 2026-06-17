use std::borrow::Cow;
use std::fmt::Write;

use serde::Serialize;

use super::extractor::ExtractedArticle;

/// Fetched page content converted to Markdown. Fields are private so the only
/// construction paths are [`to_fetch_result`] (production) and
/// [`FetchResult::for_test`] (test fixtures); callers cannot build a result
/// that bypasses Readability extraction or skips frontmatter rendering.
#[derive(Debug, Serialize)]
pub(crate) struct FetchResult {
    url: String,
    markdown: String,
    /// Internal flag: surfaced as a `notes` entry in scout's JSON output, not as data.
    #[serde(skip_serializing)]
    used_raw_fallback: bool,
    /// Internal flag (issue #241): the body could not be decoded cleanly, so the
    /// markdown is a best-effort lossy rendering. Surfaced as `DECODE_UNCERTAIN`
    /// in `degraded_reasons`, not as data.
    #[serde(skip_serializing)]
    decode_uncertain: bool,
}

impl FetchResult {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn markdown(&self) -> &str {
        &self.markdown
    }

    pub(crate) fn used_raw_fallback(&self) -> bool {
        self.used_raw_fallback
    }

    pub(crate) fn decode_uncertain(&self) -> bool {
        self.decode_uncertain
    }

    /// Test-only constructor. Production code goes through [`to_fetch_result`].
    #[cfg(test)]
    pub(crate) fn for_test(url: String, markdown: String, used_raw_fallback: bool) -> Self {
        Self {
            url,
            markdown,
            used_raw_fallback,
            decode_uncertain: false,
        }
    }

    /// Test-only builder to flag a page as decode-uncertain without widening
    /// [`for_test`] into boolean-blind positional args.
    #[cfg(test)]
    pub(crate) fn with_decode_uncertain(mut self, decode_uncertain: bool) -> Self {
        self.decode_uncertain = decode_uncertain;
        self
    }
}

pub(crate) const RAW_FALLBACK_NOTE: &str =
    "> Note: Readability extraction failed. Showing raw page conversion.\n\n";

pub(crate) const DECODE_UNCERTAIN_NOTE: &str = "> Note: Character encoding could not be determined; the body is a best-effort decode and may be garbled.\n\n";

pub(super) fn to_fetch_result(
    article: &ExtractedArticle,
    url: String,
    decode_uncertain: bool,
) -> FetchResult {
    let markdown = html2md::rewrite_html(&article.content_html, false);
    let output = format_with_frontmatter(article, &markdown);

    FetchResult {
        url,
        markdown: output,
        used_raw_fallback: article.used_raw_fallback,
        decode_uncertain,
    }
}

fn format_with_frontmatter(article: &ExtractedArticle, markdown: &str) -> String {
    let mut fm = String::from("---\n");

    if let Some(title) = &article.title {
        let _ = writeln!(fm, "title: \"{}\"", escape_yaml(title));
    }
    // "byline" is the Readability/journalism term; mapped to "author" for YAML frontmatter
    if let Some(author) = &article.byline {
        let _ = writeln!(fm, "author: \"{}\"", escape_yaml(author));
    }
    if let Some(date) = &article.published_time {
        let _ = writeln!(fm, "date: \"{}\"", escape_yaml(date));
    }

    fm.push_str("---\n\n");
    // The body is untrusted page content appended after the frontmatter; neutralize
    // any column-0 `---`/`...` so it cannot inject a YAML document boundary.
    fm.push_str(&neutralize_yaml_markers(markdown));
    fm
}

/// Neutralize YAML document markers in untrusted body text appended after a
/// `---`-delimited frontmatter block.  A line that is exactly `---` or `...` (a
/// YAML document start/end marker, and also a Markdown thematic break) is rewritten
/// to `***`, which renders as the same thematic break but is not a YAML marker, so
/// the body cannot inject a document boundary or a forged frontmatter block.  Only
/// column-0 markers are rewritten; indented or inline `---` is ordinary content and
/// left intact.
pub(crate) fn neutralize_yaml_markers(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match yaml_marker_rest(line) {
            // Bare marker (only trailing whitespace): collapse to `***`.
            Some(rest) if rest.trim_matches([' ', '\t', '\r']).is_empty() => out.push_str("***"),
            // Marker with content (`--- evil: true`): rewrite the leading token only,
            // preserving the rest as ordinary text.
            Some(rest) => {
                out.push_str("***");
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// If `line` is a YAML document marker (`---` start or `...` end) at column 0 —
/// the three chars followed by end-of-line or whitespace — return the text after
/// the marker (`""` for a bare marker).  `----` and `...foo` are not markers.
fn yaml_marker_rest(line: &str) -> Option<&str> {
    let token = line
        .strip_prefix("---")
        .or_else(|| line.strip_prefix("..."))?;
    (token.is_empty() || token.starts_with([' ', '\t', '\r'])).then_some(token)
}

pub(crate) fn escape_yaml(s: &str) -> Cow<'_, str> {
    // The common frontmatter value (a plain title/author/date) carries no escapable
    // char, so borrow it untouched instead of allocating a copy.
    if !s
        .bytes()
        .any(|b| matches!(b, b'\\' | b'"' | b'\n' | b'\r' | b'\t' | b'\0'))
    {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => {}
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-FC001] always_includes_frontmatter
    #[test]
    fn always_includes_frontmatter() {
        let article = ExtractedArticle {
            title: Some("My Title".into()),
            byline: Some("Jane Doe".into()),
            published_time: Some("2026-01-15".into()),
            content_html: "<p>Body text</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false);

        assert!(result.markdown().starts_with("---\n"));
        assert!(result.markdown().contains("\n---\n\n"));
        assert!(result.markdown().contains("title: \"My Title\""));
        assert!(result.markdown().contains("author: \"Jane Doe\""));
        assert!(result.markdown().contains("date: \"2026-01-15\""));
        assert!(result.markdown().contains("Body text"));
    }

    /// [T-FC002] frontmatter_omits_missing_fields
    #[test]
    fn frontmatter_omits_missing_fields() {
        let article = ExtractedArticle {
            title: Some("Only Title".into()),
            byline: None,
            published_time: None,
            content_html: "<p>Text</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false);

        assert!(result.markdown().contains("title: \"Only Title\""));
        assert!(!result.markdown().contains("author:"));
        assert!(!result.markdown().contains("date:"));
    }

    /// [T-FC003] escapes_yaml_special_chars
    #[test]
    fn escapes_yaml_special_chars() {
        assert_eq!(escape_yaml(r#"He said "hello""#), r#"He said \"hello\""#);
        assert_eq!(escape_yaml(r"back\slash"), r"back\\slash");
        assert_eq!(escape_yaml("line\nbreak"), "line\\nbreak");
        assert_eq!(escape_yaml("cr\rreturn"), "cr\\rreturn");
        assert_eq!(escape_yaml("tab\there"), "tab\\there");
        assert_eq!(escape_yaml("null\0byte"), "nullbyte");
    }

    /// [T-FC004] escapes_combined_special_chars
    #[test]
    fn escapes_combined_special_chars() {
        // Backslash-first ordering prevents double-escape: \" must not become \\\"
        assert_eq!(
            escape_yaml("She said \"hi\"\nand left\\"),
            "She said \\\"hi\\\"\\nand left\\\\"
        );
    }

    /// [T-FC012] escape_yaml borrows input that needs no escaping (no allocation)
    #[test]
    fn escape_yaml_borrows_when_no_escape_needed() {
        assert!(matches!(escape_yaml("plain title 2026"), Cow::Borrowed(_)));
    }

    /// [T-FC005] neutralize_yaml_markers rewrites bare ---/... lines to ***
    #[test]
    fn neutralize_yaml_markers_rewrites_bare_doc_markers() {
        assert_eq!(
            neutralize_yaml_markers("before\n---\nmiddle\n...\nafter"),
            "before\n***\nmiddle\n***\nafter"
        );
        // Trailing whitespace / CR on the marker line is tolerated.
        assert_eq!(neutralize_yaml_markers("---  "), "***");
        assert_eq!(neutralize_yaml_markers("...\r"), "***");
    }

    /// [T-FC006] neutralize_yaml_markers leaves indented and inline --- intact
    #[test]
    fn neutralize_yaml_markers_preserves_non_markers() {
        // Indented `---` is not a YAML document marker (must be at column 0).
        assert_eq!(neutralize_yaml_markers("  ---"), "  ---");
        // `---` embedded in a line is ordinary content.
        assert_eq!(neutralize_yaml_markers("a --- b"), "a --- b");
        assert_eq!(neutralize_yaml_markers("----"), "----");
        assert_eq!(neutralize_yaml_markers("...foo"), "...foo");
        assert_eq!(neutralize_yaml_markers("plain"), "plain");
    }

    /// [T-FC007] neutralize_yaml_markers catches markers carrying inline content
    /// (`--- evil: true`), which a YAML parser still honors as a document start
    #[test]
    fn neutralize_yaml_markers_rewrites_marker_with_content() {
        assert_eq!(neutralize_yaml_markers("--- evil: true"), "*** evil: true");
        assert_eq!(neutralize_yaml_markers("--- #x"), "*** #x");
        assert_eq!(neutralize_yaml_markers("---\tfoo"), "***\tfoo");
        assert_eq!(neutralize_yaml_markers("... bar"), "*** bar");
    }

    /// [T-FC008] format_with_frontmatter neutralizes doc markers in the page body
    #[test]
    fn frontmatter_body_cannot_inject_document_marker() {
        let article = ExtractedArticle {
            title: Some("T".into()),
            byline: None,
            published_time: None,
            content_html: String::new(),
            used_raw_fallback: false,
        };
        let body = "intro\n---\ninjected: pwned\nreal";
        let out = format_with_frontmatter(&article, body);

        let after_fm = out.split("---\n\n").nth(1).expect("body after frontmatter");
        assert!(
            !after_fm.lines().any(|l| l == "---" || l == "..."),
            "page body must not introduce a bare YAML document marker:\n{out}"
        );
        assert!(after_fm.contains("injected: pwned"));
    }
}
