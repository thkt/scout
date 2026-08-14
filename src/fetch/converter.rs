use std::rc::{Rc, Weak};

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::options::{Options, TranslationMode};
use htmd::{HtmlToMarkdown, Node};
use markup5ever_rcdom::NodeData;
use serde::Serialize;

use super::FetchError;
use super::extractor::ExtractedArticle;
use crate::markdown::fence_delimiter;
use crate::yaml::{neutralize_yaml_markers_outside_fences, write_yaml_str};

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
        .add_handler(vec!["table"], table_handler)
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
    // Called for its side effect as much as its value: htmd's own walk runs
    // the adjacent-sibling merge on `element.node.children` before either
    // branch below reads them (see `raw_pre_content`'s doc comment). The
    // walked string itself (`result.content`) is used only by the
    // `<pre><code>` branch, which htmd already fences correctly on its own.
    let result = handlers.walk_children(element.node);

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

/// Whether the element has a direct `<code>` child, the shape htmd's
/// `code_handler` fences on its own: it fences exactly when the `<code>`
/// element's parent is `<pre>` (htmd-0.5.5/src/element_handler/code.rs:33-41).
fn has_code_child(node: &Rc<Node>) -> bool {
    node.children.borrow().iter().any(|child| {
        matches!(&child.data, NodeData::Element { name, .. } if name.local.as_ref() == "code")
    })
}

/// Rebuilds a `<pre>` element's non-code content from its DOM children
/// instead of htmd's own walked text, so a text child opens with its source
/// character unescaped no matter where it sits among its siblings.
///
/// `Handlers::walk_children`, called once in `pre_handler` before this runs
/// and its own returned string discarded, already ran htmd's adjacent-sibling
/// merge on `node.children` (`dom_walker::can_combine`, gated on
/// `attrs1 == attrs2`, htmd-0.5.5/src/dom_walker.rs:243-297), so a run of
/// same-tag same-attrs `<span>`s reaches this loop as the single merged node
/// T-FC037 pins.
///
/// A direct Text child is appended as written: `escape_pre_text_if_needed`
/// backslash-escapes a leading `` ` `` or `~` only while htmd walks the text
/// itself (htmd-0.5.5/src/dom_walker.rs:34-41, 423-436), so reading the
/// child's `contents` straight off the DOM here never introduces that
/// backslash in the first place, at any child position — the case T-FC021,
/// T-FC027, and T-FC034-036 cover.
///
/// An Element child (a nested `<pre>`, a `<span>`, inline markup, ...) still
/// goes through `Handlers::handle`, converting the ordinary way.
/// `markdown_translated` aggregates only from those conversions: a Text child
/// never turns it false (`dom_walker::walk_node`'s `NodeData::Text` arm never
/// touches the flag, which starts `true`).
fn raw_pre_content(handlers: &dyn Handlers, node: &Rc<Node>) -> (String, bool) {
    let mut content = String::new();
    let mut markdown_translated = true;
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => content.push_str(&contents.borrow()),
            NodeData::Element { .. } => {
                if let Some(res) = handlers.handle(child) {
                    markdown_translated &= res.markdown_translated;
                    content.push_str(&res.content);
                }
            }
            _ => {}
        }
    }
    (content, markdown_translated)
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
/// `has_pre_ancestor` below looks for a `<pre>` ancestor only, narrower than
/// htmd's own `is_inside_pre` (htmd-0.5.5/src/element_handler/mod.rs:358-367),
/// which counts a `<code>` ancestor as "inside pre" too. A `<span>` in inline
/// `<code>` with no `<pre>` ancestor therefore falls to `Handlers::fallback`
/// and htmd's built-in `span_handler` strips the newline from both edges of
/// its content, before the inline-code handler's newline-to-space folding can
/// reach it. T-FC054 pins that, and DR-0025 records why the narrower check
/// stays.
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

/// Fixes htmd's built-in `table_handler`'s per-tag row extraction
/// (`extract_row_cells(handlers, row_node, "th")` /
/// `extract_row_cells(handlers, row_node, "td")`,
/// htmd-0.5.5/src/element_handler/table.rs:223-247): a row is scanned once
/// per cell tag, so a `<tr>` mixing `<th>` and `<td>` — a label/value row with
/// no `<thead>` — loses whichever tag that row's extraction call did not ask
/// for (table.rs:83-100: the `tbody`/`tfoot` branch takes only the row's
/// `<th>` cells for a candidate header row and `continue`s past its `<td>`
/// cells once that extraction is non-empty; a later mixed row falls to the
/// `<td>`-only extraction and loses its `<th>` label the same way). This
/// handler instead reads each row's cells positionally in one pass
/// (`extract_row_cells` below), so a label and its value from the same
/// source row land in the same output row, in separate cells.
///
/// Row and separator formatting drop the built-in's column-width alignment
/// padding, which widens every cell and dash run out to the column's longest
/// cell. `format_table_row` below writes one space on each side of every cell
/// regardless of a neighboring cell's width, so an empty or missing cell
/// renders as a pipe and two spaces, and `format_separator_row` writes a
/// fixed 3-dash run per cell.
///
/// Cell-content newline normalization, caption handling, and column-count
/// estimation follow the built-in's shape.
///
/// Any non-`Pure` translation mode falls straight to `Handlers::fallback`,
/// which reaches the built-in `table_handler` and its own
/// `serialize_if_faithful!` gate (table.rs:19-24) rather than the positional
/// extraction below. `markdown_converter` always builds with
/// `Options::default()`, which is `TranslationMode::Pure`, so scout's own
/// runtime never takes that branch; T-FC068 exercises it through a
/// `Faithful`-mode converter built in the test.
// `Element` must stay by-value for the same `add_handler` signature reason as
// `pre_handler` above.
#[allow(clippy::needless_pass_by_value)]
fn table_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if handlers.options().translation_mode != TranslationMode::Pure {
        return handlers.fallback(element);
    }

    let mut captions: Vec<String> = Vec::new();
    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    // Whether the header search has already been resolved: by a `thead`'s
    // first row, which always resolves it, or by the table's first row outside
    // a `thead`, which resolves it whether or not `row_is_all_header_cells`
    // promotes that row. Once true, every remaining row is a data row, a later
    // all-`<th>` row and a `thead`'s second-and-later rows included. The search
    // never re-opens past the first candidate, so no row order can rearrange
    // the body.
    let mut header_decided = false;
    let mut markdown_translated = true;

    for child in element.node.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else {
            continue;
        };
        match name.local.as_ref() {
            "caption" => {
                if let Some(res) = handlers.handle(child) {
                    markdown_translated &= res.markdown_translated;
                    captions.push(trim_document_whitespace(&res.content).to_owned());
                }
            }
            "thead" => {
                let mut thead_rows = row_children(child).into_iter();
                // The thead's first row unconditionally becomes the header,
                // regardless of whether its cells are `<th>` or `<td>` — the
                // header search never falls through past a `thead`.
                if let Some(row_node) = thead_rows.next() {
                    let (cells, translated) = extract_row_cells(handlers, &row_node);
                    headers = cells;
                    markdown_translated &= translated;
                    header_decided = true;
                }
                // A multi-row thead's remaining rows carry no further header
                // candidacy; they surface as ordinary data rows.
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
    // A header row and its separator always appear once the table has at
    // least one column. With no thead and no qualifying row the header row is
    // empty rather than absent: a table opening straight into data rows reads
    // as a table whose first row is the header.
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

/// The `<tr>` children of a `<thead>`/`<tbody>`/`<tfoot>` node, collected
/// eagerly since the borrow behind `children.borrow()` cannot outlive this
/// call.
fn row_children(node: &Rc<Node>) -> Vec<Rc<Node>> {
    node.children
        .borrow()
        .iter()
        .filter(|child| is_row(child))
        .cloned()
        .collect()
}

fn is_row(node: &Rc<Node>) -> bool {
    matches!(&node.data, NodeData::Element { name, .. } if name.local.as_ref() == "tr")
}

/// Whether every cell in `row_node` is a `<th>`, and there is at least one.
///
/// The built-in promotes any row holding a single `<th>`
/// (htmd-0.5.5/src/element_handler/table.rs:83-93, 106-117). That rule turns a
/// `<tr><th>label</th><td>value</td></tr>` row-heading row into a column
/// header, inventing a column name out of the row's own label — nginx's
/// directive tables read `Syntax:` as a column that way. Requiring every cell
/// to be a `<th>` leaves such a row in the body, and the table surfaces with an
/// empty header row instead of a false one.
///
/// Only element children count: a `<tr>` written across several source lines
/// carries whitespace text nodes between its cells. An empty `<tr>` never
/// qualifies, so `all` cannot promote it on a vacant iterator.
fn row_is_all_header_cells(row_node: &Rc<Node>) -> bool {
    let children = row_node.children.borrow();
    let mut cells = children.iter().filter_map(|cell| match &cell.data {
        NodeData::Element { name, .. } => match name.local.as_ref() {
            tag @ ("th" | "td") => Some(tag),
            _ => None,
        },
        _ => None,
    });
    let mut saw_cell = false;
    let all_th = cells.all(|tag| {
        saw_cell = true;
        tag == "th"
    });
    saw_cell && all_th
}

/// Extracts one body-level row (a `tbody`/`tfoot` row, or a bare `<tr>`
/// directly under `<table>`) and resolves the header search against it: the
/// first such row to reach this function decides `*header_decided`, and if
/// every one of its cells is a `<th>` its extracted cells become `*headers`
/// instead of a data row. Shared by the `"tbody" | "tfoot"` and `"tr"` match arms in
/// `table_handler`, which differ only in how many row nodes they hand this
/// function — a loop over `tbody`/`tfoot`'s rows versus a single top-level
/// `<tr>`.
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

/// Extracts a row's `<th>`/`<td>` cells positionally, in source order,
/// passing each cell's conversion to `Handlers::handle` (dispatches to
/// htmd's built-in `td_th_handler`) rather than filtering by a single tag
/// the way the built-in `extract_row_cells` does.
fn extract_row_cells(handlers: &dyn Handlers, row_node: &Rc<Node>) -> (Vec<String>, bool) {
    let mut cells = Vec::new();
    let mut markdown_translated = true;

    for cell in row_node.children.borrow().iter() {
        let is_cell = matches!(&cell.data, NodeData::Element { name, .. } if matches!(name.local.as_ref(), "th" | "td"));
        if !is_cell {
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

/// Mirrors htmd's built-in `normalize_cell_content`
/// (htmd-0.5.5/src/element_handler/table.rs:250-256): folds `\n` to a space
/// and drops `\r` so multi-line cell content cannot split the pipe-delimited
/// row, escapes `|` so cell content cannot introduce a spurious column, then
/// trims tab/newline/CR/space from both ends. Unlike a general whitespace
/// collapse, this does not touch other whitespace-like characters (e.g. NBSP
/// U+00A0), which must survive unchanged inside the cell.
fn normalize_cell_content(content: &str) -> String {
    let content = content
        .replace('\n', " ")
        .replace('\r', "")
        .replace('|', "&#124;");
    trim_document_whitespace(&content).to_owned()
}

/// Trims the same whitespace set as htmd's private
/// `TrimDocumentWhitespace::trim_document_whitespace`
/// (htmd-0.5.5/src/text_util.rs:14-16, 215-217): tab, newline, CR, and space
/// only, so NBSP and other non-ASCII whitespace-like characters are left in
/// place.
fn trim_document_whitespace(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\r' | ' '))
}

/// Writes one pipe-delimited row with a fixed one space of padding on each
/// side of every cell — no column-width alignment padding. A cell shorter
/// than `num_columns` (row too short) or empty renders as `|  |` (pipe,
/// space, space, pipe), per the contract's spec for an empty cell.
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

/// Writes the dash separator row: exactly 3 dashes per cell, unpadded to
/// column width.
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

/// Wraps `markdown` in a `---`-delimited YAML frontmatter block carrying
/// whichever of title/author/date the article provides. When the article
/// carries none of the three, the wrapper is skipped entirely rather than
/// emitting an empty `---\n---\n\n` shell: that shell holds no information
/// and would otherwise put a bare `---` line ahead of the article's own
/// content, which a caller scanning the body line-by-line (e.g. by leading
/// `-`) cannot distinguish from content that starts with a dash.
fn format_with_frontmatter(article: &ExtractedArticle, markdown: &str) -> String {
    let mut fields = String::new();

    if let Some(title) = &article.title {
        write_yaml_str(&mut fields, "title", title);
    }
    // "byline" is the Readability/journalism term; mapped to "author" for YAML frontmatter
    if let Some(author) = &article.byline {
        write_yaml_str(&mut fields, "author", author);
    }
    if let Some(date) = &article.published_time {
        write_yaml_str(&mut fields, "date", date);
    }

    // The body is untrusted page content appended after the frontmatter, so a
    // column-0 `---`/`...` in it would otherwise open a YAML document boundary.
    // A marker inside a closed fence is quoted sample output, not an attempt to
    // forge a document boundary, so it stays verbatim. Outside any fence, or
    // inside one that never closes, it is still rewritten to `***`.
    let body = neutralize_yaml_markers_outside_fences(markdown);

    let mut fm = String::from("---\n");
    fm.push_str(&fields);
    fm.push_str("---\n\n");
    fm.push_str(&body);
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
    /// This file's own `table_handler` pushes the header row
    /// (`format_table_row`) immediately followed by the separator row
    /// (`format_separator_row`) with no blank line between them
    /// (converter.rs:372-373).
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
    /// `extract_row_cells` above passes each cell's content through this
    /// file's own `normalize_cell_content`, which replaces every `\n` with a
    /// single space before the cell is written into the pipe-delimited row
    /// (converter.rs:427, 440-446).
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
    /// (htmd-0.5.5/src/dom_walker.rs:34-41).
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
    /// `concat_strings!("\n\n", content, "\n\n")`). This crate's own `pre`
    /// handler fences the case instead, using `crate::markdown::fence_delimiter`.
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
    /// ancestor. The sibling text after the span is where the surviving
    /// newline becomes observable.
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

    /// [T-FC053] 行ごとに span を並べた pre で各行が別の行として出る
    ///
    /// The shape a syntax highlighter emits: one `<span>` per source line, each
    /// carrying its own trailing `\n`, which the built-in fast path trims off
    /// every span independently until the lines collapse into one.
    ///
    /// Each span carries a distinct `data-line` attribute so htmd's
    /// adjacent-element merge (`dom_walker::can_combine`, gated on
    /// `attrs1 == attrs2`) does not fold the three siblings into one node ahead
    /// of the per-span trim this test targets.
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

    /// [T-FC054] pre の外の inline code の中の span では改行が剥がれ空白も残らない
    ///
    /// The passthrough branch checks for a `<pre>` ancestor only, so this span
    /// falls to htmd's built-in span handler, whose `content.trim_matches('\n')`
    /// (htmd-0.5.5/src/element_handler/span.rs:33) strips both edges before
    /// `handle_preformatted_code` can fold the newline to a space. The lines
    /// join with no separator at all.
    ///
    /// The newline has to sit inside the span: in the `<code>`'s own text node
    /// it never reaches the span handler and still folds to a space. Removing
    /// the `span` registration leaves the output identical, so this is htmd's
    /// standing behavior, not one this crate's `span` handler introduces.
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

    /// [T-FC060] th と td が混ざる行でラベルと値が同じ行の別セルに出る
    ///
    /// The row shape the whole handler exists for. Under the built-in's
    /// per-tag extraction, described on `table_handler` above, this row keeps
    /// "Name" and drops "Alice"; the positional walk must carry both into the
    /// same output row, in separate cells.
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

    /// [T-FC061] パイプで囲まれた行の中に空白 2 個以上の連続が現れない
    ///
    /// htmd's built-in row formatter pads every cell out to the column's max
    /// width across the whole table (`compute_column_widths` /
    /// `format_row_padded`, htmd-0.5.5/src/element_handler/table.rs:258-299),
    /// so a cell shorter than its column produces a run of two or more spaces
    /// before the next `|`. The fixture's cells differ in width, which is what
    /// makes the absence of such a run discriminating.
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

    /// [T-FC062] 区切り行がセルごとにダッシュ 3 本で出る
    ///
    /// htmd's built-in `format_separator_padded` widens each column's dash run
    /// to that column's computed width (htmd-0.5.5/src/element_handler/
    /// table.rs:272-279), so "Name"/"Alice" produce a 5-dash column rather
    /// than the fixed 3 the contract specifies.
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

    /// [T-FC063] セルの中の NBSP が空白へ落ちずに残る
    ///
    /// The contract requires cell-content newline normalization to follow the
    /// built-in form (`normalize_cell_content` replaces `\n`/`\r` only,
    /// htmd-0.5.5/src/element_handler/table.rs:250-256) rather than a general
    /// whitespace collapse that would fold U+00A0 into an ASCII space. This
    /// pins that a literal NBSP inside a cell reaches the output unchanged.
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

    /// [T-FC055] 隣の span が要素の子を持つ形でも pre の中の改行が残る
    ///
    /// The neighboring span's child is an element rather than a bare text
    /// node, which a passthrough reading raw text would leave unconverted.
    /// `Handlers::walk_children` recurses into it instead.
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

    /// [T-FC064] ヘッダへ昇格した行の td が失われない
    ///
    /// A `<thead>`'s first row becomes the header whatever its cells are, so
    /// the all-`<th>` rule never runs on it. A mixed row there must reach the
    /// header row with both cells, not just the `<th>` one.
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

    /// [T-FC065] 本文の途中にある th だけの行がデータ行として出る
    ///
    /// The header search reaches only `thead`'s first row or the table's first
    /// row. An all-`<th>` row anywhere else stays a data row
    /// even when the table's first row did not qualify as a header, because the
    /// search never scans on for a later row that would.
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

    /// [T-FC066] 複数行 thead の 2 行目以降がデータ行として出る
    ///
    /// Only a `thead`'s first row carries header candidacy.
    /// Its second and later rows must still reach the output, as data rows, so
    /// a multi-row `<thead>` loses nothing.
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

    /// [T-FC068] Faithful モードで属性を持つ表が組み込みへ委譲され HTML のまま出る
    ///
    /// `markdown_converter` is Pure-only, so the test builds its own converter
    /// in `Faithful` mode with the same handlers registered.
    ///
    /// Delegation shows up as raw HTML because the built-in opens with
    /// `serialize_if_faithful!` (htmd-0.5.5/src/element_handler/table.rs:19),
    /// which serializes the element unconverted. Running the positional
    /// extraction instead would emit a pipe table, since that path carries no
    /// mode check of its own. The table needs an attribute to get there:
    /// `serialize_if_faithful!` requires more than 0 of them.
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

    /// [T-FC067] 全セルが th の行が無い表で空のヘッダ行と区切り行が出る
    ///
    /// A table with no qualifying header row still opens with an empty header
    /// row and its separator, not with its data rows. The fixture is a
    /// row-heading table whose first row mixes `<th>` and `<td>`: htmd's
    /// built-in rule promotes any row holding a single `<th>`, which would read
    /// `Name` as a column name and `Alice` as the value under it. Requiring
    /// every cell to be a `<th>` keeps that row in the body, so the table opens
    /// with an empty header row instead of a fabricated one.
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

    /// [T-FC069] thead を持たない表で全セルが th の最初の行がヘッダ行になる
    ///
    /// The affirmative half of the all-`<th>` rule. T-FC067 pins the rejection
    /// side (a mixed row stays in the body) and T-FC064 promotes through
    /// `<thead>`, which never consults the rule at all. A table whose first
    /// `<tbody>` row is entirely `<th>` reaches the rule and must promote.
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

    /// [T-FC070] caption を持つ表で caption がヘッダ行の前に出る
    ///
    /// The built-in's caption placement is kept, which emits the
    /// caption's own converted content ahead of the header row rather than
    /// dropping it (htmd-0.5.5/src/element_handler/table.rs:36-44).
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

    /// [T-FC071] 行を持たない table が表として組み立てられずに退避する
    ///
    /// With no rows there is no column count to build a pipe table from, so the
    /// handler walks the children and returns their content instead of emitting
    /// a header row and separator for zero columns.
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

    /// [T-FC072] セルの間に改行がある tr でもセルが取り出される
    ///
    /// A `<tr>` written across source lines carries whitespace text nodes
    /// between its cells. Both the cell walk and the all-`<th>` rule count
    /// element children only, so the text nodes must not hide the cells or
    /// block header promotion. The row also sits directly under `<table>` with
    /// no `<tbody>`, which the browser parser preserves for a `<tr>` written
    /// this way.
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

    /// [T-FC034] pre 直下の 2 番目のテキストが先頭にバッククォートを持つとき原文のまま出る
    ///
    /// `opens_with_escaped_fence_char` only inspects the *first* Text child of
    /// `<pre>` (converter.rs, `find_map` over `NodeData::Text`), and
    /// `pre_handler` only strips a backslash sitting at the very front of the
    /// joined `content` string. A second direct-child text node starting with
    /// `` ` `` gets htmd's escape too (its parent is still `<pre>`), but that
    /// escape lands mid-string, past the first (non-backtick-leading) text
    /// node's own output, so today's front-anchored strip never reaches it.
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

    /// [T-FC035] 先頭のテキストより前に要素がある pre でもバッククォートが原文のまま出る
    ///
    /// `opens_with_escaped_fence_char` finds the first *Text* child regardless
    /// of a preceding element sibling, so it still reports a leading escape
    /// here. But the escaped backslash sits past that element's own converted
    /// output in the joined `content` string, not at its front, so
    /// `content.strip_prefix('\\')` fails silently and the backslash stays put.
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

    /// [T-FC036] 入れ子の pre でも内側のバッククォートが原文のまま出る
    ///
    /// Same root cause as T-FC034 (a `<pre>` direct-child text node's leading
    /// backtick is not the pre's first Text child, so the front-anchored strip
    /// misses it), reached one level deeper: the outer `<pre>` delegates its
    /// `<pre>` child to `Handlers::handle`, which re-enters this crate's own
    /// `pre_handler` for the inner element. The bug must not disappear once
    /// recursion is involved.
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

    /// [T-FC037] 同じタグと属性の span が連なる pre で各行の改行が保たれる
    ///
    /// Unlike T-FC052/053 (spans with distinct attrs, or attrs that block
    /// htmd's adjacent-element merge), three `<span>` siblings sharing the same
    /// tag and no attrs are eligible for htmd's own sibling merge
    /// (`dom_walker::can_combine`, gated on `attrs1 == attrs2`), which runs
    /// inside `Handlers::walk_children` and folds them into a single merged
    /// span node ahead of this crate's own per-element handling.
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

    /// [T-FC038] 隣接するブロック子の境界に残る改行が2個までに収まる
    ///
    /// `raw_pre_content` appends each Element child's own converted content
    /// directly (`content.push_str(&res.content)`) with no separator logic
    /// of its own. A block-level child's own content already opens and
    /// closes with a blank line, so two such children in a row stack both
    /// sides' blank lines instead of collapsing to one. Scout's own rule for
    /// the rebuilt `<pre>` content is that the run of newlines at such a
    /// boundary caps at 2 (a single blank line), regardless of how many the
    /// two sides' individual wrapping happens to add up to.
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

    /// [T-FC039] pre 直下の br が行末空白2個と改行として残る
    ///
    /// A `<br>` that is a direct child of `<pre>` reaches `raw_pre_content`'s
    /// `NodeData::Element` branch, which hands it to `Handlers::handle` and
    /// appends the result into the rebuilt content exactly as returned, with
    /// no trimming of a line break that lands mid-string. Under the
    /// default `BrStyle::TwoSpaces` a `<br>` converts to two trailing
    /// spaces and a newline — the Markdown hard-break form — and that form
    /// must reach the fenced output between the text on either side of it.
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

    /// [T-FC040] pre の中の未登録タグの子のテキストがエスケープされずに出る
    ///
    /// The fixture's inner tag has no handler registered for it by this
    /// crate or by htmd itself, so the `NodeData::Element` branch in
    /// `raw_pre_content` still hands it to `Handlers::handle`, but that call
    /// resolves to htmd's own no-handler fallback path instead of any
    /// handler this crate adds. That fallback still counts the surrounding
    /// `<pre>` as an ancestor, so escape-target characters in the tag's text
    /// must survive unescaped, the same as text sitting directly under
    /// `<pre>` does.
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
}
