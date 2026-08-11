use htmd::HtmlToMarkdown;
use htmd::options::Options;
use serde::Serialize;

use super::FetchError;
use super::extractor::ExtractedArticle;
use crate::yaml::{neutralize_yaml_markers, write_yaml_str};

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

/// Builds the converter fresh per call: htmd's `Options` and handler table are
/// plain owned data, not amortized global state, so there is nothing to gain
/// from caching an instance across calls.
///
/// `Options::default()` already sets `translation_mode: TranslationMode::Pure`
/// (htmd-0.5.5/src/options.rs:19-35); `preformatted_code: true` is the one
/// field the contract adds on top, so it keeps whitespace inside inline
/// `<code>` instead of collapsing it
/// (htmd-0.5.5/src/element_handler/code.rs:118-131).
fn markdown_converter() -> HtmlToMarkdown {
    let options = Options {
        preformatted_code: true,
        ..Options::default()
    };
    HtmlToMarkdown::builder().options(options).build()
}

pub(super) fn to_fetch_result(
    article: &ExtractedArticle,
    url: String,
    decode_uncertain: bool,
) -> Result<FetchResult, FetchError> {
    // Fail-close: a conversion error must surface as a `FetchError`, not as an
    // empty or partial markdown body silently returned to the caller.
    let markdown = markdown_converter()
        .convert(&article.content_html)
        .map_err(|e| FetchError::MarkdownConversion(e.to_string()))?;
    let output = format_with_frontmatter(article, &markdown);

    Ok(FetchResult {
        url,
        markdown: output,
        used_raw_fallback: article.used_raw_fallback,
        decode_uncertain,
    })
}

fn format_with_frontmatter(article: &ExtractedArticle, markdown: &str) -> String {
    let mut fm = String::from("---\n");

    if let Some(title) = &article.title {
        write_yaml_str(&mut fm, "title", title);
    }
    // "byline" is the Readability/journalism term; mapped to "author" for YAML frontmatter
    if let Some(author) = &article.byline {
        write_yaml_str(&mut fm, "author", author);
    }
    if let Some(date) = &article.published_time {
        write_yaml_str(&mut fm, "date", date);
    }

    fm.push_str("---\n\n");
    // The body is untrusted page content appended after the frontmatter, so a
    // column-0 `---`/`...` in it would otherwise open a YAML document boundary.
    fm.push_str(&neutralize_yaml_markers(markdown));
    fm
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-FC001]
    #[test]
    fn always_includes_frontmatter() {
        let article = ExtractedArticle {
            title: Some("My Title".into()),
            byline: Some("Jane Doe".into()),
            published_time: Some("2026-01-15".into()),
            content_html: "<p>Body text</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(result.markdown().starts_with("---\n"));
        assert!(result.markdown().contains("\n---\n\n"));
        assert!(result.markdown().contains("title: \"My Title\""));
        assert!(result.markdown().contains("author: \"Jane Doe\""));
        assert!(result.markdown().contains("date: \"2026-01-15\""));
        assert!(result.markdown().contains("Body text"));
    }

    /// [T-FC002]
    #[test]
    fn frontmatter_omits_missing_fields() {
        let article = ExtractedArticle {
            title: Some("Only Title".into()),
            byline: None,
            published_time: None,
            content_html: "<p>Text</p>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(result.markdown().contains("title: \"Only Title\""));
        assert!(!result.markdown().contains("author:"));
        assert!(!result.markdown().contains("date:"));
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

    /// [T-FC014] to_fetch_result carries both internal flags from their sources
    ///
    /// `used_raw_fallback` arrives on the article (Readability decided it) and
    /// `decode_uncertain` as an argument (the download layer decided it), so the
    /// two are easy to swap. Both reach the caller as `notes` / `degraded_reasons`
    /// entries, and T-FC001/T-FC002 assert only the frontmatter — a swap, or
    /// either field pinned to `false`, passes every other test in this file.
    #[test]
    fn to_fetch_result_carries_both_flags() {
        let raw_fallback_only = ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: "<p>x</p>".into(),
            used_raw_fallback: true,
        };
        let result =
            to_fetch_result(&raw_fallback_only, "https://example.com".into(), false).unwrap();
        assert!(
            result.used_raw_fallback(),
            "the article's raw-fallback flag must reach the result"
        );
        assert!(
            !result.decode_uncertain(),
            "the caller passed decode_uncertain=false"
        );

        let decode_uncertain_only = ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: "<p>x</p>".into(),
            used_raw_fallback: false,
        };
        let result =
            to_fetch_result(&decode_uncertain_only, "https://example.com".into(), true).unwrap();
        assert!(
            !result.used_raw_fallback(),
            "the article carried no raw-fallback flag"
        );
        assert!(
            result.decode_uncertain(),
            "the caller's decode_uncertain must reach the result"
        );
    }

    /// [T-FC015] pre の中の code が含むエスケープ対象 6 文字にバックスラッシュが付かない
    ///
    /// htmd 0.5.5's `escape_if_needed` backslash-escapes six ASCII bytes in
    /// ordinary text — `\ * _ \` [ ]` (htmd-0.5.5/src/dom_walker.rs:374-406) —
    /// but a `<pre><code>` text node takes the `is_pre && parent_tag != "pre"`
    /// branch, which copies the text through with no escaping at all
    /// (htmd-0.5.5/src/dom_walker.rs:34-41). This pins that pass-through: none
    /// of the six bytes gains a backslash inside a code block.
    #[test]
    fn pre_code_escape_target_chars_are_not_backslash_escaped() {
        let article = ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: r#"<pre><code>\ * _ ` [ ] end</code></pre>"#.into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains(r"\ * _ ` [ ] end"),
            "the six escape-target characters must survive unescaped inside a code block:\n{}",
            result.markdown()
        );
    }

    /// [T-FC016] 3 個のバッククォートを含むコードに対してフェンスが 4 個に広がる
    ///
    /// htmd's `get_code_fence_marker` sets the fence width to
    /// `3.max(longest_backtick_run_in_content + 1)`
    /// (htmd-0.5.5/src/element_handler/code.rs:85-103), so a code block
    /// containing a run of 3 backticks must be wrapped in a 4-backtick fence
    /// rather than the usual 3, or the fence would terminate the block early.
    #[test]
    fn code_block_with_three_backticks_widens_fence_to_four() {
        let article = ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: "<pre><code>a ``` b</code></pre>".into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("````\na ``` b\n````"),
            "a 3-backtick run in the content must widen the fence to 4 backticks:\n{}",
            result.markdown()
        );
    }

    /// [T-FC017] class="language-rust" を持つ code のフェンスに情報文字列 rust が付く
    ///
    /// htmd's `find_language_from_attrs` reads the `code` element's `class`
    /// attribute for a `language-*` token and appends the suffix as the
    /// fence's info string with no separating space
    /// (htmd-0.5.5/src/element_handler/code.rs:58-69, 105-116).
    #[test]
    fn code_block_with_language_class_gets_language_info_string() {
        let article = ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: r#"<pre><code class="language-rust">fn main() {}</code></pre>"#.into(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```rust\nfn main() {}\n```"),
            "a `language-rust` class must attach `rust` as the fence info string:\n{}",
            result.markdown()
        );
    }
}
