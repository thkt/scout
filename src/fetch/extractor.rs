use std::borrow::Cow;

use dom_smoothie::{Config, Readability};
use tracing::warn;

use super::RedactedLogUrl;

/// Render an optional source URL for logging with credentials redacted.
/// `url` is `None` only in tests; production fetch paths always pass `Some`.
fn log_url(url: Option<&str>) -> String {
    url.map_or_else(|| "(none)".to_owned(), |u| RedactedLogUrl(u).to_string())
}

pub(super) struct ExtractedArticle {
    pub(super) title: Option<String>,
    pub(super) byline: Option<String>,
    pub(super) published_time: Option<String>,
    pub(super) content_html: String,
    /// False for both successful extraction and explicit raw mode.
    pub(super) used_raw_fallback: bool,
}

pub(super) fn extract_article(html: &str, url: Option<&str>) -> ExtractedArticle {
    let mut readability = match Readability::new(html, url, Some(Config::default())) {
        Ok(r) => r,
        Err(e) => {
            warn!(url = %log_url(url), error = %e, "readability init failed, using raw fallback");
            return raw_fallback(html);
        }
    };

    match readability.parse() {
        Ok(article) => {
            let title = (!article.title.is_empty()).then(|| article.title.to_string());

            ExtractedArticle {
                title,
                byline: article.byline.map(|b| b.to_string()),
                published_time: article.published_time.map(|t| t.to_string()),
                content_html: article.content.to_string(),
                used_raw_fallback: false,
            }
        }
        Err(e) => {
            warn!(url = %log_url(url), error = %e, "readability parse failed, using raw fallback");
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
        content_html: html.to_owned(),
        used_raw_fallback,
    }
}

/// Scans the bytes case-insensitively in place rather than lowercasing the whole
/// document, which would copy up to the full (multi-MB) input on the warm path.
fn extract_title_from_html(html: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let tag_start = find_ascii_ci(bytes, b"<title")?;
    // `>` is ASCII, so a byte search is exact even inside multi-byte UTF-8 content.
    let content_start = tag_start + bytes[tag_start..].iter().position(|&b| b == b'>')? + 1;
    let content_end = content_start + find_ascii_ci(&bytes[content_start..], b"</title>")?;
    // Trim again after decoding: `&nbsp;` becomes U+00A0, which the first trim
    // could not see and which `str::trim` does treat as whitespace.
    let decoded = decode_char_refs(html[content_start..content_end].trim());
    let title = decoded.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// Decode the HTML character references that realistically reach a `<title>`:
/// the five XML predefined names, `&nbsp;`, and numeric references.
///
/// dom_smoothie hands back an already-decoded title, so without this the same
/// page reads `A & B` or `A &amp; B` in the frontmatter depending on whether
/// Readability succeeded — and the body never shows the difference, because
/// `htmd` decodes it on the way to Markdown. Running the title through
/// that same converter would decode it too, but it also applies Markdown
/// escaping (`&lt;` becomes `\<`), which has no business in a YAML scalar.
///
/// Names outside this set stay literal. The HTML5 table has over 2000 entries,
/// and a title carrying one of the rest is rarer than the cost of shipping the
/// table to catch it.
fn decode_char_refs(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        // The longest form handled here is `&#x10FFFF;` at 10 bytes; a `;` further
        // out belongs to some later construct, so the `&` stays literal.
        match after.find(';').filter(|&semi| semi <= 10) {
            Some(semi) => {
                match decode_one_ref(&after[1..semi]) {
                    Some(ch) => out.push(ch),
                    None => out.push_str(&after[..=semi]),
                }
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The body of one character reference (between `&` and `;`), or `None` when it
/// is a name this does not carry or a number outside Unicode.
fn decode_one_ref(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Byte offset of the first case-insensitive (ASCII) match of `needle` in `haystack`,
/// or `None`.  Offsets index into the original bytes, so slicing the source `&str` at
/// the returned position stays on a char boundary (`needle` is ASCII).
fn find_ascii_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
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

    /// [T-FX001]
    #[test]
    fn extracts_article_content() {
        let result = extract_article(BLOG_HTML, None);

        assert!(!result.used_raw_fallback);
        assert_eq!(result.title.as_deref(), Some("Test Blog Post"));
        assert!(result.byline.is_some());
    }

    /// [T-FX002]
    #[test]
    fn raw_mode_returns_full_html() {
        let result = extract_raw(BLOG_HTML);

        assert!(!result.used_raw_fallback);
        assert!(result.content_html.contains("<nav>"));
        assert!(result.content_html.contains("<footer>"));
    }

    /// [T-FX003]
    #[test]
    fn uses_parsed_result_for_minimal_html() {
        let minimal = "<html><body><p>hi</p></body></html>";
        let result = extract_article(minimal, None);

        assert!(!result.used_raw_fallback);
        assert!(result.content_html.contains("hi"));
    }

    /// [T-FX004]
    #[test]
    fn extracts_title_from_html_tag() {
        let html = "<html><head><title>My Page</title></head><body></body></html>";
        assert_eq!(extract_title_from_html(html), Some("My Page".to_owned()));
    }

    /// [T-FX005]
    #[test]
    fn title_extraction_returns_none_for_empty() {
        let html = "<html><head><title></title></head><body></body></html>";
        assert_eq!(extract_title_from_html(html), None);
    }

    /// [T-FX006]
    #[test]
    fn title_extraction_returns_none_when_missing() {
        let html = "<html><head></head><body></body></html>";
        assert_eq!(extract_title_from_html(html), None);
    }

    /// [T-FX007]
    #[test]
    fn title_extraction_handles_attributes() {
        let html = r#"<html><head><title lang="en">Attributed Title</title></head></html>"#;
        assert_eq!(
            extract_title_from_html(html),
            Some("Attributed Title".to_owned())
        );
    }

    /// [T-FX008]
    #[test]
    fn extracts_title_from_minimal_html() {
        let html = "<html><head><title>Minimal Page</title></head><body><p>hi</p></body></html>";
        let result = extract_article(html, None);

        assert!(!result.used_raw_fallback);
        assert_eq!(result.title, Some("Minimal Page".to_owned()));
    }

    /// [T-FX009]
    #[test]
    fn title_extraction_handles_multibyte() {
        let html = "<html><head><title>日本語タイトル</title></head><body></body></html>";
        assert_eq!(
            extract_title_from_html(html),
            Some("日本語タイトル".to_owned())
        );
    }

    /// [T-FX010]
    #[test]
    fn title_extraction_safe_with_unicode_case_expansion() {
        // The byte-window scan matches the uppercase <TITLE> tag without lowercasing,
        // and returns byte offsets, so slicing the source &str past multibyte content
        // (İ, U+0130, 2 bytes) stays on a char boundary and does not panic.
        let html = "<html><head><TITLE>My Title</TITLE></head><body>İİİ</body></html>";
        assert_eq!(extract_title_from_html(html), Some("My Title".to_owned()));
    }

    /// [T-FX013] the raw-fallback title decodes character references
    ///
    /// dom_smoothie decodes the title it returns, and `htmd` decodes the
    /// body on the way to Markdown — so without this the frontmatter is the one
    /// place a page reads differently depending on whether Readability succeeded.
    #[test]
    fn raw_title_decodes_character_references() {
        let cases = [
            ("A &amp; B", "A & B"),
            ("&lt;script&gt;", "<script>"),
            ("it&#39;s", "it's"),
            ("&quot;quoted&quot;", "\"quoted\""),
            ("&#x3042;", "\u{3042}"),
            ("A &unknownref; B", "A &unknownref; B"),
            ("Tom &amp; Jerry &lt;3", "Tom & Jerry <3"),
        ];
        for (input, expected) in cases {
            let html = format!("<html><head><title>{input}</title></head></html>");
            assert_eq!(
                extract_title_from_html(&html).as_deref(),
                Some(expected),
                "input: {input}"
            );
        }
    }

    /// [T-FX014] a lone `&` and an unterminated reference stay literal
    #[test]
    fn raw_title_leaves_bare_ampersand_alone() {
        for input in ["R&D", "AT&T rules", "&", "&amp"] {
            let html = format!("<html><head><title>{input}</title></head></html>");
            assert_eq!(
                extract_title_from_html(&html).as_deref(),
                Some(input),
                "input: {input}"
            );
        }
    }

    /// [T-FX015] `&nbsp;` decodes and then trims away on its own
    #[test]
    fn raw_title_of_only_nbsp_is_none() {
        let html = "<html><head><title>&nbsp;</title></head></html>";
        assert_eq!(extract_title_from_html(html), None);
    }

    /// [T-FX016] Readability drops chrome and keeps the article body
    ///
    /// ADR-0014 delegates active-markup removal to dom_smoothie, and nothing
    /// asserted that the delegate actually does it — a library upgrade could
    /// start returning the full page and every test here would still pass.
    #[test]
    fn readability_removes_nav_and_footer() {
        let result = extract_article(BLOG_HTML, None);

        assert!(
            !result.used_raw_fallback,
            "this fixture must extract cleanly"
        );
        assert!(
            result.content_html.contains("ownership system"),
            "article body must survive: {}",
            result.content_html
        );
        for chrome in ["<nav>", "Navigation links", "Site footer"] {
            assert!(
                !result.content_html.contains(chrome),
                "{chrome} should have been dropped: {}",
                result.content_html
            );
        }
    }

    /// [T-FX011] readability fallback emits a WARN event whose `url`
    /// field has credentials redacted, so the raw userinfo never reaches the log.
    #[tracing_test::traced_test]
    #[test]
    fn fallback_logs_warn_with_redacted_url() {
        // Empty HTML drives readability into the fallback path.
        let result = extract_article("", Some("https://user:s3cret@example.com/page"));

        assert!(result.used_raw_fallback);
        assert!(
            logs_contain("using raw fallback"),
            "expected the fallback WARN event"
        );
        assert!(logs_contain("WARN"), "event level should be WARN");
        assert!(
            logs_contain("example.com"),
            "url field should retain the host for diagnosis"
        );
        assert!(
            !logs_contain("s3cret"),
            "url credentials must be redacted before logging"
        );
    }

    /// [T-FX012] the fallback `url` field renders a placeholder when
    /// no source URL is available, instead of leaking a Rust `None` debug form.
    #[tracing_test::traced_test]
    #[test]
    fn fallback_logs_placeholder_when_url_absent() {
        let result = extract_article("", None);

        assert!(result.used_raw_fallback);
        assert!(
            logs_contain("using raw fallback"),
            "expected the fallback WARN event"
        );
        assert!(
            logs_contain("url=(none)"),
            "absent url should render as the (none) placeholder"
        );
    }

    /// [T-FX017]
    ///
    /// dom_smoothie returns `GrabFailed` when it finds no article to grab, and
    /// a document holding only a `<script>` is such a case. From here the
    /// default path carries what `--raw` carries.
    #[test]
    fn parse_failure_on_non_empty_html_falls_back_with_the_content_intact() {
        // Marker matches T-FC097's, which needs one free of `_` to survive
        // htmd's escaping. Nothing here converts, but the two read together.
        let html = "<script>SCRIPTMARKER</script>";

        let result = extract_article(html, Some("https://example.com"));

        assert!(
            result.used_raw_fallback,
            "a document with nothing to grab must take the fallback"
        );
        assert_eq!(
            result.content_html, html,
            "the fallback must carry the input through unchanged"
        );
    }

    /// [T-FX018]
    ///
    /// The other of the two fallback entries in `extract_article`:
    /// `Readability::new` rejects a relative `document_url` with
    /// `BadDocumentURL`. Production passes the final absolute URL, so only a
    /// test reaches this one.
    #[test]
    fn relative_document_url_falls_back_at_init() {
        let html = "<article><p>body</p></article>";

        let result = extract_article(html, Some("/relative"));

        assert!(
            result.used_raw_fallback,
            "a relative document URL must take the fallback"
        );
        assert_eq!(result.content_html, html);
    }
}
