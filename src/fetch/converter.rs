use std::rc::{Rc, Weak};

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::options::Options;
use htmd::{HtmlToMarkdown, Node};
use markup5ever_rcdom::NodeData;
use serde::Serialize;

use super::FetchError;
use super::extractor::ExtractedArticle;
use crate::markdown::fence_delimiter;
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
    HtmlToMarkdown::builder()
        .options(options)
        .add_handler(vec!["pre"], pre_handler)
        .add_handler(vec!["span"], span_handler)
        .build()
}

/// Fences a `<pre>` with no `<code>` child, which htmd's built-in `pre_handler`
/// otherwise emits unfenced (htmd-0.5.5/src/element_handler/pre.rs:29-40,
/// `concat_strings!("\n\n", content, "\n\n")` with no fence markers).
///
/// A `<pre><code>` pair arrives already fenced by htmd's built-in
/// `code_handler` (htmd-0.5.5/src/element_handler/code.rs:44-73, registered
/// ahead of this handler in `ElementHandlers::new` so `add_handler` shadows
/// only the outer `pre` dispatch, not the inner `code` one), so it passes
/// through unchanged. Telling the two shapes apart reads the DOM, not the
/// walked text: a bare `<pre>` holding syntax-highlighter `<span>`s emits its
/// own leading backtick raw and reads as already fenced.
// `Element` must stay by-value: htmd's blanket `ElementHandler` impl only
// covers `Fn(&dyn Handlers, Element) -> Option<HandlerResult>`
// (htmd-0.5.5/src/element_handler/mod.rs:95-100), so a `&Element` signature
// would not satisfy `add_handler`'s `Handler: ElementHandler` bound.
#[allow(clippy::needless_pass_by_value)]
fn pre_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    let result = handlers.walk_children(element.node);
    let content = result.content.trim_matches('\n');

    if has_code_child(element.node) {
        return Some(HandlerResult {
            content: format!("\n\n{content}\n\n"),
            markdown_translated: result.markdown_translated,
        });
    }

    let content = if opens_with_escaped_fence_char(element.node) {
        content.strip_prefix('\\').unwrap_or(content)
    } else {
        content
    };
    let fence = fence_delimiter(content);
    Some(HandlerResult {
        content: format!("\n\n{fence}\n{content}\n{fence}\n\n"),
        markdown_translated: result.markdown_translated,
    })
}

/// Whether the element has a direct `<code>` child, the shape htmd's
/// `code_handler` fences on its own: it fences exactly when the `<code>`
/// element's parent is `<pre>` (htmd-0.5.5/src/element_handler/code.rs:33-41).
fn has_code_child(node: &Rc<Node>) -> bool {
    node.children.borrow().iter().any(|child| {
        matches!(&child.data, NodeData::Element { name, .. } if name.local.as_ref() == "code")
    })
}

/// Whether the leading `\` in the walked content is htmd's escape rather than
/// source text.
///
/// `dom_walker::escape_pre_text_if_needed` prepends the backslash only to a
/// text node whose direct parent is `<pre>` and whose first character is
/// `` ` `` or `~` (htmd-0.5.5/src/dom_walker.rs:34-41 and 423-436). Reading
/// that first character back off the DOM inverts the escape exactly: source
/// text that already opens with `` \` `` produces the same walked bytes and
/// must keep its backslash.
///
/// The first *text* child is the one to read, since a comment can precede it.
/// An element before it opens the content with its own output instead, leaving
/// no leading backslash to strip either way.
fn opens_with_escaped_fence_char(node: &Rc<Node>) -> bool {
    node.children
        .borrow()
        .iter()
        .find_map(|child| match &child.data {
            NodeData::Text { contents } => Some(contents.borrow().starts_with(['`', '~'])),
            _ => None,
        })
        .unwrap_or(false)
}

/// Passes a `<span>`'s content through unmodified when the span has a `<pre>`
/// ancestor; every other span delegates to `Handlers::fallback`.
///
/// htmd's own `span` fast path (htmd-0.5.5/src/dom_walker.rs:87-110, active
/// while exactly one handler is registered for `span`) trims every leading
/// and trailing `\n` off the span's own walked content unconditionally,
/// including when the span sits inside a `<pre>` and those newlines are real
/// line breaks the surrounding preformatted text depends on. Registering a
/// second `span` handler here (this one) raises the registered-handler count
/// past that fast path's `== 1` gate, so htmd falls back to its normal
/// per-element dispatch for every `<span>` instead
/// (htmd-0.5.5/src/element_handler/mod.rs, `ElementHandlers::handle` /
/// `find_handler`), and this handler runs first as the most-recently
/// registered one.
///
/// The `<pre>`-ancestor check below (`has_pre_ancestor`) looks for a `<pre>`
/// ancestor only. That is narrower than htmd's own `is_inside_pre`
/// (htmd-0.5.5/src/element_handler/mod.rs:358-367), which also treats a
/// `<code>` ancestor as "inside pre": a `<span>` nested in inline `<code>`
/// with no `<pre>` ancestor is not "suppressed" by this handler and falls
/// through to `Handlers::fallback`, reaching htmd's built-in `span_handler`
/// unchanged and keeping the inline-code handler's own newline-to-space
/// folding.
// `Element` must stay by-value for the same `add_handler` signature reason as
// `pre_handler` above.
#[allow(clippy::needless_pass_by_value)]
fn span_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if has_pre_ancestor(element.node) {
        return Some(handlers.walk_children(element.node));
    }
    handlers.fallback(element)
}

/// Whether any ancestor of `node` is a `<pre>` element. Mirrors the
/// take-upgrade-put-back pattern htmd's own `node_util::get_parent_node` uses
/// to read the `Cell<Option<WeakHandle>>` parent link without leaving it
/// empty for later traversals (htmd-0.5.5/src/node_util.rs:13-23).
fn has_pre_ancestor(node: &Rc<Node>) -> bool {
    let mut current = get_parent(node);
    while let Some(parent) = current {
        if matches!(&parent.data, NodeData::Element { name, .. } if name.local.as_ref() == "pre") {
            return true;
        }
        current = get_parent(&parent);
    }
    false
}

fn get_parent(node: &Rc<Node>) -> Option<Rc<Node>> {
    let value = node.parent.take();
    let parent = value.as_ref().and_then(Weak::upgrade);
    node.parent.set(value);
    parent
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

    /// Minimal `ExtractedArticle` fixture for tests that only vary the body
    /// HTML: no title/byline/published_time and no raw-fallback flag.
    fn article(html: &str) -> ExtractedArticle {
        ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: html.into(),
            used_raw_fallback: false,
        }
    }

    /// [T-FC023] table の出力がヘッダ行に続く区切り行を含む
    ///
    /// htmd's `table_handler` pushes the header row (`format_row_padded`)
    /// immediately followed by the separator row (`format_separator_padded`)
    /// with no blank line between them
    /// (htmd-0.5.5/src/element_handler/table.rs:178-183).
    #[test]
    fn table_output_includes_a_separator_row_following_the_header_row() {
        let article = article(
            "<table><thead><tr><th>Name</th><th>Age</th></tr></thead>\
                <tbody><tr><td>Alice</td><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        let header_idx = lines
            .iter()
            .position(|line| line.contains("Name") && line.contains("Age"))
            .expect("header row must be present");
        let separator_line = lines
            .get(header_idx + 1)
            .expect("a line must immediately follow the header row");

        assert!(
            !separator_line.is_empty()
                && separator_line.contains('-')
                && separator_line
                    .chars()
                    .all(|c| c == '|' || c == '-' || c == ' '),
            "the line right after the header row must be a dash separator row:\n{markdown}"
        );
    }

    /// [T-FC024] li の中の pre がリストのマーカーと同じ項目に留まる
    ///
    /// `list_item_handler` walks the `<li>`'s children into one string and
    /// indents every line but the first by the marker's width before
    /// prefixing the marker
    /// (htmd-0.5.5/src/element_handler/li.rs:9-21,
    /// `indent_text_except_first_line`).
    #[test]
    fn li_pre_stays_in_the_same_item_as_the_list_marker() {
        let article = article("<ul><li>intro<pre><code>line1\nline2</code></pre></li></ul>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        let marker_line = markdown
            .lines()
            .find(|line| line.contains("intro"))
            .expect("the list marker line must carry the li's leading text");
        assert!(
            marker_line.trim_start().starts_with(['-', '*']),
            "the li's leading text must carry a list marker:\n{markdown}"
        );

        let fence_body_lines: Vec<&str> = markdown
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed == "```" || trimmed == "line1" || trimmed == "line2"
            })
            .collect();
        assert_eq!(
            fence_body_lines.len(),
            4,
            "expected two fence delimiters and two content lines:\n{markdown}"
        );
        for line in fence_body_lines {
            assert!(
                line.starts_with(' '),
                "a <pre> block inside <li> must stay indented under the list marker, \
                 not break out as a column-0 block: {line:?}\n{markdown}"
            );
        }
    }

    /// [T-FC025] td の中の pre が表の行を分断しない
    ///
    /// A table cell's content passes through `normalize_cell_content`, which
    /// replaces every `\n` with a single space before the cell is written
    /// into the pipe-delimited row
    /// (htmd-0.5.5/src/element_handler/table.rs:227-233).
    #[test]
    fn td_pre_does_not_split_the_table_row() {
        let article = article(
            "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
                <pre><code>line1\nline2</code></pre></td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        let data_row = markdown
            .lines()
            .find(|line| line.contains("line1"))
            .expect("the data row must be present");
        assert!(
            data_row.contains("line1") && data_row.contains("line2"),
            "both lines of the <pre> block must land on the same table row:\n{markdown}"
        );
        assert!(
            data_row.starts_with('|') && data_row.ends_with('|'),
            "the row must stay a single well-formed pipe-delimited row:\n{markdown}"
        );
        assert_eq!(
            markdown.lines().filter(|l| l.contains('|')).count(),
            3,
            "the table must still have exactly 3 pipe-bearing lines \
             (header, separator, one data row):\n{markdown}"
        );
    }

    /// [T-FC026] 括弧を含む URL のリンク先が括弧の手前で切れない
    ///
    /// `AnchorElementHandler::escape_link_destination` backslash-escapes
    /// every `(` and `)` in the href before writing it as the link
    /// destination (htmd-0.5.5/src/element_handler/anchor.rs:170-177), so the
    /// part of the URL after an opening paren cannot be misread as closing the
    /// Markdown link early.
    #[test]
    fn link_target_with_parens_is_not_cut_off_before_the_parenthesis() {
        let article = article(r#"<p><a href="https://example.com/wiki/Foo_(bar)">Foo</a></p>"#);

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains(r"[Foo](https://example.com/wiki/Foo_\(bar\))"),
            "the link destination must carry the full URL past the parenthesis, \
             not truncate at it:\n{markdown}"
        );
    }

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
            ..article("<p>Text</p>")
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
            ..article("")
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
            used_raw_fallback: true,
            ..article("<p>x</p>")
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

        let decode_uncertain_only = article("<p>x</p>");
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
        let article = article(r#"<pre><code>\ * _ ` [ ] end</code></pre>"#);

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
        let article = article("<pre><code>a ``` b</code></pre>");

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
        let article = article(r#"<pre><code class="language-rust">fn main() {}</code></pre>"#);

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```rust\nfn main() {}\n```"),
            "a `language-rust` class must attach `rust` as the fence info string:\n{}",
            result.markdown()
        );
    }

    /// [T-FC019] code 子を持たない pre がフェンスで囲まれて出る
    ///
    /// htmd's built-in `pre_handler` wraps a `<pre>` with no `<code>` child in
    /// blank lines only, with no fence markers at all
    /// (htmd-0.5.5/src/element_handler/pre.rs:29-40,
    /// `concat_strings!("\n\n", content, "\n\n")`). U-003 registers a `pre`
    /// handler via `HtmlToMarkdownBuilder::add_handler` that fences this case
    /// using `crate::markdown::fence_delimiter`.
    #[test]
    fn pre_without_code_child_is_wrapped_in_a_fence() {
        let article = article("<pre>plain text</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```\nplain text\n```"),
            "a <pre> with no <code> child must be wrapped in a fenced code block:\n{}",
            result.markdown()
        );
    }

    /// [T-FC020] htmd が既にフェンスした pre の中の code を二重のフェンスで囲まない
    ///
    /// A `<pre><code>` pair is already turned into a single fenced block by
    /// htmd's built-in `code_handler`
    /// (htmd-0.5.5/src/element_handler/code.rs:44-73). The added `pre` handler
    /// must recognize this case by the direct `<code>` child in the DOM, and
    /// pass it through instead of wrapping it in a second fence.
    #[test]
    fn pre_code_already_fenced_by_htmd_is_not_double_fenced() {
        let article = article("<pre><code>fn main() {}</code></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```\nfn main() {}\n```"),
            "the pre>code block must still be fenced once:\n{}",
            result.markdown()
        );
        assert_eq!(
            result.markdown().matches("```").count(),
            2,
            "an already-fenced pre>code block must not gain a second fence:\n{}",
            result.markdown()
        );
    }

    /// [T-FC021] htmd が pre 直下のテキスト先頭に付けるバックスラッシュはフェンス内に残らない
    ///
    /// `dom_walker::escape_pre_text_if_needed` prepends a backslash to a
    /// `<pre>` direct text node whose first character is a fence character
    /// (`` ` `` or `~`), so htmd's own unfenced output cannot be misread as
    /// opening a fence (htmd-0.5.5/src/dom_walker.rs:423-436). Once the added
    /// `pre` handler wraps that content in its own fence, the character is
    /// already protected and the extra backslash must not survive.
    #[test]
    fn htmd_leading_backslash_before_pre_text_does_not_survive_inside_the_fence() {
        let article = article("<pre>`hello</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```\n`hello\n```"),
            "the leading backtick must survive unescaped inside the fence:\n{}",
            result.markdown()
        );
        assert!(
            !result.markdown().contains("\\`hello"),
            "htmd's pre-text leading backslash must be stripped once the content is fenced:\n{}",
            result.markdown()
        );
    }

    /// [T-FC022] 原文の行頭にあるバックスラッシュとバッククォートの並びがそのまま残る
    ///
    /// `escape_pre_text_if_needed` only inspects the first character of the
    /// whole text node (htmd-0.5.5/src/dom_walker.rs:423-426), so a literal
    /// `` \` `` sequence occurring after the text node's first character is
    /// never touched by htmd. This pins that the stripping added for T-FC021
    /// targets only the walked content's overall leading position, not every
    /// line head inside it.
    #[test]
    fn literal_backslash_backtick_pair_mid_content_survives_unstripped() {
        let article = article("<pre>abc\n\\` def</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```\nabc\n\\` def\n```"),
            "a literal backslash-backtick pair not at the content's head must survive as written:\n{}",
            result.markdown()
        );
    }

    /// [T-FC027] 原文の先頭にあるバックスラッシュとバッククォートの並びがそのまま残る
    ///
    /// `escape_pre_text_if_needed` prepends its backslash only when the text
    /// node's first character is `` ` `` or `~`
    /// (htmd-0.5.5/src/dom_walker.rs:423-436), so source text that already
    /// opens with `` \` `` reaches the handler untouched. Telling the two apart
    /// needs the DOM: the walked content is the same `` \` `` either way.
    #[test]
    fn source_leading_backslash_backtick_pair_survives_unstripped() {
        let article = article("<pre>\\`hello</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```\n\\`hello\n```"),
            "a source backslash before the leading backtick must survive as written:\n{}",
            result.markdown()
        );
    }

    /// [T-FC029] コメントノードが先行しても htmd のエスケープはフェンス内に残らない
    ///
    /// The escape lands on the first *text* child, which a comment or any other
    /// non-text node can precede. Looking only at the element's first child
    /// would read that node instead and leave the backslash in place.
    #[test]
    fn htmd_leading_backslash_is_stripped_when_a_comment_precedes_the_text() {
        let article = article("<pre><!-- c -->`hello</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            !result.markdown().contains("\\`hello"),
            "htmd's escape must be stripped even when a comment node comes first:\n{}",
            result.markdown()
        );
    }

    /// [T-FC028] インライン要素を子に持つ code 無しの pre がフェンスで囲まれて出る
    ///
    /// Syntax highlighters wrap code lines in `<span>` without a `<code>`
    /// child. htmd escapes only text nodes whose direct parent is `<pre>`
    /// (htmd-0.5.5/src/dom_walker.rs:34-41), so text nested in a `<span>`
    /// reaches the handler with its leading fence character raw, looking
    /// exactly like the already-fenced output of htmd's `code_handler`.
    #[test]
    fn pre_with_nested_inline_element_is_wrapped_in_a_fence() {
        let article = article("<pre><span>`x`</span></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("```\n`x`\n```"),
            "a <pre> whose fence-leading text comes from a nested element must still be fenced:\n{}",
            result.markdown()
        );
    }

    /// [T-FC052] pre の中の span の末尾にある改行が出力に残る
    ///
    /// htmd's built-in `span` fast path (dom_walker.rs:87-110, active while
    /// exactly one handler is registered for `span`) trims every leading and
    /// trailing `\n` off a span's own walked content regardless of a `<pre>`
    /// ancestor. U-001 registers a `span` handler that passes a `<pre>`-nested
    /// span's content through unmodified, so a trailing `\n` inside the span
    /// must survive up to the sibling text that follows it.
    #[test]
    fn trailing_newline_at_the_end_of_a_span_inside_pre_survives_in_the_output() {
        let article = article("<pre><span>line1\n</span>line2</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("line1\nline2"),
            "the span's trailing newline must reach the sibling text as a real line break:\n{}",
            result.markdown()
        );
    }

    /// [T-FC053] 行ごとに span を並べた pre で各行が別の行として出る
    ///
    /// A syntax highlighter emits one `<span>` per source line, each carrying
    /// its own trailing `\n`. The built-in fast path trims that `\n` off each
    /// span independently, so adjacent lines collapse into one. U-001's `span`
    /// handler must pass every such span's content through untouched so the
    /// per-span newlines keep the lines apart.
    ///
    /// Each span carries a distinct `data-line` attribute so htmd's adjacent-
    /// element merge (`dom_walker::can_combine`, htmd-0.5.5/src/dom_walker.rs:
    /// 250-307, gated on `attrs1 == attrs2`) does not fold the three sibling
    /// spans into one node ahead of the per-span trim this test targets.
    #[test]
    fn pre_with_one_span_per_line_keeps_each_line_on_its_own_output_line() {
        let article = article(
            "<pre><span data-line=\"1\">line1\n</span>\
             <span data-line=\"2\">line2\n</span>\
             <span data-line=\"3\">line3</span></pre>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("line1\nline2\nline3"),
            "each line-span's content must land on its own output line, in order:\n{}",
            result.markdown()
        );
    }

    /// [T-FC054] pre の外の inline code の中の span では改行が剥がれ空白も残らない
    ///
    /// U-001's ancestor check for the passthrough branch looks for a `<pre>`
    /// ancestor only — narrower than htmd's `is_inside_pre`, which also treats
    /// a `<code>` ancestor as "inside pre" (htmd-0.5.5/src/element_handler/
    /// mod.rs:358-367). A span nested in inline `<code>` with no `<pre>`
    /// ancestor therefore falls through to htmd's built-in span handler via
    /// `Handlers::fallback`, and that handler's `content.trim_matches('\n')`
    /// (htmd-0.5.5/src/element_handler/span.rs:33) removes the newline from
    /// both edges of the span's content. The removal happens before
    /// `handle_preformatted_code`'s own newline-to-space folding
    /// (htmd-0.5.5/src/element_handler/code.rs:189-208) can reach it, so the
    /// two lines join with no separator at all.
    ///
    /// Measured identical with the `span` registration removed, so this is
    /// htmd's standing behavior rather than a difference U-001 introduces. A
    /// newline sitting in the `<code>`'s own text node instead of inside a
    /// span never reaches the span handler and still folds to a space.
    #[test]
    fn span_inside_inline_code_outside_pre_loses_the_newline_entirely() {
        let article = article("<p><code><span>line1\n</span>line2</code></p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("line1line2"),
            "a span inside inline code with no <pre> ancestor must fall through to htmd's \
             built-in span handler, whose trim_matches('\\n') strips the newline before the \
             code handler can fold it to a space:\n{}",
            result.markdown()
        );
        assert!(
            !result.markdown().contains("line1\nline2"),
            "the newline must not survive raw:\n{}",
            result.markdown()
        );
    }

    /// [T-FC055] 隣の span が要素の子を持つ形でも pre の中の改行が残る
    ///
    /// The passthrough branch must walk the span's children through the full
    /// handler chain (`Handlers::walk_children`, which recurses into nested
    /// elements) rather than reading raw text only. A neighboring line-span
    /// whose own child is an element (not a bare text node) must still convert
    /// that nested element to Markdown while the preceding line-span's
    /// trailing newline survives.
    #[test]
    fn pre_newline_survives_when_the_neighboring_span_has_an_element_child() {
        let article = article("<pre><span>line1\n</span><span><b>line2</b></span></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();

        assert!(
            result.markdown().contains("line1\n**line2**"),
            "the newline before a line-span with an element child must survive, and that \
             child element must still be converted to Markdown:\n{}",
            result.markdown()
        );
    }
}
