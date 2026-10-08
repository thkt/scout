use std::borrow::Cow;
use std::rc::{Rc, Weak};

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::options::{Options, TranslationMode};
use htmd::{HtmlToMarkdown, Node};
use markup5ever_rcdom::NodeData;
use serde::Serialize;

use super::FetchError;
use super::extractor::ExtractedArticle;
use crate::markdown::{fence_delimiter, shift_headings};
use crate::yaml::{neutralize_yaml_markers_outside_fences, write_yaml_str};

/// Fetched page content converted to Markdown. Fields are private so the only
/// construction paths are [`to_fetch_result`], [`plain_text_result`] and
/// [`FetchResult::for_test`] (test fixtures); callers cannot build a result
/// that skips the output boundary's frontmatter and YAML neutralization.
#[derive(Debug, Serialize)]
pub(crate) struct FetchResult {
    url: String,
    markdown: String,
    /// Plain-text literals must bypass Markdown heading interpretation.
    #[serde(skip_serializing)]
    is_plain_text: bool,
    /// Internal flag: surfaced as a `notes` entry in scout's JSON output, not as data.
    #[serde(skip_serializing)]
    used_raw_fallback: bool,
    /// Internal flag: the body could not be decoded cleanly, so the
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

    /// Shift converted HTML headings while preserving plain-text syntax and whitespace.
    pub(crate) fn with_heading_offset(&self, offset: usize) -> Cow<'_, str> {
        if self.is_plain_text {
            Cow::Borrowed(self.markdown())
        } else {
            Cow::Owned(shift_headings(self.markdown(), offset))
        }
    }

    pub(crate) fn used_raw_fallback(&self) -> bool {
        self.used_raw_fallback
    }

    pub(crate) fn decode_uncertain(&self) -> bool {
        self.decode_uncertain
    }

    /// Test-only constructor. Production uses the media-specific constructors.
    #[cfg(test)]
    pub(crate) fn for_test(url: String, markdown: String, used_raw_fallback: bool) -> Self {
        Self {
            url,
            markdown,
            is_plain_text: false,
            used_raw_fallback,
            decode_uncertain: false,
        }
    }

    /// Test builder keeps decode uncertainty separate from the raw-fallback flag.
    #[cfg(test)]
    pub(crate) fn with_decode_uncertain(mut self, decode_uncertain: bool) -> Self {
        self.decode_uncertain = decode_uncertain;
        self
    }
}

pub(crate) const RAW_FALLBACK_NOTE: &str =
    "> Note: Readability extraction failed. Showing raw page conversion.\n\n";

pub(crate) const DECODE_UNCERTAIN_NOTE: &str = "> Note: Character encoding could not be determined; the body is a best-effort decode and may be garbled.\n\n";

/// Keep inline-code whitespace with `preformatted_code`; the default mode is Pure.
fn markdown_converter() -> HtmlToMarkdown {
    let options = Options {
        preformatted_code: true,
        ..Options::default()
    };
    HtmlToMarkdown::builder()
        .options(options)
        .add_handler(vec!["pre"], pre_handler)
        .add_handler(vec!["span"], span_handler)
        .add_handler(vec!["table"], table_handler)
        .add_handler(vec!["a"], a_handler)
        .add_handler(SUPPRESSED_TAGS.to_vec(), suppressed_handler)
        .build()
}

/// Fence bare `<pre>` content; htmd already fences a direct `<code>` child.
/// Inspect the DOM: highlighted bare text can itself start with backticks.
#[expect(
    clippy::needless_pass_by_value,
    reason = "htmd's ElementHandler blanket impl takes Element by value"
)]
fn pre_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    // Walking first merges adjacent DOM siblings before either branch reads them.
    // Only the `<pre><code>` branch uses the walked string.
    let result = handlers.walk_children(element.node);

    // Handle cells before the code-child split to avoid block fences in tables.
    // Read the whole subtree so text beside a `<code>` child survives.
    if has_table_cell_ancestor(element.node) {
        let content = text_content(element.node);
        return Some(HandlerResult {
            content: inline_code_span(content.trim_matches('\n')),
            markdown_translated: result.markdown_translated,
        });
    }

    if has_code_child(element.node) {
        let content = result.content.trim_matches('\n');
        return Some(HandlerResult {
            content: format!("\n\n{content}\n\n"),
            markdown_translated: result.markdown_translated,
        });
    }

    let (content, markdown_translated) = raw_pre_content(handlers, element.node);
    let content = content.trim_matches('\n');
    let fence = fence_delimiter(content);
    Some(HandlerResult {
        content: format!("\n\n{fence}\n{content}\n{fence}\n\n"),
        markdown_translated,
    })
}

/// Suppress element bodies that htmd otherwise walks in Pure mode. This runs
/// after JS detection and does not alter the downloaded source.
///
/// htmd dispatches by local name: suppress `desc` only in SVG so visible HTML
/// text survives. Suppress `title` in all namespaces; metadata extraction
/// handles the page title separately. Other `desc` elements delegate to fallback.
fn suppressed_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if !is_suppressed_element(element.node) {
        return handlers.fallback(element);
    }
    Some(HandlerResult {
        content: String::new(),
        markdown_translated: true,
    })
}

const SUPPRESSED_TAGS: [&str; 7] = [
    "script", "style", "noscript", "textarea", "iframe", "desc", "title",
];

/// Direct DOM readers must use the same suppression rules as handler dispatch.
fn is_suppressed_element(node: &Rc<Node>) -> bool {
    let Some(tag) = element_tag(node) else {
        return false;
    };
    SUPPRESSED_TAGS.contains(&tag)
        && (tag != "desc" || element_namespace(node) == Some(SVG_NAMESPACE))
}

/// SVG namespace of the `desc` element, even though its children parse as HTML.
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

fn element_namespace(node: &Rc<Node>) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.ns.as_ref()),
        _ => None,
    }
}

/// Tags whose content html5ever tokenizes as raw text, so an unclosed one
/// consumes every following byte until its own end tag. Every entry is also a
/// `suppressed_handler` tag, which is what turns the swallow into a silent
/// loss rather than garbled output. `desc` is deliberately absent: it holds
/// ordinary parsed children and cannot swallow anything.
const RAW_TEXT_TAGS: [&str; 6] = ["script", "style", "textarea", "iframe", "noscript", "title"];

/// Expand self-closed raw-text tags so HTML parsing of XHTML cannot swallow
/// the remaining body into an element that suppression then removes.
/// This does not implement the rest of XML syntax. The byte scan can also
/// rewrite matches in comments or quoted attributes, where they remain inert.
fn close_self_closed_raw_text_tags(html: &str) -> Cow<'_, str> {
    let bytes = html.as_bytes();
    let mut rewritten: Option<String> = None;
    let mut copied_to = 0;
    let mut cursor = 0;

    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }
        let Some(tag) = raw_text_tag_at(bytes, cursor + 1) else {
            cursor += 1;
            continue;
        };
        let Some(tag_end) = start_tag_end(bytes, cursor + 1 + tag.len()) else {
            break;
        };
        if bytes[tag_end - 1] == b'/' {
            let out = rewritten.get_or_insert_with(String::new);
            out.push_str(&html[copied_to..tag_end - 1]);
            out.push_str("></");
            out.push_str(tag);
            out.push('>');
            copied_to = tag_end + 1;
            cursor = tag_end + 1;
            continue;
        }
        // Skip raw-text contents: inserting a closing tag into a JS string would
        // end the element early and leak the remaining script into the body.
        cursor = end_tag_at_or_after(bytes, tag_end + 1, tag).unwrap_or(bytes.len());
    }

    match rewritten {
        Some(mut out) => {
            out.push_str(&html[copied_to..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(html),
    }
}

/// Match raw-text tag names case-insensitively at a tokenizer name boundary
/// so `<scriptlet>` does not match `script`.
fn raw_text_tag_at(bytes: &[u8], from: usize) -> Option<&'static str> {
    RAW_TEXT_TAGS.into_iter().find(|tag| {
        let end = from + tag.len();
        bytes.len() > end
            && bytes[from..end].eq_ignore_ascii_case(tag.as_bytes())
            && matches!(
                bytes[end],
                b' ' | b'\t' | b'\n' | b'\r' | 0x0c | b'/' | b'>'
            )
    })
}

/// Find the matching end tag; start tags inside raw text cannot close it.
fn end_tag_at_or_after(bytes: &[u8], from: usize, tag: &str) -> Option<usize> {
    (from..bytes.len().saturating_sub(1)).find(|&index| {
        bytes[index] == b'<'
            && bytes[index + 1] == b'/'
            && raw_text_tag_at(bytes, index + 2) == Some(tag)
    })
}

/// Find the start-tag terminator, ignoring `>` inside quoted attributes.
fn start_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (offset, &byte) in bytes[from..].iter().enumerate() {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(from + offset),
            None => {}
        }
    }
    None
}

fn element_tag(node: &Rc<Node>) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

fn has_code_child(node: &Rc<Node>) -> bool {
    node.children
        .borrow()
        .iter()
        .any(|child| element_tag(child) == Some("code"))
}

/// Read bare `<pre>` text from the DOM to avoid htmd's fence-character escapes.
/// Requires the initial `walk_children` in `pre_handler` to merge siblings.
/// Only element children contribute to `markdown_translated`, as in htmd.
fn raw_pre_content(handlers: &dyn Handlers, node: &Rc<Node>) -> (String, bool) {
    let mut content = String::new();
    let mut markdown_translated = true;
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => content.push_str(&contents.borrow()),
            NodeData::Element { .. } => {
                if let Some(res) = handlers.handle(child) {
                    markdown_translated &= res.markdown_translated;
                    push_element_content(&mut content, &res.content);
                }
            }
            _ => {}
        }
    }
    (content, markdown_translated)
}

/// Cap the join at two newlines when appending a converted element's content.
/// Text children append directly, but their trailing newlines can be capped
/// when a subsequent element's content is appended.
/// Newline counts are also byte counts, so these cuts are UTF-8 safe.
fn push_element_content(content: &mut String, addition: &str) {
    let trailing = content.chars().rev().take_while(|&c| c == '\n').count();
    let leading = addition.chars().take_while(|&c| c == '\n').count();
    let total = trailing + leading;
    if total > 2 {
        let excess = total - 2;
        let cut_from_content = excess.min(trailing);
        content.truncate(content.len() - cut_from_content);
        let cut_from_addition = excess - cut_from_content;
        content.push_str(&addition[cut_from_addition..]);
    } else {
        content.push_str(addition);
    }
}

/// Preserve span-edge newlines inside `<pre>`. Registering a second span handler
/// disables htmd's trimming fast path; other spans delegate to fallback.
/// The ancestor check excludes inline `<code>` (DR-0025, T-FC054).
fn span_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if has_pre_ancestor(element.node) {
        return Some(handlers.walk_children(element.node));
    }
    handlers.fallback(element)
}

fn has_pre_ancestor(node: &Rc<Node>) -> bool {
    has_ancestor_matching(node, |tag| tag == "pre")
}

fn has_table_cell_ancestor(node: &Rc<Node>) -> bool {
    has_ancestor_matching(node, |tag| matches!(tag, "td" | "th"))
}

fn has_ancestor_matching(node: &Rc<Node>, predicate: impl Fn(&str) -> bool) -> bool {
    let mut current = get_parent(node);
    while let Some(parent) = current {
        if element_tag(&parent).is_some_and(&predicate) {
            return true;
        }
        current = get_parent(&parent);
    }
    false
}

/// Taking the `Cell` parent link is necessary to read it; restore it so later
/// traversals retain the same DOM ancestry.
fn get_parent(node: &Rc<Node>) -> Option<Rc<Node>> {
    let value = node.parent.take();
    let parent = value.as_ref().and_then(Weak::upgrade);
    node.parent.set(value);
    parent
}

/// Read table-cell `<pre>` content without htmd's fence-character escapes.
/// Direct DOM traversal must suppress hidden elements and turn `<br>` into a
/// newline; `normalize_cell_content` folds that newline to a space.
fn text_content(node: &Rc<Node>) -> String {
    let mut out = String::new();
    push_text_content(node, &mut out);
    out
}

fn push_text_content(node: &Rc<Node>, out: &mut String) {
    match &node.data {
        NodeData::Text { contents } => {
            out.push_str(&contents.borrow());
            return;
        }
        NodeData::Element { .. } if is_suppressed_element(node) => return,
        NodeData::Element { .. } if element_tag(node) == Some("br") => {
            out.push('\n');
            return;
        }
        _ => {}
    }
    for child in node.children.borrow().iter() {
        push_text_content(child, out);
    }
}

/// Build a CommonMark 0.31.2 §6.3 code span: its backtick delimiter must exceed
/// the longest content run, without the block-fence minimum. Inner spaces keep
/// content-edge backticks separate from the delimiter.
fn inline_code_span(content: &str) -> String {
    let max_run = content
        .bytes()
        .fold((0usize, 0usize), |(longest, run), b| {
            if b == b'`' {
                let next = run + 1;
                (longest.max(next), next)
            } else {
                (longest, 0)
            }
        })
        .0;
    let delimiter = "`".repeat(max_run + 1);
    if content.starts_with('`') || content.ends_with('`') {
        format!("{delimiter} {content} {delimiter}")
    } else {
        format!("{delimiter}{content}{delimiter}")
    }
}

/// Remove empty same-page anchors emitted by highlighters and heading links.
/// Delegate other links so destination escaping and link bookkeeping stay in htmd.
/// Strip titles only from links with text; empty absolute links retain theirs
/// (T-FC049).
fn a_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    let result = handlers.walk_children(element.node);
    let has_link_text = !result.content.trim().is_empty();
    // An empty href resolves to the current page, just like a fragment.
    let is_empty_fragment_anchor = anchor_href(&element)
        .is_some_and(|href| href.is_empty() || href.starts_with('#'))
        && !has_link_text;

    if is_empty_fragment_anchor {
        return Some(HandlerResult {
            content: String::new(),
            markdown_translated: true,
        });
    }

    // Read before `element` moves into `fallback` below.
    let title_attr = has_link_text
        .then(|| anchor_attr(&element, "title"))
        .flatten();
    let result = handlers.fallback(element)?;

    let Some(title_attr) = title_attr else {
        return Some(result);
    };
    Some(HandlerResult {
        content: strip_link_title(&result.content, &title_attr),
        markdown_translated: result.markdown_translated,
    })
}

fn anchor_href(element: &htmd::Element) -> Option<String> {
    anchor_attr(element, "href")
}

fn anchor_attr(element: &htmd::Element, name: &str) -> Option<String> {
    element
        .attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .map(|attr| attr.value.to_string())
}

/// Remove only the delegated link's title suffix, never matching text in the
/// link body. Unknown tail shapes remain unchanged. Reproduce htmd's title
/// escaping and reflow before matching, including whitespace-only titles.
fn strip_link_title(content: &str, title_attr: &str) -> String {
    let processed_title = process_title_like_htmd(title_attr);
    let (body, trailing_ws) = split_trailing_document_whitespace(content);
    let Some(before_close_paren) = body.strip_suffix(')') else {
        return content.to_owned();
    };
    let title_suffix = format!(" \"{processed_title}\"");
    let Some(before_title) = before_close_paren.strip_suffix(title_suffix.as_str()) else {
        return content.to_owned();
    };
    format!("{before_title}){trailing_ws}")
}

/// Mirror htmd's private `process_title` byte for byte so the delegated suffix
/// can be located; this is not an independent escaping policy.
fn process_title_like_htmd(text: &str) -> String {
    let mut result = String::new();
    let mut wrote_any = false;
    for line in text.lines() {
        let line = trim_document_whitespace(line);
        if line.is_empty() {
            continue;
        }
        if wrote_any {
            result.push('\n');
        }
        for ch in line.chars() {
            if ch == '"' {
                result.push('\\');
            }
            result.push(ch);
        }
        wrote_any = true;
    }
    result
}

/// Separate whitespace htmd re-appends after the closing `)` so it cannot hide
/// the title suffix. Leading whitespace does not affect this tail match.
fn split_trailing_document_whitespace(content: &str) -> (&str, &str) {
    let body = content.trim_end_matches(['\t', '\n', '\r', ' ']);
    content.split_at(body.len())
}

/// Read mixed `<th>`/`<td>` rows positionally; htmd's per-tag extraction loses
/// cells of the other kind. Keep caption, cell normalization and column-count
/// behavior, but omit column-width padding. Non-Pure modes delegate to htmd
/// (T-FC068); production uses Pure.
fn table_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if handlers.options().translation_mode != TranslationMode::Pure {
        return handlers.fallback(element);
    }

    let mut captions: Vec<String> = Vec::new();
    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    // Only the first candidate row may decide the header.
    let mut header_decided = false;
    let mut markdown_translated = true;

    for child in element.node.children.borrow().iter() {
        let Some(tag) = element_tag(child) else {
            continue;
        };
        match tag {
            "caption" => {
                if let Some(res) = handlers.handle(child) {
                    markdown_translated &= res.markdown_translated;
                    captions.push(trim_document_whitespace(&res.content).to_owned());
                }
            }
            "thead" => {
                let mut thead_rows = row_children(child).into_iter();
                // A thead's first row becomes the header whether its cells are
                // `<th>` or `<td>`, unlike a body row.
                if let Some(row_node) = thead_rows.next() {
                    let (cells, translated) = extract_row_cells(handlers, &row_node);
                    headers = cells;
                    markdown_translated &= translated;
                    header_decided = true;
                }
                for row_node in thead_rows {
                    let (cells, translated) = extract_row_cells(handlers, &row_node);
                    markdown_translated &= translated;
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                }
            }
            "tbody" | "tfoot" => {
                for row_node in row_children(child) {
                    markdown_translated &= extract_data_row(
                        handlers,
                        &row_node,
                        &mut header_decided,
                        &mut headers,
                        &mut rows,
                    );
                }
            }
            "tr" => {
                markdown_translated &= extract_data_row(
                    handlers,
                    child,
                    &mut header_decided,
                    &mut headers,
                    &mut rows,
                );
            }
            _ => {}
        }
    }

    let num_columns = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if num_columns == 0 {
        let content = handlers.walk_children(element.node).content;
        let content = content.trim_matches('\n');
        if content.is_empty() {
            return None;
        }
        return Some(HandlerResult {
            content: format!("\n\n{content}\n\n"),
            markdown_translated,
        });
    }

    let mut table_md = String::from("\n\n");
    for caption in captions {
        table_md.push_str(&caption);
        table_md.push('\n');
    }
    // With columns but no qualifying header, emit an empty header and separator
    // so the first data row is not interpreted as the header.
    table_md.push_str(&format_table_row(&headers, num_columns));
    table_md.push_str(&format_separator_row(num_columns));
    for row in &rows {
        table_md.push_str(&format_table_row(row, num_columns));
    }
    table_md.push('\n');

    Some(HandlerResult {
        content: table_md,
        markdown_translated,
    })
}

/// Collect eagerly because the children borrow cannot outlive this call.
fn row_children(node: &Rc<Node>) -> Vec<Rc<Node>> {
    node.children
        .borrow()
        .iter()
        .filter(|child| is_row(child))
        .cloned()
        .collect()
}

fn is_row(node: &Rc<Node>) -> bool {
    element_tag(node) == Some("tr")
}

/// Promote only nonempty all-`<th>` rows: a mixed label/value row is data, not
/// a fabricated column header. Ignore whitespace text nodes between cells.
fn row_is_all_header_cells(row_node: &Rc<Node>) -> bool {
    let children = row_node.children.borrow();
    let mut cells = children
        .iter()
        .filter_map(|cell| element_tag(cell).filter(|tag| matches!(*tag, "th" | "td")));
    let mut saw_cell = false;
    let all_th = cells.all(|tag| {
        saw_cell = true;
        tag == "th"
    });
    saw_cell && all_th
}

/// The first body-level row decides header candidacy; later rows remain data.
fn extract_data_row(
    handlers: &dyn Handlers,
    row_node: &Rc<Node>,
    header_decided: &mut bool,
    headers: &mut Vec<String>,
    rows: &mut Vec<Vec<String>>,
) -> bool {
    let (cells, translated) = extract_row_cells(handlers, row_node);
    if !*header_decided {
        *header_decided = true;
        if row_is_all_header_cells(row_node) {
            *headers = cells;
            return translated;
        }
    }
    if !cells.is_empty() {
        rows.push(cells);
    }
    translated
}

/// Read both cell kinds in source order, preserving handler conversion.
fn extract_row_cells(handlers: &dyn Handlers, row_node: &Rc<Node>) -> (Vec<String>, bool) {
    let mut cells = Vec::new();
    let mut markdown_translated = true;

    for cell in row_node.children.borrow().iter() {
        if !matches!(element_tag(cell), Some("th" | "td")) {
            continue;
        }
        let Some(res) = handlers.handle(cell) else {
            continue;
        };
        markdown_translated &= res.markdown_translated;
        cells.push(normalize_cell_content(&res.content));
    }

    (cells, markdown_translated)
}

/// Fold line breaks and escape pipes so content cannot split rows or columns.
/// Preserve NBSP and other non-document whitespace. GFM resolves `\|` inside
/// code spans, whereas `&#124;` remains literal there.
fn normalize_cell_content(content: &str) -> String {
    let content = content
        .replace('\n', " ")
        .replace('\r', "")
        .replace('|', "\\|");
    trim_document_whitespace(&content).to_owned()
}

/// Match htmd's document whitespace set: tab, newline, CR and space.
/// NBSP and other non-ASCII whitespace remain content.
fn trim_document_whitespace(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\r' | ' '))
}

/// Use fixed one-space cell padding, including empty or missing cells.
fn format_table_row(row: &[String], num_columns: usize) -> String {
    let mut line = String::from("|");
    for i in 0..num_columns {
        let cell = row.get(i).map(String::as_str).unwrap_or("");
        line.push(' ');
        line.push_str(cell);
        line.push_str(" |");
    }
    line.push('\n');
    line
}

fn format_separator_row(num_columns: usize) -> String {
    let mut line = String::from("|");
    for _ in 0..num_columns {
        line.push_str(" --- |");
    }
    line.push('\n');
    line
}

pub(super) fn to_fetch_result(
    article: &ExtractedArticle,
    url: String,
    decode_uncertain: bool,
) -> Result<FetchResult, FetchError> {
    // Fail-close: a conversion error must surface as a `FetchError`, not as an
    // empty or partial markdown body silently returned to the caller.
    let content_html = close_self_closed_raw_text_tags(&article.content_html);
    let markdown = markdown_converter()
        .convert(&content_html)
        .map_err(|e| FetchError::MarkdownConversion(e.to_string()))?;
    let output = format_with_frontmatter(article, &markdown);

    Ok(FetchResult {
        url,
        markdown: output,
        is_plain_text: false,
        used_raw_fallback: article.used_raw_fallback,
        decode_uncertain,
    })
}

/// Preserve decoded plain text without interpreting tags, entities or Markdown
/// syntax. It has no HTML metadata or Readability failure, but shares the same
/// YAML boundary defense and the caller's output cap as converted HTML.
pub(crate) fn plain_text_result(text: &str, url: String, decode_uncertain: bool) -> FetchResult {
    FetchResult {
        url,
        markdown: format_body("", text),
        is_plain_text: true,
        used_raw_fallback: false,
        decode_uncertain,
    }
}

/// Wraps `markdown` in a `---`-delimited YAML frontmatter block carrying
/// whichever of title/author/date the article provides. The wrapper remains
/// present when there are no metadata fields.
fn format_with_frontmatter(article: &ExtractedArticle, markdown: &str) -> String {
    let mut fields = String::new();

    if let Some(title) = &article.title {
        write_yaml_str(&mut fields, "title", title);
    }
    if let Some(author) = &article.byline {
        write_yaml_str(&mut fields, "author", author);
    }
    if let Some(date) = &article.published_time {
        write_yaml_str(&mut fields, "date", date);
    }

    format_body(&fields, markdown)
}

fn format_body(fields: &str, markdown: &str) -> String {
    // The body is untrusted page content appended after the frontmatter, so a
    // column-0 `---`/`...` in it would otherwise open a YAML document boundary.
    // A marker inside a closed fence is quoted sample output, not an attempt to
    // forge a document boundary, so it stays verbatim. Outside any fence, or
    // inside one that never closes, it is still rewritten to `***`.
    let body = neutralize_yaml_markers_outside_fences(markdown);

    let mut fm = String::from("---\n");
    fm.push_str(fields);
    fm.push_str("---\n\n");
    fm.push_str(&body);
    fm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::extractor::extract_article;
    use crate::search::engine::MAX_PAGE_BYTES;
    use crate::yaml::truncate_and_reneutralize;

    /// Hand-authored HTML fixture bypasses Readability. Class-driven behavior here
    /// therefore also applies to raw fetch; normal extraction strips classes.
    fn article(html: &str) -> ExtractedArticle {
        ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: html.into(),
            used_raw_fallback: false,
        }
    }

    /// [T-FC023] A blank line between header and separator would break the table.
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

    /// [T-FC024] A nested pre must stay inside its list item.
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

    /// [T-FC025] Preformatted cell newlines must not split the table row.
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

    /// [T-FC026] Parentheses in a destination must not close the Markdown link early.
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

    /// [T-FC014] Raw-fallback and decode-uncertain flags have different sources;
    /// swapping them or dropping either must be observable.
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

    /// [T-FC015] Code-block text must not acquire ordinary Markdown backslash escapes.
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

    /// [T-FC016] Content backticks must not close the surrounding code fence.
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

    /// [T-FC017] A language class supplies the fence info string. This fixture
    /// bypasses Readability; normal extraction strips classes (see `article`).
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

    /// [T-FC019] Bare pre content needs a fence even without a code child.
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

    /// [T-FC083] Bare-pre fencing must use a delimiter wider than content backticks.
    #[test]
    fn pre_without_code_child_widens_its_fence_past_a_backtick_run_in_the_content() {
        let article = article("<pre>a ``` b</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("````\na ``` b\n````"),
            "a <pre> whose content holds a 3-backtick run must be fenced with 4:\n{markdown}"
        );
    }

    /// [T-FC082] Caption and table stay adjacent. This asserts source formatting,
    /// not equivalent rendering across Markdown consumers.
    #[test]
    fn table_caption_precedes_the_header_row_without_a_blank_line() {
        let article = article(
            "<table><caption>Cap</caption><thead><tr><th>A</th><th>B</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Cap\n| A | B |"),
            "the caption must sit on the line directly above the header row:\n{markdown}"
        );
    }

    /// [T-FC020] Already-fenced code children must not receive a second fence.
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

    /// [T-FC021] Fenced bare-pre text must not retain htmd's leading fence escape.
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

    /// [T-FC022] Source backslashes inside pre content must survive unchanged.
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

    /// [T-FC027] A source backslash before a leading backtick is literal content.
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

    /// [T-FC029] htmd's extra leading backslash must not remain when a comment
    /// precedes the pre text.
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

    /// [T-FC028] Highlighted bare-pre text starting with backticks still needs fencing.
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

    /// [T-FC052] Span-edge newlines must separate following pre text.
    #[test]
    fn trailing_newline_at_the_end_of_a_span_inside_pre_survives_in_the_output() {
        let article = article("<pre><span>line1\n</span>line2</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1\nline2"),
            "the span's trailing newline must reach the sibling text as a real line break:\n{markdown}"
        );
    }

    /// [T-FC053] Per-line spans must not collapse code lines. Distinct `data-line`
    /// attributes prevent sibling merging from hiding per-span trimming.
    #[test]
    fn pre_with_one_span_per_line_keeps_each_line_on_its_own_output_line() {
        let article = article(
            "<pre><span data-line=\"1\">line1\n</span>\
             <span data-line=\"2\">line2\n</span>\
             <span data-line=\"3\">line3</span></pre>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1\nline2\nline3"),
            "each line-span's content must land on its own output line, in order:\n{markdown}"
        );
    }

    /// [T-FC054] Inline-code spans retain htmd trimming; only pre ancestors bypass it.
    /// The newline must be inside the span to exercise that handler.
    #[test]
    fn span_inside_inline_code_outside_pre_loses_the_newline_entirely() {
        let article = article("<p><code><span>line1\n</span>line2</code></p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1line2"),
            "a span inside inline code with no <pre> ancestor must fall through to htmd's \
             built-in span handler, whose trim_matches('\\n') strips the newline before the \
             code handler can fold it to a space:\n{markdown}"
        );
        assert!(
            !markdown.contains("line1\nline2"),
            "the newline must not survive raw:\n{markdown}"
        );
    }

    /// [T-FC060] Mixed header/data cells must both survive in their source row.
    #[test]
    fn label_and_value_from_a_mixed_th_td_row_land_in_the_same_row_in_separate_cells() {
        let article = article(
            "<table><tbody><tr><th>Name</th><td>Alice</td></tr>\
             <tr><th>Age</th><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        let name_row = markdown
            .lines()
            .find(|line| line.contains("Name"))
            .expect("a row carrying the Name label must be present");
        assert!(
            name_row.contains("Alice"),
            "the th label and its td value from the same source row must land in the same \
             output row:\n{markdown}"
        );

        let age_row = markdown
            .lines()
            .find(|line| line.contains("Age"))
            .expect("a row carrying the Age label must be present");
        assert!(
            age_row.contains("30"),
            "the th label and its td value from the same source row must land in the same \
             output row:\n{markdown}"
        );
    }

    /// [T-FC061] Unequal cell widths must not introduce column-alignment padding.
    #[test]
    fn table_row_between_pipes_has_no_run_of_two_or_more_spaces() {
        let article = article(
            "<table><thead><tr><th>Name</th><th>City</th></tr></thead>\
             <tbody><tr><td>Al</td><td>Springfield</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        let data_row = markdown
            .lines()
            .find(|line| line.contains("Al") && line.contains("Springfield"))
            .expect("the data row must be present");

        assert!(
            !data_row.contains("  "),
            "a table row must carry no run of two or more consecutive spaces \
             (no column-width alignment padding):\n{markdown}"
        );
    }

    /// [T-FC062] Separator width must remain three dashes per column.
    #[test]
    fn separator_row_has_exactly_three_dashes_per_cell() {
        let article = article(
            "<table><thead><tr><th>Name</th><th>Age</th></tr></thead>\
             <tbody><tr><td>Alice</td><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        // Anchored on the pipe so the frontmatter's own `---` delimiter cannot
        // stand in for the separator row.
        let separator_line = markdown
            .lines()
            .find(|line| line.starts_with("| ---"))
            .expect("a dash separator row must be present");

        assert_eq!(
            separator_line, "| --- | --- |",
            "the separator row must carry exactly three dashes per cell, unpadded to column \
             width:\n{markdown}"
        );
    }

    /// [T-FC063] Cell normalization must preserve literal NBSP.
    #[test]
    fn nbsp_inside_a_cell_survives_without_collapsing_to_a_space() {
        let article = article(
            "<table><thead><tr><th>Name</th><th>Value</th></tr></thead>\
             <tbody><tr><td>A\u{00A0}B</td><td>x</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("A\u{00A0}B"),
            "a literal NBSP inside a cell must survive unchanged, not collapse to an ASCII \
             space:\n{markdown}"
        );
    }

    /// [T-FC055] Pre spans must still convert nested elements, not copy raw markup.
    #[test]
    fn pre_newline_survives_when_the_neighboring_span_has_an_element_child() {
        let article = article("<pre><span>line1\n</span><span><b>line2</b></span></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1\n**line2**"),
            "the newline before a line-span with an element child must survive, and that \
             child element must still be converted to Markdown:\n{markdown}"
        );
    }

    /// [T-FC064] The first thead row keeps mixed cell kinds when promoted.
    #[test]
    fn header_promoted_row_keeps_its_td_cells() {
        let article = article(
            "<table><thead><tr><th>Name</th><td>Alice</td></tr></thead>\
             <tbody><tr><td>Bob</td><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        let header_line = markdown
            .lines()
            .find(|line| line.contains("Name"))
            .expect("the thead's first row must become the header row");
        assert!(
            header_line.contains("Alice"),
            "the row promoted to header must keep its td cell alongside its th cell, \
             not lose it:\n{markdown}"
        );
    }

    /// [T-FC065] Both cells of a later all-th row must survive on the same line.
    /// The position check also matches frontmatter, so it cannot detect header promotion.
    #[test]
    fn th_only_row_in_the_middle_of_the_body_appears_as_a_data_row() {
        let article = article(
            "<table><tbody><tr><td>Alice</td><td>30</td></tr>\
             <tr><th>Bob</th><th>40</th></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        let separator_idx = lines
            .iter()
            .position(|line| {
                !line.is_empty()
                    && line.contains('-')
                    && line.chars().all(|c| c == '|' || c == '-' || c == ' ')
            })
            .expect("a dash separator row must be present");
        let bob_idx = lines
            .iter()
            .position(|line| line.contains("Bob") && line.contains("40"))
            .expect("the th-only row's cells must land in the same row");

        assert!(
            bob_idx > separator_idx,
            "a th-only row that is not the table's first row must be emitted as a data row \
             after the separator, not promoted to header:\n{markdown}"
        );
    }

    /// [T-FC066] Both cells of a later thead row must survive on the same line.
    #[test]
    fn second_and_later_rows_of_a_multi_row_thead_appear_as_data_rows() {
        let article = article(
            "<table><thead><tr><th>Name</th><th>Age</th></tr>\
             <tr><th>Category A</th><th>Category B</th></tr></thead>\
             <tbody><tr><td>Alice</td><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        markdown
            .lines()
            .find(|line| line.contains("Category A") && line.contains("Category B"))
            .expect(
                "the thead's second row must survive as a data row, not be dropped from the \
                 output",
            );
    }

    /// [T-FC068] Non-Pure mode must delegate to htmd. The fixture needs an attribute
    /// to trigger its `serialize_if_faithful!` gate.
    #[test]
    fn faithful_mode_table_with_attributes_delegates_to_the_built_in_handler_and_stays_html() {
        use htmd::options::{Options, TranslationMode};

        let options = Options {
            translation_mode: TranslationMode::Faithful,
            ..Options::default()
        };
        let converter = HtmlToMarkdown::builder()
            .options(options)
            .add_handler(vec!["pre"], pre_handler)
            .add_handler(vec!["span"], span_handler)
            .add_handler(vec!["table"], table_handler)
            .build();

        let html = r#"<table class="data"><thead><tr><th>Name</th></tr></thead><tbody><tr><td>Alice</td></tr></tbody></table>"#;
        let markdown = converter.convert(html).expect("conversion must succeed");

        assert!(
            markdown.contains("<table"),
            "a table with attributes under Faithful mode must delegate to htmd's built-in \
             table handler and come out as raw HTML, not the app's positional Markdown \
             table:\n{markdown}"
        );
        assert!(
            !markdown.contains("| Name |"),
            "the app's own pipe-delimited table formatting must not run under Faithful \
             mode:\n{markdown}"
        );
    }

    /// [T-FC067] A mixed label/value row must not fabricate column headers.
    #[test]
    fn table_with_no_all_th_row_emits_an_empty_header_row_and_separator() {
        let article = article(
            "<table><tbody><tr><th>Name</th><td>Alice</td></tr>\
             <tr><th>Age</th><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        let header_idx = lines
            .iter()
            .position(|line| *line == "|  |  |")
            .expect("an empty header row must be present when no row qualifies as a header");
        let separator_line = lines
            .get(header_idx + 1)
            .expect("a line must immediately follow the empty header row");
        assert_eq!(
            *separator_line, "| --- | --- |",
            "the line right after the empty header row must be the dash separator row:\n{markdown}"
        );
        assert!(
            markdown.contains("| Name | Alice |") && markdown.contains("| Age | 30 |"),
            "both row-heading rows must stay in the body after the empty header, label and \
             value together:\n{markdown}"
        );
    }

    /// [T-FC069] The first all-header body row must be promoted without a thead.
    #[test]
    fn all_th_first_body_row_becomes_the_header_without_a_thead() {
        let article = article(
            "<table><tbody><tr><th>Name</th><th>Age</th></tr>\
             <tr><td>Alice</td><td>30</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        let header_idx = lines
            .iter()
            .position(|line| *line == "| Name | Age |")
            .expect("the all-th first body row must become the header row");
        assert_eq!(
            lines.get(header_idx + 1).copied(),
            Some("| --- | --- |"),
            "the separator row must follow the promoted header row:\n{markdown}"
        );
        assert_eq!(
            lines.get(header_idx + 2).copied(),
            Some("| Alice | 30 |"),
            "the remaining row must stay a data row under the header:\n{markdown}"
        );
    }

    /// [T-FC070] Converted caption content must precede the table.
    #[test]
    fn table_caption_precedes_the_header_row() {
        let article = article(
            "<table><caption>Population</caption>\
             <thead><tr><th>City</th><th>Count</th></tr></thead>\
             <tbody><tr><td>Osaka</td><td>2</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        let caption_idx = lines
            .iter()
            .position(|line| line.contains("Population"))
            .expect("the caption's text must reach the output");
        let header_idx = lines
            .iter()
            .position(|line| *line == "| City | Count |")
            .expect("the header row must be present");
        assert!(
            caption_idx < header_idx,
            "the caption must precede the header row:\n{markdown}"
        );
    }

    /// [T-FC071] A rowless table must return child content, not a zero-column table.
    #[test]
    fn table_with_no_rows_falls_back_to_its_walked_content() {
        let article = article("<table><caption>Empty</caption></table>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Empty"),
            "the table's own content must survive the fallback:\n{markdown}"
        );
        assert!(
            !markdown.contains("---|") && !markdown.contains("| ---"),
            "a table with no rows must not emit a separator row:\n{markdown}"
        );
    }

    /// [T-FC072] Whitespace between cells must not drop either row's cell content
    /// or split it across output lines. Header placement is not checked.
    #[test]
    fn cells_are_extracted_from_a_tr_split_across_source_lines() {
        let article = article(
            "<table>\n  <tr>\n    <th>Name</th>\n    <th>Age</th>\n  </tr>\n\
             \n  <tr>\n    <td>Alice</td>\n    <td>30</td>\n  </tr>\n</table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("| Name | Age |"),
            "the whitespace between cells must not stop the all-th row from becoming the \
             header:\n{markdown}"
        );
        assert!(
            markdown.contains("| Alice | 30 |"),
            "both cells of the data row must be extracted past the whitespace text \
             nodes:\n{markdown}"
        );
    }

    /// [T-FC034] Fence-character preservation must also hold after preceding text.
    #[test]
    fn second_pre_child_text_starting_with_backtick_survives_unstripped() {
        let article = article("<pre>abc<span>X</span>`def</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("```\nabcX`def\n```"),
            "the second text node's leading backtick must survive unescaped inside the \
             fence:\n{markdown}"
        );
        assert!(
            !markdown.contains(r"abcX\`def"),
            "htmd's escape on the second text node must not survive as a literal \
             backslash:\n{markdown}"
        );
    }

    /// [T-FC035] Fence-character preservation must hold after a converted element.
    #[test]
    fn backtick_after_a_preceding_element_child_survives_unstripped() {
        let article = article("<pre><span>abc</span>`def</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("```\nabc`def\n```"),
            "the backtick following a preceding element child must survive unescaped inside \
             the fence:\n{markdown}"
        );
        assert!(
            !markdown.contains(r"abc\`def"),
            "htmd's escape must not survive as a literal backslash when an element precedes \
             the text node:\n{markdown}"
        );
    }

    /// [T-FC036] DOM-based text preservation must hold through nested pre handling.
    #[test]
    fn inner_pre_backtick_survives_unstripped_when_pre_is_nested() {
        let article = article("<pre><pre>abc<span>X</span>`def</pre></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("abcX`def"),
            "the inner pre's second text node's leading backtick must survive unescaped:\n{markdown}"
        );
        assert!(
            !markdown.contains(r"abcX\`def"),
            "htmd's escape on the inner pre's second text node must not survive as a literal \
             backslash:\n{markdown}"
        );
    }

    /// [T-FC037] Line breaks survive between same-tag, same-attribute spans
    /// that htmd merges as siblings inside pre.
    #[test]
    fn newlines_survive_across_merged_same_tag_same_attrs_spans_in_pre() {
        let article =
            article("<pre><span>line1\n</span><span>line2\n</span><span>line3</span></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1\nline2\nline3"),
            "each line's newline must survive as a real line break even when htmd merges the \
             same-tag, same-attrs spans into one node before handling:\n{markdown}"
        );
    }

    /// [T-FC038] Adjacent block children must not accumulate extra blank lines.
    #[test]
    fn adjacent_block_children_boundary_keeps_newlines_capped_at_two() {
        let article = article("<pre><div>ALPHA</div><div>BETA</div></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        let alpha_end = markdown
            .find("ALPHA")
            .expect("first block child's text must be present")
            + "ALPHA".len();
        let beta_start = markdown
            .find("BETA")
            .expect("second block child's text must be present");
        let between = &markdown[alpha_end..beta_start];
        let newline_run = between.chars().filter(|&c| c == '\n').count();

        assert!(
            newline_run <= 2,
            "the boundary between adjacent block children must carry at most 2 newlines, \
             got {newline_run}:\n{markdown}"
        );
    }

    /// [T-FC039] A direct pre child br must retain its Markdown hard break.
    #[test]
    fn br_directly_under_pre_survives_as_two_trailing_spaces_and_a_newline() {
        let article = article("<pre>line1<br>line2</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1  \nline2"),
            "a <br> directly under <pre> must leave two trailing spaces before the newline \
             it introduces:\n{markdown}"
        );
    }

    /// [T-FC040] Unregistered child tags must still preserve pre text without escaping.
    #[test]
    fn unregistered_tag_child_text_inside_pre_is_not_escaped() {
        let article = article("<pre><mark>a_b*c</mark></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("a_b*c"),
            "an unregistered tag's text content inside <pre> must survive without \
             escape-target characters gaining a backslash:\n{markdown}"
        );
    }

    /// [T-FC041] Ordinary paragraph newlines retain htmd whitespace folding.
    #[test]
    fn a_newline_inside_a_paragraph_collapses_to_a_single_space() {
        let article = article("<p>line1\nline2</p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1 line2"),
            "a newline inside a paragraph's text must collapse to a single space:\n{markdown}"
        );
        assert!(
            !markdown.contains("line1\nline2"),
            "the source newline must not survive as a literal line break:\n{markdown}"
        );
    }

    /// [T-FC042] A paragraph br must retain the default Markdown hard break.
    #[test]
    fn br_inside_a_paragraph_survives_as_two_trailing_spaces_and_a_newline() {
        let article = article("<p>line1<br>line2</p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1  \nline2"),
            "a <br> inside a paragraph must leave two trailing spaces before the newline it \
             introduces:\n{markdown}"
        );
    }

    /// [T-FC043] Bare-pre source newlines must survive inside the fence.
    #[test]
    fn pre_content_is_not_collapsed_and_keeps_its_newline() {
        let article = article("<pre>line1\nline2</pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("```\nline1\nline2\n```"),
            "a <pre> block's internal newline must survive as a real line break, not collapse \
             to a space:\n{markdown}"
        );
    }

    /// [T-FC044] Inline code must not collapse consecutive spaces.
    #[test]
    fn consecutive_spaces_inside_inline_code_survive_unchanged() {
        let article = article("<p>a <code>x   y</code> b</p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("`x   y`"),
            "three consecutive spaces inside inline code must survive without collapsing to a \
             single space:\n{markdown}"
        );
    }

    /// [T-FC045] A cell br must not introduce a row-breaking newline.
    #[test]
    fn br_inside_a_table_cell_loses_the_line_break_and_collapses_to_whitespace() {
        let article = article("<table><tr><td>line1<br>line2</td></tr></table>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("| line1   line2 |"),
            "a <br> inside a table cell must collapse to a run of spaces between the text on \
             either side of it, not survive as a line break:\n{markdown}"
        );
        assert!(
            !markdown.contains("line1  \nline2"),
            "the hard-break form a <br> takes in a paragraph or <pre> must not survive inside \
             a table cell, which cannot hold a literal newline:\n{markdown}"
        );
    }

    /// [T-FC046] List-item br output retains htmd indentation and trimming.
    #[test]
    fn br_inside_a_list_item_loses_its_trailing_spaces_and_becomes_an_indented_newline() {
        let article = article("<ul><li>line1<br>line2</li></ul>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1\n    line2"),
            "a <br> inside a list item must leave no trailing spaces on the line before it, \
             and the text after it must reappear indented under the bullet on its own \
             line:\n{markdown}"
        );
        assert!(
            !markdown.contains("line1  \n"),
            "the hard-break form a <br> takes in a paragraph must not survive inside a list \
             item, where the indent step trims it away:\n{markdown}"
        );
    }

    /// [T-FC047] A heading br leaves the next line unmarked, as htmd emits it.
    #[test]
    fn text_after_a_br_inside_a_heading_lands_outside_the_heading_line() {
        let article = article("<h2>line1<br>line2</h2>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("## line1  \nline2"),
            "text after a <br> inside a heading must land on its own line below the '#' \
             marker, carrying the same hard-break form a <br> produces inside a paragraph:\n\
             {markdown}"
        );
        assert!(
            !markdown
                .lines()
                .any(|line| line.starts_with('#') && line.contains("line2")),
            "the text after a <br> inside a heading must not end up inside the heading's own \
             '#'-prefixed line:\n{markdown}"
        );
    }

    /// [T-FC048] Empty per-line highlighter anchors must not add links to code.
    #[test]
    fn empty_anchor_inside_pre_disappears_leaving_the_original_line_and_indentation() {
        let article = article(
            "<pre><a href=\"#__codelineno-0-1\"></a>    def foo():\n\
             <a href=\"#__codelineno-0-2\"></a>        return 1\n</pre>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            !markdown.contains("__codelineno"),
            "an empty anchor pointing only at a fragment must leave no trace of its href:\n{markdown}"
        );
        assert!(
            markdown.lines().any(|line| line == "    def foo():"),
            "the first original line and its indentation must survive with the anchor removed:\n{markdown}"
        );
        assert!(
            markdown.lines().any(|line| line == "        return 1"),
            "the second original line and its indentation must survive with the anchor removed:\n{markdown}"
        );
    }

    /// [T-FC049] A fragment-anchor control distinguishes selective suppression
    /// from dropping every empty anchor, including absolute links with titles.
    #[test]
    fn empty_anchor_to_absolute_url_keeps_destination_and_title() {
        let article = article(
            "<p>See <a href=\"https://example.com/target\" title=\"Target page\"></a> \
             and <a href=\"#top\"></a> for details.</p>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("[](https://example.com/target \"Target page\")"),
            "an empty anchor pointing at an absolute URL must keep its destination and title:\n{markdown}"
        );
        assert!(
            !markdown.contains("#top"),
            "the fragment-only sibling anchor must be suppressed, not just the absolute one kept:\n{markdown}"
        );
    }

    /// [T-FC050]
    #[test]
    fn titled_headerlink_with_empty_content_is_removed() {
        let article = article(
            "<h2>Section<a class=\"headerlink\" href=\"#section\" \
             title=\"Link to this heading\"></a></h2>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("## Section"),
            "the heading text must survive with the headerlink removed:\n{markdown}"
        );
        assert!(
            !markdown.contains("Link to this heading"),
            "the headerlink's title must not survive:\n{markdown}"
        );
        assert!(
            !markdown.contains("#section"),
            "the headerlink's href must not survive:\n{markdown}"
        );
    }

    /// [T-FC051] Text in a missing-href anchor survives without link brackets;
    /// the empty fragment-only sibling is suppressed.
    #[test]
    fn anchor_without_href_delegates_to_the_builtin_handler() {
        let article = article("<p><a>plain text</a> and <a href=\"#nav\"></a></p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("plain text") && !markdown.contains("[plain text]"),
            "an <a> with no href must fall through to the builtin handler's walk-children \
             behavior, not gain link brackets:\n{markdown}"
        );
        assert!(
            !markdown.contains("#nav"),
            "the fragment-only sibling anchor must be suppressed, not just the hrefless one delegated:\n{markdown}"
        );
    }

    /// [T-FC077] Empty href resolves to the same page and must not emit an empty link.
    #[test]
    fn anchor_with_an_empty_href_and_no_content_is_suppressed() {
        let article = article("<p><a href=\"\" title=\"here\"></a>tail</p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            !markdown.contains("[]("),
            "an empty-href anchor with no content must be suppressed, not emitted as an \
             empty link carrying its title:\n{markdown}"
        );
        assert!(
            markdown.contains("tail"),
            "the text after the suppressed anchor must survive:\n{markdown}"
        );
    }

    /// [T-FC073] Delegated nonempty links must not retain redundant title suffixes.
    #[test]
    fn title_disappears_from_the_output_of_a_link_that_has_link_text() {
        let article = article(
            "<p><a href=\"https://example.com/target\" title=\"My Title\">link text</a></p>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("[link text](https://example.com/target)"),
            "a titled link with link text must lose its title, leaving a bare \
             `[text](url)`:\n{markdown}"
        );
        assert!(
            !markdown.contains("My Title"),
            "the title text must not survive anywhere in the output:\n{markdown}"
        );
    }

    /// [T-FC074] Escaped quotes in a delegated title must not defeat suffix removal.
    #[test]
    fn title_containing_double_quotes_does_not_escape_the_rewrite() {
        let article = article(
            "<p><a href=\"https://example.com/target\" title=\"say &quot;hi&quot;\">\
             link text</a></p>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("[link text](https://example.com/target)"),
            "a title holding double quotes must still be dropped in full, leaving a bare \
             `[text](url)`:\n{markdown}"
        );
        assert!(
            !markdown.contains("hi"),
            "no fragment of the quoted title text must survive:\n{markdown}"
        );
    }

    /// [T-FC075] Title reflow must not defeat suffix matching across lines.
    #[test]
    fn title_containing_a_newline_does_not_escape_the_rewrite() {
        let article = article(
            "<p><a href=\"https://example.com/target\" title=\"line1\nline2\">\
             link text</a></p>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("[link text](https://example.com/target)"),
            "a title holding a newline must still be dropped in full, leaving a bare \
             `[text](url)`:\n{markdown}"
        );
        assert!(
            !markdown.contains("line1") && !markdown.contains("line2"),
            "no fragment of the multi-line title text must survive:\n{markdown}"
        );
    }

    /// [T-FC076] Whitespace-only titles still produce an empty suffix in htmd;
    /// that suffix must be removed from nonempty links.
    #[test]
    fn whitespace_only_title_is_treated_as_no_title() {
        let article =
            article("<p><a href=\"https://example.com/target\" title=\"   \">link text</a></p>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("[link text](https://example.com/target)"),
            "a whitespace-only title must leave a bare `[text](url)` with no empty title \
             syntax:\n{markdown}"
        );
        assert!(
            !markdown.contains("\"\""),
            "no empty title marker must survive:\n{markdown}"
        );
    }

    /// [T-FC084] Pure-mode block handling must not leak script or style source.
    #[test]
    fn script_and_style_content_left_in_content_html_does_not_reach_the_body() {
        let article = article(
            "<div><p>Visible text</p>\
             <script>var scriptSecret = 'do-not-show';</script>\
             <style>.hiddenStyleRule { color: red; }</style></div>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Visible text"),
            "ordinary sibling text must still reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("scriptSecret"),
            "a <script> element's source text must not reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("hiddenStyleRule"),
            "a <style> element's source text must not reach the body:\n{markdown}"
        );
    }

    /// [T-FC085] Suppress hidden bodies in both registered and fallback tag paths.
    /// With scripting enabled, noscript and iframe children are parsed as raw text.
    #[test]
    fn noscript_textarea_iframe_child_and_svg_desc_title_content_do_not_reach_the_body() {
        let article = article(
            "<div><p>Visible text</p>\
             <noscript>noscript fallback content</noscript>\
             <textarea>textarea leaked content</textarea>\
             <iframe><p>iframe fallback content</p></iframe>\
             <svg><title>svg title content</title><desc>svg desc content</desc></svg>\
             </div>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Visible text"),
            "ordinary sibling text must still reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("noscript fallback content"),
            "a <noscript> element's content must not reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("textarea leaked content"),
            "a <textarea> element's content must not reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("iframe fallback content"),
            "an <iframe> element's child content must not reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("svg title content"),
            "an <svg> element's <title> content must not reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("svg desc content"),
            "an <svg> element's <desc> content must not reach the body:\n{markdown}"
        );
    }

    /// [T-FC086] Raw extraction bypasses Readability cleanup; suppression must
    /// still hold through the real extraction-to-conversion seam.
    #[test]
    fn raw_extraction_end_to_end_does_not_leak_script_content_into_the_body() {
        let html = "<html><head><title>Page</title></head><body>\
             <p>Visible text</p>\
             <script>var rawPathSecret = 'do-not-show';</script>\
             </body></html>";
        let raw_article = super::super::extractor::extract_raw(html);

        let result = to_fetch_result(&raw_article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Visible text"),
            "ordinary sibling text must still reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("rawPathSecret"),
            "a <script> element's source text must not reach the body on the raw \
             end-to-end path:\n{markdown}"
        );
    }

    /// [T-FC089] XHTML self-closed raw-text tags must not swallow the rest of the
    /// body when htmd parses them as HTML. Cover script and iframe.
    #[test]
    fn body_after_a_self_closed_raw_text_tag_still_reaches_the_body() {
        let article = article(
            "<div><p>Before script</p><script src=\"app.js\" />\
             <p>After script</p><iframe src=\"embed.html\"/>\
             <p>After iframe</p></div>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Before script"),
            "text ahead of the self-closed tag must reach the body:\n{markdown}"
        );
        assert!(
            markdown.contains("After script"),
            "text after a self-closed <script /> must reach the body:\n{markdown}"
        );
        assert!(
            markdown.contains("After iframe"),
            "text after a self-closed <iframe /> must reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("app.js"),
            "the rewritten <script> must still be suppressed, attributes \
             included:\n{markdown}"
        );
    }

    /// [T-FC092] A self-closing tag inside a JS string must stay raw text;
    /// rewriting it would end the script early and leak the remaining source.
    #[test]
    fn a_script_tag_inside_js_source_is_not_rewritten_into_an_early_close() {
        let article = article(
            "<div><p>Visible text</p>\
             <script>document.write('<script src=\"x\" />'); var leaked = 'nestedSecret';</script>\
             </div>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("Visible text"),
            "ordinary sibling text must still reach the body:\n{markdown}"
        );
        assert!(
            !markdown.contains("nestedSecret"),
            "script source following a <script … /> written inside it must not \
             reach the body:\n{markdown}"
        );
    }

    /// [T-FC090] Tag-name prefixes and quoted `>` must not confuse the byte scan;
    /// ordinary closing tags must not receive another end tag.
    #[test]
    fn self_closed_tag_rewrite_respects_name_boundaries_and_quoted_attributes() {
        assert_eq!(
            close_self_closed_raw_text_tags("<scriptlet a=\"b\" />x"),
            "<scriptlet a=\"b\" />x",
            "a longer tag name starting with a target name must not be rewritten"
        );
        assert_eq!(
            close_self_closed_raw_text_tags("<script data-x=\"a>b\" />x"),
            "<script data-x=\"a>b\" ></script>x",
            "a > inside a quoted attribute value must not end the start tag"
        );
        assert_eq!(
            close_self_closed_raw_text_tags("<script>var a = 1;</script>x"),
            "<script>var a = 1;</script>x",
            "an ordinarily closed tag must pass through unchanged"
        );
    }

    /// [T-FC091] Namespace-specific suppression must preserve visible HTML desc
    /// text while removing SVG desc text.
    #[test]
    fn desc_outside_the_svg_namespace_keeps_its_text_in_the_body() {
        let article = article(
            "<div><p>before <desc>html desc content</desc> after</p>\
             <svg><desc>svg desc content</desc></svg></div>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("html desc content"),
            "a <desc> outside the SVG namespace must keep its text:\n{markdown}"
        );
        assert!(
            !markdown.contains("svg desc content"),
            "an SVG <desc> must still be suppressed:\n{markdown}"
        );
    }

    /// [T-FC078] A pre/code pair inside a table cell must use inline code.
    #[test]
    fn table_cell_code_block_renders_as_inline_code_with_one_backtick_delimiter() {
        let article = article(
            "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
                <pre><code>let x = 1;</code></pre></td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("| `let x = 1;` |"),
            "a table-cell code block must render as inline code with a \
             1-backtick delimiter:\n{markdown}"
        );
        assert!(
            !markdown.contains("```"),
            "a table-cell code block must not carry a 3-backtick fence:\n{markdown}"
        );
    }

    /// [T-FC093] A bare pre inside a cell must also avoid block fencing.
    #[test]
    fn table_cell_pre_without_a_code_child_renders_as_inline_code() {
        let article =
            article("<table><tbody><tr><td><pre>x\ny</pre></td><td>b</td></tr></tbody></table>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            !markdown.contains("```"),
            "a bare <pre> in a table cell must not leave fence backticks in the \
             cell:\n{markdown}"
        );
        assert!(
            markdown.contains("`x y`"),
            "a bare <pre> in a table cell must render as inline code:\n{markdown}"
        );
    }

    /// [T-FC094] Cell pre text beside a code child must survive.
    #[test]
    fn table_cell_pre_keeps_text_outside_its_code_child() {
        let article = article(
            "<table><tbody><tr><td><pre>prefix<code>x</code>suffix</pre></td>\
                <td>b</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("`prefixxsuffix`"),
            "text on either side of a <code> child must survive the cell's \
             inline-code rendering:\n{markdown}"
        );
    }

    /// [T-FC095] Direct DOM reads for cell pre content must also suppress scripts.
    #[test]
    fn table_cell_pre_drops_the_body_of_a_suppressed_element() {
        let article = article(
            "<table><tbody><tr><td><pre><code>visible\
                <script>hidden()</script></code></pre></td><td>b</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            !markdown.contains("hidden()"),
            "a <script> body inside a table cell's <pre> must not reach the \
             markdown:\n{markdown}"
        );
        assert!(
            markdown.contains("`visible`"),
            "the cell's visible code text must survive the suppression:\n{markdown}"
        );
    }

    /// [T-FC096] A br without a Text child must still separate cell pre words.
    #[test]
    fn table_cell_pre_keeps_a_br_as_a_visible_separator() {
        let article = article(
            "<table><tbody><tr><td><pre><code>a<br>b</code></pre></td>\
                <td>c</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("`a b`"),
            "a <br> inside a table cell's <pre> must separate the two lines \
             rather than joining them:\n{markdown}"
        );
    }

    /// [T-FC079] Cell-code delimiters must exceed the longest content backtick run.
    #[test]
    fn table_cell_code_delimiter_widens_to_four_backticks_when_content_has_a_three_backtick_run() {
        let article = article(
            "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
                <pre><code>a ``` b</code></pre></td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("| ````a ``` b```` |"),
            "a 3-backtick run in the cell content must widen the delimiter to \
             4 backticks:\n{markdown}"
        );
    }

    /// [T-FC080] Inner spaces separate content-edge backticks from delimiters.
    #[test]
    fn table_cell_code_delimiter_gets_inner_space_when_content_starts_and_ends_with_backtick() {
        let article = article(
            "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
                <pre><code>`code`</code></pre></td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("| `` `code` `` |"),
            "content starting and ending with a backtick must get a single \
             inner space next to each delimiter:\n{markdown}"
        );
    }

    /// [T-FC081] Pre/code outside cells must retain block fencing.
    #[test]
    fn pre_outside_a_table_still_renders_as_a_fenced_block() {
        let article = article("<pre><code>fn main() {}</code></pre>");

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("```\nfn main() {}\n```"),
            "a <pre><code> outside any table must still render as a fenced \
             block:\n{markdown}"
        );
    }

    /// [T-FC104] Field caps must leave a closed frontmatter and body after
    /// the caller's output cap, not merely shorten individual fields.
    #[test]
    fn a_title_over_the_cap_still_yields_a_closed_frontmatter_and_a_body() {
        let article = ExtractedArticle {
            title: Some("T".repeat(120_000)),
            byline: None,
            published_time: None,
            content_html: "<p>body text</p>".to_owned(),
            used_raw_fallback: false,
        };

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let cut = truncate_and_reneutralize(result.markdown(), MAX_PAGE_BYTES);
        let delimiters = cut.lines().filter(|l| *l == "---").count();

        assert_eq!(
            delimiters, 2,
            "the frontmatter must open and close within the cap:\n{cut}"
        );
        assert!(
            cut.contains("body text"),
            "the body must survive a title that would otherwise fill the cap:\n{cut}"
        );
    }

    /// [T-FC098] Escape cell-code pipes with `\|`; GFM does not decode entities
    /// inside code spans.
    #[test]
    fn table_cell_code_span_escapes_a_pipe_with_a_backslash() {
        let article = article(
            "<table><tbody><tr><td><pre><code>a | b</code></pre></td><td>x</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains(r"`a \| b`"),
            "a pipe inside a cell's code span must be backslash-escaped:\n{markdown}"
        );
        assert!(
            !markdown.contains("&#124;"),
            "an entity reference would show as literal text inside the code span:\n{markdown}"
        );
    }

    /// [T-FC099] Row padding must keep a trailing content backslash from escaping
    /// the next cell delimiter. Current inline-code output closes with a backtick
    /// so it cannot produce a bare trailing backslash at that boundary.
    #[test]
    fn table_row_keeps_a_space_between_a_trailing_backslash_and_the_closing_pipe() {
        // htmd leaves a backslash inside a code span unescaped (T-FC015).
        let article = article(
            "<table><tbody><tr><td><pre><code>a\\</code></pre></td><td>second</td></tr></tbody></table>",
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            markdown.contains("`a\\` | second |"),
            "a trailing backslash must not run into the closing pipe:\n{markdown}"
        );
    }

    /// [T-FC097] Hidden-source suppression must hold through Readability's raw
    /// fallback, not only hand-built converter fixtures.
    #[test]
    fn readability_failure_path_still_drops_script_content() {
        // Avoid `_` in the leak marker: htmd escapes it and a literal substring
        // assertion could then miss leaked script text.
        let html = "<script>SCRIPTMARKER</script>";
        let article = extract_article(html, Some("https://example.com"));
        assert!(
            article.used_raw_fallback,
            "fixture must take the fallback for this test to cover that path"
        );

        let result = to_fetch_result(&article, "https://example.com".into(), false).unwrap();
        let markdown = result.markdown();

        assert!(
            !markdown.contains("SCRIPTMARKER"),
            "a script body must not reach the output on the fallback path:\n{markdown}"
        );
    }
}
