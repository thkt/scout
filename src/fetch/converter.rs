use std::borrow::Cow;
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
/// (htmd's options.rs `Options`); `preformatted_code: true` is the one
/// field the contract adds on top, so it keeps whitespace inside inline
/// `<code>` instead of collapsing it
/// (htmd's element_handler/code.rs `handle_inline_code`).
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

/// Fences a `<pre>` with no `<code>` child, which htmd's built-in `pre_handler`
/// otherwise emits unfenced (htmd's element_handler/pre.rs `pre_handler`,
/// `concat_strings!("\n\n", content, "\n\n")` with no fence markers).
///
/// A `<pre><code>` pair arrives already fenced by htmd's built-in
/// `code_handler` (htmd's element_handler/code.rs `code_handler`, registered
/// ahead of this handler in `ElementHandlers::new` so `add_handler` shadows
/// only the outer `pre` dispatch, not the inner `code` one), so it passes
/// through unchanged. Telling the two shapes apart reads the DOM, not the
/// walked text: a bare `<pre>` holding syntax-highlighter `<span>`s emits its
/// own leading backtick raw and reads as already fenced.
// `Element` must stay by-value: htmd's blanket `ElementHandler` impl only
// covers `Fn(&dyn Handlers, Element) -> Option<HandlerResult>`
// (htmd's element_handler/mod.rs `handle`), so a `&Element` signature
// would not satisfy `add_handler`'s `Handler: ElementHandler` bound.
#[allow(clippy::needless_pass_by_value)]
fn pre_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    // Called for its side effect as much as its value: the walk runs htmd's
    // adjacent-sibling merge on `element.node.children` before either branch
    // below reads them. The walked string is used only by the `<pre><code>`
    // branch, which htmd already fences correctly on its own.
    let result = handlers.walk_children(element.node);

    // Ahead of the `<code>`-child split: both shapes fence, and a fence inside
    // a cell leaves its own backticks as cell text. Reading the whole `<pre>`
    // rather than a `<code>` child keeps sibling text a `<code>` does not cover.
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

/// Drops a target element's content instead of walking its children.
///
/// Left unhandled, every one of these tags still reaches the body: htmd's own
/// `block_handler` walks the children of the ones it covers, and `Pure` mode's
/// unregistered-tag fallback walks the rest. `add_handler` shadows both paths,
/// the same way `pre_handler` shadows the built-in `pre` handler.
///
/// The removal stays in this conversion layer and never touches the freshly
/// downloaded `html`. `is_js_dependent` (`src/fetch.rs`) scans that raw byte
/// string for `b"<script"` to detect an SPA shell before the `need_js` branch.
///
/// htmd looks tags up by local name, not namespace, so the two tags SVG shares
/// with HTML resolve separately here. `desc` is suppressed in the SVG namespace
/// only: an element literally named `<desc>` elsewhere renders as visible text,
/// and dropping it would delete body text the reader sees. `title` is
/// suppressed in every namespace, since no `<title>` renders as body text. The
/// page title still reaches the frontmatter, which `make_raw` reads through
/// `extract_title_from_html` without passing this converter.
///
/// A non-SVG `<desc>` hands back to [`Handlers::fallback`], which finds no
/// further handler for the tag and lands on `Pure` mode's walk-children
/// default (htmd's element_handler/mod.rs `handle`).
// `Element` must stay by-value for the same `add_handler` signature reason as
// `pre_handler` above.
#[allow(clippy::needless_pass_by_value)]
fn suppressed_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if !is_suppressed_element(element.node) {
        return handlers.fallback(element);
    }
    Some(HandlerResult {
        content: String::new(),
        markdown_translated: true,
    })
}

/// The tags [`suppressed_handler`] is registered for.
const SUPPRESSED_TAGS: [&str; 7] = [
    "script", "style", "noscript", "textarea", "iframe", "desc", "title",
];

/// Whether [`suppressed_handler`] drops this element's content.
///
/// Every reader of the DOM outside the handler dispatch has to agree with it.
/// `push_text_content` walks a `<pre>`'s subtree directly and would otherwise
/// resurrect the bodies the handler removes.
fn is_suppressed_element(node: &Rc<Node>) -> bool {
    let Some(tag) = element_tag(node) else {
        return false;
    };
    SUPPRESSED_TAGS.contains(&tag)
        && (tag != "desc" || element_namespace(node) == Some(SVG_NAMESPACE))
}

/// The namespace html5ever stamps on an SVG element. `<desc>` is an HTML
/// integration point, so its children parse as HTML, but the element itself
/// still carries this namespace (measured; pinned by T-FC091).
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// The element's namespace URI, or `None` when the node is not an element.
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

/// Rewrites a self-closed raw-text start tag (`<script src="app.js" />`) into
/// an explicit open/close pair (`<script src="app.js"></script>`), so it
/// cannot swallow the rest of the document.
///
/// The HTML tokenizer ignores the self-closing flag on a raw-text start tag
/// and switches to raw-text state regardless, so everything up to the matching
/// end tag — in an XHTML page written with `<script … />`, that is the whole
/// remaining body — becomes one Text child of that element. `check_content_type`
/// (src/fetch/download.rs) accepts `application/xhtml+xml`, and htmd parses
/// what it accepts as HTML, so such a page reaches this converter mis-parsed.
/// `suppressed_handler` then drops the swallowed body along with the element.
///
/// This rewrite changes parse structure only. A rewritten element still has
/// empty content and is still suppressed; the swallowed markup becomes the
/// sibling elements the author wrote. It does not make scout an XHTML parser:
/// the rest of XML's syntax stays unhandled.
///
/// The scan reads the byte string, not a parse tree, so a `<script … />`
/// written inside an HTML comment or inside a quoted attribute value is
/// rewritten there too. Neither position reaches the body, so the rewritten
/// text stays inert.
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
        // An unterminated start tag has no `>` to rewrite, and nothing after
        // it can be a start tag either, so the scan is done.
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
        // The tag opened the ordinary way, so the tokenizer is now in raw-text
        // state and the scan must jump over the content to stay in step with
        // it. Rewriting a `<script … />` that a JS string happens to contain
        // would insert a real `</script>` into script data and end the element
        // early, spilling the rest of the source into the body — the leak
        // `suppressed_handler` exists to prevent.
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

/// The [`RAW_TEXT_TAGS`] entry naming the start tag that begins at `from`, or
/// `None` when no entry matches. Tag names are ASCII case-insensitive, and the
/// name must end on a character the tokenizer treats as a name boundary, so
/// `<scriptlet>` does not match `script`.
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

/// The index of the `<` beginning `tag`'s own end tag at or after `from`, or
/// `None` when the element never closes. In raw-text state the tokenizer ends
/// the content on `</` plus this tag's name plus a name boundary and on
/// nothing else, so a start tag written inside the content stays text.
fn end_tag_at_or_after(bytes: &[u8], from: usize, tag: &str) -> Option<usize> {
    (from..bytes.len().saturating_sub(1)).find(|&index| {
        bytes[index] == b'<'
            && bytes[index + 1] == b'/'
            && raw_text_tag_at(bytes, index + 2) == Some(tag)
    })
}

/// The index of the `>` closing the start tag whose attribute list begins at
/// `from`, or `None` when the tag never closes. A `>` inside a quoted
/// attribute value does not close the tag, so quoting is tracked.
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

/// The element's tag name, or `None` when the node is not an element.
fn element_tag(node: &Rc<Node>) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

/// Whether the element has a direct `<code>` child, the shape htmd's
/// `code_handler` fences on its own: it fences exactly when the `<code>`
/// element's parent is `<pre>` (htmd's element_handler/code.rs `code_handler`).
fn has_code_child(node: &Rc<Node>) -> bool {
    node.children
        .borrow()
        .iter()
        .any(|child| element_tag(child) == Some("code"))
}

/// Rebuilds a `<pre>` element's non-code content from its DOM children rather
/// than htmd's walked text.
///
/// `escape_pre_text_if_needed` backslash-escapes a leading `` ` `` or `~` only
/// while htmd walks the text (htmd's dom_walker.rs `walk_node` / `escape_pre_text_if_needed`).
/// Reading a Text child's `contents` off the DOM never introduces that
/// backslash, at any child position, so nothing has to be reverse-escaped
/// afterwards.
///
/// Requires `pre_handler`'s discarded `walk_children` call to have run first:
/// that is what merges adjacent same-tag same-attrs `<span>`s
/// (htmd's dom_walker.rs `can_combine`) into the
/// single node this loop then sees.
///
/// `markdown_translated` aggregates from Element children alone. A Text child
/// cannot turn it false: htmd's own `NodeData::Text` arm never touches the
/// flag.
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

/// Appends `addition` to `content`, capping the newline run straddling the
/// junction at 2 so the boundary reads as at most one blank line.
///
/// Two block-level children each wrap themselves in blank lines, so back to
/// back they stack both sides'. Only Element children come through here: a
/// Text child's embedded newlines are real line breaks the preformatted text
/// depends on, and capping them would corrupt the block.
///
/// Trimming by character count is a valid byte cut because both counts are of
/// `\n`, a 1-byte character.
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

/// Passes a `<span>`'s content through unmodified when the span has a `<pre>`
/// ancestor; every other span delegates to `Handlers::fallback`.
///
/// htmd's own `span` fast path (htmd's dom_walker.rs `walk_node`) trims
/// every leading and trailing `\n` off the span's walked content, including
/// inside a `<pre>` where those newlines are line breaks the preformatted text
/// depends on. That path is gated on exactly one handler being registered for
/// `span`, so registering this second one takes htmd back to its normal
/// per-element dispatch, where the most recently registered handler runs first.
///
/// `has_pre_ancestor` below is narrower than htmd's own `is_inside_pre`
/// (htmd's element_handler/mod.rs `is_inside_pre`), which counts a `<code>`
/// ancestor as inside pre too. DR-0025 records why the narrower check stays;
/// T-FC054 pins what a `<span>` in inline `<code>` gets as a result.
// `Element` must stay by-value for the same `add_handler` signature reason as
// `pre_handler` above.
#[allow(clippy::needless_pass_by_value)]
fn span_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    if has_pre_ancestor(element.node) {
        return Some(handlers.walk_children(element.node));
    }
    handlers.fallback(element)
}

/// Whether any ancestor of `node` is a `<pre>` element.
fn has_pre_ancestor(node: &Rc<Node>) -> bool {
    has_ancestor_matching(node, |tag| tag == "pre")
}

/// Whether any ancestor of `node` is a `<td>` or `<th>` element.
fn has_table_cell_ancestor(node: &Rc<Node>) -> bool {
    has_ancestor_matching(node, |tag| matches!(tag, "td" | "th"))
}

/// Whether any ancestor of `node` is an element whose tag satisfies `predicate`.
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

/// The parent link is a `Cell<Option<WeakHandle>>`, so reading it means taking
/// the value out. Putting it back leaves the link intact for later traversals,
/// the same take-upgrade-put-back htmd's own `node_util::get_parent_node` does
/// (htmd's node_util.rs `get_parent_node`).
fn get_parent(node: &Rc<Node>) -> Option<Rc<Node>> {
    let value = node.parent.take();
    let parent = value.as_ref().and_then(Weak::upgrade);
    node.parent.set(value);
    parent
}

/// The text a reader sees in `node`'s subtree, depth-first.
///
/// A table cell's `<pre>` reads its content through this rather than the walked
/// markdown text, which carries htmd's fence-char backslash-escaping (see
/// `raw_pre_content` above) that the inline-code-span delimiter math must not
/// count.
///
/// Reading the DOM bypasses the handler dispatch, so the two rules deciding
/// what a reader sees are applied here instead. A suppressed element
/// ([`is_suppressed_element`]) contributes nothing. A `<br>` contributes a bare
/// `\n`, since it holds no Text child and concatenating text alone would run
/// the lines it separates into one word. Folding that newline to a space is
/// `normalize_cell_content`'s job, not this function's.
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

/// Builds a CommonMark 0.31.2 §6.3 inline code span for a table cell's code
/// content.
///
/// The delimiter length is the content's longest backtick run + 1, computed
/// the same way `fence_delimiter` computes a block fence's run length, but
/// with no 3-backtick floor: an inline span may open with a single backtick.
/// When the content starts or ends with a backtick, one inner space next to
/// each delimiter keeps that backtick from reading as part of the delimiter.
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

/// Suppresses an `<a>` whose `href` is a same-page fragment (`#…`) and whose
/// content is empty: a syntax highlighter's per-line `#__codelineno-…`
/// anchor, or a Sphinx-style `<a class="headerlink" href="#…"></a>`. Neither
/// carries a destination a reader could follow, so htmd's own
/// `AnchorElementHandler` would still emit a bare `[](#…)`
/// (htmd's element_handler/anchor.rs `handle`, which only special-cases
/// a missing `href`, not an empty one).
///
/// Shadows `a` in the same shape as `pre_handler` above: `walk_children` runs
/// once, up front, and its content decides the branch. `markdown_translated`
/// is set explicitly on the suppress branch, since discarding the anchor
/// leaves no content that could still be raw HTML.
///
/// Every other case falls to [`Handlers::fallback`]. Destination escaping,
/// link styles and referenced-link bookkeeping live in the built-in handler,
/// and this one does not re-derive them.
///
/// The built-in also carries the `<a>`'s `title` attribute into its output as
/// `](url "title")` (htmd's element_handler/anchor.rs `build_inlined_anchor`).
/// `strip_link_title` drops that suffix only where `content` is non-empty:
/// T-FC049 pins that an absolute-URL anchor with empty content keeps its
/// title, and only a *fragment* href with empty content is suppressed.
// `Element` must stay by-value for the same `add_handler` signature reason as
// `pre_handler` above.
#[allow(clippy::needless_pass_by_value)]
fn a_handler(handlers: &dyn Handlers, element: htmd::Element) -> Option<HandlerResult> {
    let result = handlers.walk_children(element.node);
    let has_link_text = !result.content.trim().is_empty();
    // `href=""` resolves to the current page, the same nothing a bare `#`
    // points at, so both suppress on empty content.
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

/// The `<a>` element's `href` attribute value, or `None` when absent.
fn anchor_href(element: &htmd::Element) -> Option<String> {
    anchor_attr(element, "href")
}

/// The value of the `<a>` element's attribute named `name`, or `None` when
/// the element carries no such attribute.
fn anchor_attr(element: &htmd::Element, name: &str) -> Option<String> {
    element
        .attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .map(|attr| attr.value.to_string())
}

/// Drops the ` "title")` suffix htmd's built-in `AnchorElementHandler`
/// writes onto a delegated link's tail
/// (htmd's element_handler/anchor.rs `build_inlined_anchor`, the `Inlined` /
/// `InlinedPreferAutolinks` styles `markdown_converter` always builds with).
/// The match is by tail position, never by searching for the title text
/// anywhere in `content`: a title that also reads as ordinary link text must
/// not be cut out of the middle. A tail that does not match returns verbatim,
/// so an unrecognized shape is never rewritten blind.
///
/// `title_attr` is the raw `title` attribute text. htmd escapes and
/// reflows it first (htmd's element_handler/anchor.rs `process_title`) before writing it
/// into the delegated result, so `process_title_like_htmd` below must
/// reproduce that same transform for the tail match to line up, including a
/// whitespace-only attribute: htmd renders that as an empty-but-present `("")`
/// title rather than omitting the title syntax.
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

/// Reimplements htmd's private `process_title`
/// (htmd's element_handler/anchor.rs `process_title`). `strip_link_title`
/// above has to reproduce htmd's transform byte for byte to locate the title
/// htmd already wrote, so this makes no escaping decision of its own.
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

/// Splits `content` at the start of its trailing run of document whitespace
/// (tab, newline, CR, space — the same set `trim_document_whitespace` below
/// trims from both ends). `AnchorElementHandler::build_inlined_anchor`
/// strips the anchor's own trailing whitespace off the link text before
/// building `[text](url "title")` and re-appends it after the closing `)`
/// (htmd's element_handler/anchor.rs `build_inlined_anchor`), so a tail match against
/// the raw `content` misses whenever that whitespace is present. The leading
/// side is left alone: `content` always opens with `[`.
fn split_trailing_document_whitespace(content: &str) -> (&str, &str) {
    let body = content.trim_end_matches(['\t', '\n', '\r', ' ']);
    content.split_at(body.len())
}

/// Fixes htmd's built-in `table_handler`'s per-tag row extraction
/// (`extract_row_cells(handlers, row_node, "th")` /
/// `extract_row_cells(handlers, row_node, "td")`,
/// htmd's element_handler/table.rs `extract_row_cells`): a row is scanned once
/// per cell tag, so a `<tr>` mixing `<th>` and `<td>` — a label/value row with
/// no `<thead>` — loses whichever tag that row's extraction call did not ask
/// for (the branching that drops it lives in `table_handler`). This handler reads
/// each row's cells positionally in one pass, so a label and its value from
/// the same source row land in the same output row, in separate cells.
///
/// Row and separator formatting drop the built-in's column-width alignment
/// padding, which widens every cell and dash run out to the column's longest
/// cell.
///
/// Cell-content newline normalization, caption handling, and column-count
/// estimation follow the built-in's shape.
///
/// Any non-`Pure` translation mode falls straight to `Handlers::fallback` and
/// the built-in's own `serialize_if_faithful!` gate.
/// `markdown_converter` always builds `Pure`, so scout's runtime never takes
/// that branch; T-FC068 exercises it through a `Faithful`-mode converter built
/// in the test.
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
    element_tag(node) == Some("tr")
}

/// Whether every cell in `row_node` is a `<th>`, and there is at least one.
///
/// The built-in promotes any row holding a single `<th>`
/// (htmd's element_handler/table.rs `table_handler`). That rule turns a
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

/// Folds newlines to a space, escapes `|`, and trims tab, newline, CR and
/// space from both ends, so cell content can neither split the row nor add a
/// column. Other whitespace-like characters survive unchanged, NBSP U+00A0
/// among them; a general whitespace collapse would eat those too.
///
/// The pipe escape is `\|`, where htmd writes `&#124;`
/// (htmd's element_handler/table.rs `normalize_cell_content`). GFM unescapes `\|` while
/// splitting the row into cells, ahead of inline parsing, so it resolves
/// wherever it lands; an entity reference stays six literal characters inside a
/// code span. htmd needs the entity because its `format_row_padded` counts
/// chars for column alignment, and `format_table_row` here writes none.
/// `escape_md_inline` (`src/markdown.rs`) already writes `\|`, and
/// `src/github/format.rs` feeds its output into table cells.
fn normalize_cell_content(content: &str) -> String {
    let content = content
        .replace('\n', " ")
        .replace('\r', "")
        .replace('|', "\\|");
    trim_document_whitespace(&content).to_owned()
}

/// Trims the same whitespace set as htmd's private
/// `TrimDocumentWhitespace::trim_document_whitespace`
/// (htmd's text_util.rs `trim_document_whitespace` / `is_document_whitespace`):
/// tab, newline, CR, and space
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
    let content_html = close_self_closed_raw_text_tags(&article.content_html);
    let markdown = markdown_converter()
        .convert(&content_html)
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
    use crate::fetch::extractor::extract_article;
    use crate::search::engine::MAX_PAGE_BYTES;
    use crate::yaml::truncate_and_reneutralize;

    /// Minimal `ExtractedArticle` fixture for tests that only vary the body
    /// HTML: no title/byline/published_time and no raw-fallback flag.
    ///
    /// `html` lands in `content_html` verbatim: the helper never calls
    /// `extractor::extract_article`, so dom_smoothie's Readability pass never
    /// touches it. Every test calling `to_fetch_result` directly exercises
    /// htmd handling on hand-authored HTML, not the production pipeline.
    ///
    /// The gap is load-bearing for `class`. `extract_article` runs dom_smoothie
    /// with `keep_classes: false`, which strips every element's `class` before
    /// conversion sees the DOM
    /// (dom_smoothie's `Readability::post_process_content` / `clean_classes`). A test here asserting
    /// on class-driven behavior pins it for HTML that never passed that strip.
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
    /// (`format_separator_row`) with no blank line between them.
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
    /// (htmd's element_handler/li.rs `indent_text_except_first_line`).
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
    /// single space before the cell is written into the pipe-delimited row.
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
    /// destination (htmd's element_handler/anchor.rs `escape_link_destination`), so the
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
    /// ordinary text — `\ * _ \` [ ]` (htmd's dom_walker.rs `escape_if_needed`) —
    /// but a `<pre><code>` text node takes the `is_pre && parent_tag != "pre"`
    /// branch, which copies the text through with no escaping at all
    /// (htmd's dom_walker.rs `walk_node`).
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
    /// (htmd's element_handler/code.rs `get_code_fence_marker`), so a code block
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
    /// (htmd's element_handler/code.rs `handle_code_block` / `find_language_from_attrs`).
    ///
    /// This holds for the `--raw` path, which skips Readability, and for direct
    /// `to_fetch_result` callers. The default fetch path strips the `class`
    /// first, as `article`'s doc above records, so a fetched page never reaches
    /// conversion with one.
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
    /// (htmd's element_handler/pre.rs `pre_handler`,
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

    /// [T-FC083] code 子を持たない pre の中身が 3 連バッククォートを含むとき
    /// フェンスが 4 連で出る
    ///
    /// `fence_delimiter` widens the fence past the longest backtick run in the
    /// content. Pins the wiring, not the width rule: `markdown.rs` unit-tests
    /// the rule itself, and a fence as wide as its content would close the
    /// block partway through.
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

    /// [T-FC082] 表の caption がヘッダ行の直前に空行なしで出る
    ///
    /// T-FC070 pins the order, this one the adjacency: no blank line separates
    /// the two. Both pulldown-cmark 0.13.4 and comrak 0.54.0 render this input
    /// and the blank-line variant to identical HTML — `Cap` as its own
    /// paragraph, the rows as a table — because GFM's table extension
    /// interrupts a paragraph. GitHub's own renderer, markdown-it and marked
    /// were not measured.
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

    /// [T-FC020] htmd が既にフェンスした pre の中の code を二重のフェンスで囲まない
    ///
    /// A `<pre><code>` pair is already turned into a single fenced block by
    /// htmd's built-in `code_handler`
    /// (htmd's element_handler/code.rs `code_handler`). The added `pre` handler
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
    /// opening a fence (htmd's dom_walker.rs `escape_pre_text_if_needed`). Once the added
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
    /// `escape_pre_text_if_needed` only ever escapes a text node's very first
    /// character (htmd's dom_walker.rs `escape_pre_text_if_needed`), so a `` \` `` sequence
    /// occurring later in the same text is source content htmd never touches
    /// either. `raw_pre_content` copies a Text child's `contents` straight off
    /// the DOM with no position-based logic of its own, so a mid-content
    /// `` \` `` survives the same way T-FC021's leading one does: both reach
    /// the output because nothing on this crate's side inspects position at
    /// all.
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
    /// `escape_pre_text_if_needed` prepends its own backslash only when the
    /// text node's first character is `` ` `` or `~`
    /// (htmd's dom_walker.rs `escape_pre_text_if_needed`), so a text node whose source
    /// already opens with `` \` `` reaches htmd's walk untouched: the walked
    /// content is the same `` \` `` either way. `raw_pre_content` never reads
    /// that walked content for a Text child, so it does not need to tell the
    /// two cases apart; copying `contents` straight off the DOM reproduces the
    /// source `` \` `` regardless of which case produced it.
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
    /// A `<!-- comment -->` direct child matches neither `NodeData::Text` nor
    /// `NodeData::Element` in `raw_pre_content`'s loop, so it falls through the
    /// catch-all arm and adds nothing to the rebuilt content. The loop keeps
    /// walking past it and reaches the following Text child on that child's
    /// own turn, copying its `contents` straight off the DOM the same as when
    /// no comment precedes it: reading every child in order, rather than only
    /// the first, is what makes a preceding comment harmless here.
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
    /// (htmd's dom_walker.rs `walk_node`), so text nested in a `<span>`
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
    /// htmd's built-in `span` fast path (`walk_node`, active while
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
    /// (htmd's element_handler/span.rs `span_handler`) strips both edges before
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
    /// `format_row_padded`, htmd's element_handler/table.rs `format_row_padded`),
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
    /// to that column's computed width (htmd's element_handler/table.rs
    /// `format_separator_padded`), so "Name"/"Alice" produce a 5-dash column
    /// rather than the fixed 3 the contract specifies.
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
    /// htmd's element_handler/table.rs `normalize_cell_content`) rather than a general
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
    /// `serialize_if_faithful!` (htmd's element_handler/table.rs `table_handler`),
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
    /// dropping it (htmd's element_handler/table.rs `table_handler`).
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
    /// `raw_pre_content` reads each Text child's `contents` off the DOM, so no
    /// escape is introduced and none has to be removed. Reverse-escaping the
    /// walked text instead — stripping a leading backslash off the joined
    /// `content` — reaches only the front of the string, and this backtick sits
    /// past the first text node's output.
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
    /// Same DOM read as T-FC034, with an element sibling ahead of the text.
    /// A front-anchored strip over the joined `content` would miss the escape
    /// here too: it sits past that element's own converted output.
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
    /// T-FC034's shape one level deeper: the outer `<pre>` delegates its
    /// `<pre>` child to `Handlers::handle`, which re-enters this crate's own
    /// `pre_handler`. The DOM read has to hold through that recursion, not just
    /// at the top level.
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
    /// A block-level child's converted content already opens and closes with a
    /// blank line, so two such children in a row would stack both sides' blank
    /// lines. `raw_pre_content` routes each Element child through
    /// `push_element_content`, which caps the newline run at the junction at 2
    /// (a single blank line) regardless of what the two sides add up to.
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

    /// [T-FC041] 段落の中の改行が空白 1 個へ畳まれる
    ///
    /// A raw `\n` makes a Text node fail htmd's `is_plain_text` check
    /// (htmd's dom_walker.rs `is_plain_text`), which routes it through
    /// `compress_whitespace`. That folds any run of ASCII whitespace, a lone
    /// newline included, to a single space.
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

    /// [T-FC042] 段落の中の br が行末空白2個と改行として残る
    ///
    /// htmd's built-in `br_handler` converts a `<br>` to `"  \n"` under the
    /// default `BrStyle::TwoSpaces` (htmd's element_handler/br.rs `br_handler`),
    /// the Markdown hard-break form. Scout registers no `br` handler, so the
    /// built-in is what every `<br>` reaches.
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

    /// [T-FC043] pre の中身は畳まれずに改行が残る
    ///
    /// A `<pre>` with no `<code>` child is rebuilt by this crate's own
    /// `pre_handler` via `raw_pre_content`, which reads a Text child's
    /// `contents` straight off the DOM rather than htmd's walked text, so the
    /// source newline never reaches
    /// `compress_whitespace` at all and survives as a real line break inside
    /// the fence.
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

    /// [T-FC044] inline code の中の連続する空白がそのまま残る
    ///
    /// A `<code>`'s children walk with `is_pre = true` from the tag name
    /// alone, so the whitespace compression that folds a paragraph never runs
    /// on them (htmd's element_handler/mod.rs `walk_children`). The fold that
    /// does run downstream touches `\n` only, leaving a run of spaces alone.
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

    /// [T-FC045] 表セルの中の br は改行を失い空白へ畳まれる
    ///
    /// The `<br>` produces the same `"  \n"` hard break a paragraph gets, but
    /// this crate's own `normalize_cell_content` then replaces every `\n` with
    /// a space so a cell cannot split its pipe-delimited row. The break becomes
    /// a third space beside the two it already carries, leaving no line break
    /// in the rendered table.
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

    /// [T-FC046] リスト項目の中の br は行末空白 2 個を失いインデントされた改行になる
    ///
    /// The `<br>` produces the same `"  \n"` hard break a paragraph gets, but
    /// `list_item_handler` indents every line after the first with
    /// `trim_line_end: true` (htmd's element_handler/li.rs `list_item_handler`). The
    /// trim takes the hard break's two trailing spaces, and the indent stands
    /// in their place.
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

    /// [T-FC047] 見出しの中の br 以降は見出しの外へ出る
    ///
    /// The `<br>` produces the same `"  \n"` hard break a paragraph gets, and
    /// the heading handler writes its `#` marker once, ahead of the whole
    /// content. An ATX heading is a single source line, so everything past the
    /// embedded `\n` lands unmarked on the line below.
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

    /// [T-FC048] pre の中の空アンカーが消えて原文の行とインデントが残る
    ///
    /// Fixture mirrors a syntax-highlighted code block's per-line
    /// `#__codelineno-…` anchor: a bare `<pre>` (no `<code>` child) with an
    /// empty `<a>` at the start of each indented line.
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

    /// [T-FC049] 絶対 URL を指す中身が空のアンカーは行き先と title を保つ
    ///
    /// Paired with a fragment-only empty anchor (`#top`) in the same
    /// paragraph so the assertion cannot pass by coincidence: it fails today
    /// because the fragment-only sibling is not yet suppressed, and only
    /// holds once suppression targets the fragment-only case specifically,
    /// leaving the absolute-URL anchor untouched.
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

    /// [T-FC050] title 付きで中身が空の headerlink が消える
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

    /// [T-FC051] href 属性を持たない a は組み込みへ委譲される
    ///
    /// Paired with a fragment-only empty anchor (`#nav`) in the same
    /// paragraph so the assertion cannot pass by coincidence: it fails today
    /// because the fragment-only sibling is not yet suppressed, and only
    /// holds once suppression is scoped to anchors carrying `href`, leaving
    /// an `<a>` with no `href` at all to fall through to htmd's own handler
    /// unchanged.
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

    /// [T-FC077] href が空文字列で中身も空のアンカーが消える
    ///
    /// `href=""` resolves to the current page, so it points at the same
    /// nothing a bare `#` does. Left to the builtin handler it emits
    /// `[]( "title")`, which is the empty link plus restated title this
    /// handler exists to remove.
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

    /// [T-FC073] リンクテキストを持つリンクの出力から title が消える
    ///
    /// htmd's built-in anchor handler writes the `title` attribute straight
    /// into the link as `](url "title")`
    /// (htmd's element_handler/anchor.rs `build_inlined_anchor`). A link whose `<a>`
    /// carries non-empty content is exactly the case this unit targets, so
    /// the title must not survive delegation.
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

    /// [T-FC074] 二重引用符を含む title でも書き換えが外れない
    ///
    /// htmd's built-in backslash-escapes every `"` inside the title
    /// (htmd's element_handler/anchor.rs `process_title`,
    /// `process_title`), so the delegated tail reads `\"hi\"")` rather than
    /// the raw attribute text. The position match against that tail must
    /// still land, so the rewrite is not knocked off by the extra
    /// backslashes.
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

    /// [T-FC075] 改行を含む title でも書き換えが外れない
    ///
    /// htmd's built-in trims and rejoins each line of the title with `\n`
    /// (htmd's element_handler/anchor.rs `process_title`, `process_title`),
    /// so the delegated tail carries the title's own line break. The
    /// position match must still land against that multi-line tail.
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

    /// [T-FC076] 空白だけの title は title 無しとして扱われる
    ///
    /// htmd's built-in `process_title` drops every whitespace-only line
    /// (htmd's element_handler/anchor.rs `process_title`), so a
    /// whitespace-only `title` attribute still reaches the built-in's
    /// `Some(title)` branch with an empty string and renders as `("")`
    /// rather than omitting the title syntax outright. This unit must treat
    /// that empty result the same as no title at all.
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

    /// [T-FC084] `content_html` に残った `<script>` と `<style>` の中身が本文へ出ない
    ///
    /// htmd's own `block_handler` registers `script` and `style` among its
    /// "other block elements"
    /// (htmd's element_handler/mod.rs `new`), but in `Pure`
    /// translation mode `block_handler` walks the element's children and keeps
    /// their content, wrapped in blank lines
    /// (htmd's element_handler/mod.rs `block_handler`). A `<script>`/`<style>`
    /// element's sole child is the raw JS/CSS source as a single Text node, so
    /// without `suppressed_handler` shadowing that path the source text would
    /// reach the markdown body.
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

    /// [T-FC085] `noscript`, `textarea`, `iframe` の子, `svg` の `desc`, `title` の中身が本文へ出ない
    ///
    /// None of these five tags is `script`/`style`, so T-FC084's block_handler
    /// path does not even apply uniformly: `textarea` and `iframe` do sit in
    /// htmd's own block-element list and share `block_handler`'s walk-and-keep
    /// behavior, but `noscript`, `svg`, and `svg`'s `desc` carry no htmd
    /// handler registration at all, so `Pure`-mode's own "unregistered tag"
    /// fallback in `dom_walker::walk_node` walks their children and keeps the
    /// content the same way
    /// (htmd's dom_walker.rs `walk_node`). `svg`'s `title` reaches the
    /// body via the same `block_handler` path as top-level `<title>`, since
    /// htmd's handler lookup is keyed by local tag name only, not namespace
    /// (htmd's element_handler/mod.rs `new`, "title" in the block list).
    /// `markdown_converter` builds with `scripting_enabled` left at its
    /// default `true` (htmd's lib.rs `new`), which makes `<noscript>` and
    /// `<iframe>` raw-text elements in html5ever's parse: the markup written
    /// inside them below is captured as one literal Text child rather than
    /// parsed into elements, so the fixture text still reaches the body as a
    /// verbatim substring however that child is walked.
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

    /// [T-FC086] `--raw` の end-to-end で `<script>` の中身が本文へ出ない
    ///
    /// Seam test: runs the real `--raw` path's own extraction
    /// (`extractor::extract_raw`, the function `fetch_page` calls when
    /// `opts.raw` is set) into this file's own `to_fetch_result`, rather than
    /// hand-building an `ExtractedArticle` the way `article()` above does.
    /// `extract_raw` skips Readability entirely and carries the full source
    /// HTML into `content_html` unchanged, so this pins that the element
    /// suppression T-FC084 exercises through Readability-cleaned content also
    /// holds on this raw path, which never runs Readability's own DOM
    /// cleanup.
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

    /// [T-FC089] XHTML 式に自己終了した `<script />` の後ろの本文が消えない
    ///
    /// `check_content_type` (src/fetch/download.rs) accepts
    /// `application/xhtml+xml`, but htmd parses every accepted body as HTML.
    /// The HTML tokenizer ignores the self-closing flag on `script` and enters
    /// raw-text state anyway, so without
    /// `close_self_closed_raw_text_tags` the whole remainder of the document
    /// becomes one Text child of that `<script>` and `suppressed_handler`
    /// drops it with the element. `iframe` covers the same shape for a
    /// raw-text element htmd itself registers as a block element.
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

    /// [T-FC092] JS ソースの中の `<script … />` は書き換えず本文へ漏らさない
    ///
    /// The rewrite scans the byte string, so it has to track raw-text state
    /// itself or it will rewrite a `<script … />` that a JS string literal
    /// contains. In raw-text state the tokenizer ends `<script>` on `</script`
    /// and nothing else, so inserting one there closes the element early and
    /// spills the remaining source into the body as text — the exact leak
    /// `suppressed_handler` closes.
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

    /// [T-FC090] 自己終了タグの書き換えがタグ名境界と引用符つき属性値を守る
    ///
    /// Two ways the scan can overreach: matching a longer tag name that merely
    /// starts with a target name, and reading the `>` inside a quoted
    /// attribute value as the end of the start tag. The third case pins that a
    /// tag closed the ordinary way is returned untouched, so the rewrite adds
    /// no end tag where the author already wrote one.
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

    /// [T-FC091] SVG 名前空間の外の `<desc>` の中身は本文に残る
    ///
    /// htmd dispatches handlers by local tag name only, so registering `desc`
    /// for suppression reaches every element with that name. Outside `<svg>`,
    /// html5ever puts `<desc>` in the XHTML namespace and a browser renders
    /// its text like any unknown inline element, so suppressing it would
    /// delete text the reader sees. This pins both sides of the namespace
    /// check in one fixture: the SVG `<desc>` still goes away.
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

    /// [T-FC078] 表セルの中のコードブロックがバッククォート 1 個で挟んだインラインコードで出る
    ///
    /// A `<pre><code>` written inside a `<td>`/`<th>` must render as inline
    /// code delimited by a single backtick, not as the 3-line fenced block a
    /// `<pre><code>` gets outside a table (T-FC020, T-FC081).
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

    /// [T-FC093] `<code>` 子を持たない表セルの `<pre>` もインラインコードで出る
    ///
    /// The cell branch sits ahead of the `<code>`-child split, so a bare
    /// `<pre>` fences the same way outside a cell and would leave its own
    /// backticks as cell text.
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

    /// [T-FC094] 表セルの `<pre>` で `<code>` の外側にある兄弟テキストが残る
    ///
    /// Reading only a `<code>` child would drop text the author wrote beside
    /// it, which the non-cell path keeps.
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

    /// [T-FC095] 表セルの `<pre>` でも `<script>` の中身は落ちる
    ///
    /// `suppressed_handler` drops a `<script>` body everywhere else, the raw
    /// fallback path included. A cell's `<pre>` reads its own subtree instead
    /// of the walked text, so it has to apply the same suppression itself.
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

    /// [T-FC096] 表セルの `<pre>` の `<br>` が空白 1 個の区切りとして残る
    ///
    /// A `<br>` carries no Text child, so concatenating text alone would run
    /// the two lines together into one word. The cell folds the line break to
    /// a space the same way it folds a Text child's `\n` (T-FC093).
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

    /// [T-FC079] セルの中身が 3 連バッククォートを含むとき区切りが 4 連へ伸びる
    ///
    /// The delimiter width is the content's longest backtick run plus 1
    /// (CommonMark 0.31.2 §6.3), the same rule `fence_delimiter` applies for a
    /// block fence but computed independently here for an inline delimiter,
    /// which carries no 3-backtick floor.
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

    /// [T-FC080] セルの中身の先頭と末尾がバッククォートのとき区切りの内側に空白 1 個が入る
    ///
    /// CommonMark 0.31.2 §6.3: when the code span's contents start or end
    /// with a backtick, a single space inside each delimiter keeps the
    /// content's own backtick from reading as part of the delimiter.
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

    /// [T-FC081] 表の外の `<pre>` は従来どおりフェンスで出る
    ///
    /// Regression guard alongside T-FC078-080: only a `<pre>` with a
    /// `<td>`/`<th>` ancestor switches to inline code. A `<pre><code>` with no
    /// such ancestor must keep the 3-line fenced block T-FC020 already pins.
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

    /// [T-FC104] 上限を超える title を持つ記事の出力で frontmatter が閉じ本文が残る
    ///
    /// `to_fetch_result` builds the frontmatter, then the caller's byte cap
    /// cuts the result. Without the field cap the cut lands inside the block,
    /// leaving an unclosed `---` and no body.
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

    /// [T-FC098] 表セルの code span のパイプが `\|` で出る
    ///
    /// GFM unescapes `\|` while splitting the row, before inline parsing, so it
    /// resolves inside a code span too. An entity reference does not: both
    /// pulldown-cmark 0.13.4 and comrak 0.54.0 render `&#124;` there as six
    /// literal characters.
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

    /// [T-FC099] `\` で終わるセルの中身と閉じパイプの間に空白が入る
    ///
    /// Writing the pipe as `&#124;` leaves no `|` in the cell string at all, so
    /// a cell cannot reach the row delimiter. `\|` moves that guarantee onto
    /// `format_table_row`'s one space before the closing pipe: a bare trailing
    /// `\` right before `|` would read as an escaped delimiter and swallow the
    /// next cell.
    ///
    /// No path reaches that shape today, since `inline_code_span` always closes
    /// a cell's `<pre>` with a backtick. The assertion is on the row rather
    /// than `normalize_cell_content`, because the space is what would hold.
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

    /// [T-FC097] Readability が失敗した経路でも `<script>` の中身は本文へ出ない
    ///
    /// `extract_article`'s dom_smoothie fallback (T-FX017) reaches this layer's
    /// suppression the same way `--raw` does, and this test walks the real
    /// seam rather than building an `ExtractedArticle` literal.
    #[test]
    fn readability_failure_path_still_drops_script_content() {
        // Marker without `_`: htmd's own text escaping (not this crate's
        // `escape_md_inline`) would rewrite it to `SCRIPT\_MARKER`
        // (htmd's dom_walker.rs `escape_if_needed`), and `contains` would miss the
        // leak it is meant to catch.
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
