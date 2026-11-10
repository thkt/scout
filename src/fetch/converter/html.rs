//! dom_query's built-in Markdown output is not used: it changed the output of
//! 31 of the 77 converter tests at the time (DR-0036).

mod table;
#[cfg(test)]
mod tests;

use dom_query::{Document, NodeData, NodeRef};
use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{ParseOpts, parse_document};

use crate::markdown::{escape_md_inline, fence_delimiter};

pub(super) fn convert(source: &str) -> String {
    let document = parse_document(
        Document::default(),
        ParseOpts {
            tree_builder: TreeBuilderOpts {
                scripting_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .one(source);
    children(document.root(), false)
        .trim_matches('\n')
        .trim_end_matches([' ', '\t', '\r'])
        .to_owned()
}

fn tag(node: NodeRef<'_>) -> String {
    node.node_name()
        .map(|name| name.to_string())
        .unwrap_or_default()
}

fn suppressed(node: NodeRef<'_>) -> bool {
    match tag(node).as_str() {
        "script" | "style" | "noscript" | "textarea" | "iframe" | "title" | "template" => true,
        "desc" => node
            .qual_name_ref()
            .is_some_and(|name| &*name.ns == "http://www.w3.org/2000/svg"),
        _ => false,
    }
}

fn children(node: NodeRef<'_>, preformatted: bool) -> String {
    let mut out = String::new();
    for child in node.children_it(false) {
        let addition = render(child, preformatted);
        if preformatted && child.is_text() {
            out.push_str(&addition);
            continue;
        }
        let trailing = out.len() - out.trim_end_matches('\n').len();
        let leading = addition.len() - addition.trim_start_matches('\n').len();
        let excess = (trailing + leading).saturating_sub(2);
        let from_out = excess.min(trailing);
        out.truncate(out.len() - from_out);
        let addition = &addition[excess - from_out..];
        if (out.is_empty() || out.ends_with([' ', '\n'])) && addition.starts_with(' ') {
            out.push_str(addition.trim_start_matches(' '));
        } else {
            out.push_str(addition);
        }
    }
    out
}

fn render(node: NodeRef<'_>, preformatted: bool) -> String {
    if node.is_text() {
        let text = node.text();
        return if preformatted {
            text.to_string()
        } else {
            escape_text(&collapse(&text))
        };
    }
    if suppressed(node) {
        return String::new();
    }
    if !node.is_element() {
        return if node.query_or(false, |node| {
            matches!(node.data, NodeData::Document | NodeData::Fragment)
        }) {
            children(node, preformatted)
        } else {
            String::new()
        };
    }
    let name = tag(node);
    match name.as_str() {
        "pre" => render_pre(node),
        "span" => render_span(node, preformatted),
        "table" => table::render(node),
        "code" => render_code(node),
        "a" => render_link(node, preformatted),
        "img" => {
            let Some(src) = node.attr("src") else {
                return String::new();
            };
            let alt = node
                .attr("alt")
                .map(|alt| escape_text(&collapse(&alt)))
                .unwrap_or_default();
            let title = node
                .attr("title")
                .map(|title| format!(" \"{}\"", normalize_title(&title)))
                .unwrap_or_default();
            format!("![{alt}]({}{title})", destination(&src))
        }
        "ul" | "ol" => render_list(node),
        "blockquote" => {
            let content = children(node, preformatted);
            let lines = trim(&content)
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            block(&lines)
        }
        "br" => "  \n".into(),
        "hr" => block("* * *"),
        "strong" | "b" => emphasis(node, "**", preformatted),
        "em" | "i" => emphasis(node, "*", preformatted),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = usize::from(name.as_bytes()[1] - b'0');
            block(&format!(
                "{} {}",
                "#".repeat(level),
                trim(&children(node, preformatted))
            ))
        }
        "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "aside"
        | "address" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "details" | "summary" => {
            block(trim(&children(node, preformatted)))
        }
        _ => children(node, preformatted),
    }
}

fn render_span(node: NodeRef<'_>, preformatted: bool) -> String {
    if node
        .ancestors(None)
        .into_iter()
        .any(|ancestor| tag(ancestor) == "pre")
    {
        return children(node, preformatted);
    }
    let attrs = node.attrs();
    let parts = node.children();
    if attrs.len() == 1
        && parts.len() == 1
        && parts[0].is_text()
        && let Some(class) = node.attr("class")
    {
        let delimiter = match class.as_ref() {
            "math math-inline" => Some("$"),
            "math math-display" => Some("$$"),
            _ => None,
        };
        // Math is emitted unescaped so TeX survives. Text that could open a
        // block, a link, a code span or HTML takes the escaped path below.
        let text = parts[0].text();
        if let Some(delimiter) = delimiter
            && !text.contains(['\n', '\r', '[', ']', '<', '`'])
        {
            return format!("{delimiter}{text}{delimiter}");
        }
    }
    let content = children(node, preformatted);
    if preformatted {
        content
    } else {
        content.trim_matches('\n').to_owned()
    }
}

fn emphasis(node: NodeRef<'_>, marker: &str, preformatted: bool) -> String {
    let content = children(node, preformatted);
    let inner = trim(&content);
    if inner.is_empty() {
        return content;
    }
    let leading = content.len() - content.trim_start_matches([' ', '\n', '\t', '\r']).len();
    let trailing = content.trim_end_matches([' ', '\n', '\t', '\r']).len();
    format!(
        "{}{marker}{inner}{marker}{}",
        &content[..leading],
        &content[trailing..]
    )
}

fn raw_text(node: NodeRef<'_>) -> String {
    if suppressed(node) {
        return String::new();
    }
    if node.is_text() {
        return node.text().to_string();
    }
    if tag(node) == "br" {
        return "\n".into();
    }
    node.children_it(false).map(raw_text).collect()
}

fn render_pre(node: NodeRef<'_>) -> String {
    if node
        .ancestors(None)
        .into_iter()
        .any(|ancestor| matches!(tag(ancestor).as_str(), "td" | "th"))
    {
        return inline_code(raw_text(node).trim_matches('\n'), false);
    }
    let code_children = node
        .children_it(false)
        .filter(|child| tag(*child) == "code")
        .count();
    if code_children == 0 {
        return block(&fenced(children(node, true).trim_matches('\n'), ""));
    }
    // `render_code` fences each `code` child. Sibling content (a filename
    // label, a line-number gutter, a copy button) would otherwise sit on the
    // fence lines and leave a fence open for the rest of the page, so each
    // sibling becomes its own escaped paragraph.
    let parts: Vec<String> = node
        .children_it(false)
        .map(|child| {
            if tag(child) == "code" {
                render(child, true)
            } else {
                escape_text(trim(&collapse(&raw_text(child))))
            }
        })
        .filter(|part| !part.is_empty())
        .collect();
    block(&parts.join("\n\n"))
}

fn render_code(node: NodeRef<'_>) -> String {
    let content = children(node, true);
    if let Some(parent) = node.parent()
        && tag(parent) == "pre"
    {
        let language = node
            .attr("class")
            .or_else(|| parent.attr("class"))
            .and_then(|classes| {
                classes
                    .split(' ')
                    .find_map(|class| class.strip_prefix("language-"))
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        return fenced(content.strip_suffix('\n').unwrap_or(&content), &language);
    }
    inline_code(&content.replace('\n', " "), true)
}

fn fenced(content: &str, language: &str) -> String {
    let fence = fence_delimiter(content);
    format!("{fence}{language}\n{content}\n{fence}")
}

fn inline_code(content: &str, ordinary_inline: bool) -> String {
    let runs: Vec<usize> = content
        .split(|ch| ch != '`')
        .filter(|run| !run.is_empty())
        .map(str::len)
        .collect();
    let longest = runs.iter().copied().max().unwrap_or(0);
    let edge_backtick = content.starts_with('`') || content.ends_with('`');
    let length = if ordinary_inline && !edge_backtick {
        (1..=longest + 1)
            .find(|length| !runs.contains(length))
            .unwrap()
    } else {
        longest + 1
    };
    let delimiter = "`".repeat(length);
    let boundary_spaces = ordinary_inline
        && content.starts_with(' ')
        && content.ends_with(' ')
        && !content.trim_matches(' ').is_empty();
    if edge_backtick || boundary_spaces {
        format!("{delimiter} {content} {delimiter}")
    } else {
        format!("{delimiter}{content}{delimiter}")
    }
}

fn render_link(node: NodeRef<'_>, preformatted: bool) -> String {
    let content = children(node, preformatted);
    let Some(href) = node.attr("href") else {
        return content;
    };
    let empty = trim(&content).is_empty();
    if empty && (href.is_empty() || href.starts_with('#')) {
        return String::new();
    }
    let title = if empty {
        node.attr("title")
            .map(|title| format!(" \"{}\"", normalize_title(&title)))
    } else {
        None
    };
    let trailing = &content[content.trim_end_matches([' ', '\n', '\t', '\r']).len()..];
    if let Some(href) = non_http_scheme_href(&href) {
        return if empty {
            String::new()
        } else {
            // `<…>` is a CommonMark autolink for any scheme and a backtick
            // opens a code span, so neither may reach the body from an href.
            let inert = escape_md_inline(&href)
                .replace('<', "%3C")
                .replace('>', "%3E")
                .replace('`', "%60");
            format!("{} ({inert}){trailing}", trim(&content))
        };
    }
    let destination = destination(&href);
    format!(
        "[{}]({destination}{}){trailing}",
        trim(&content),
        title.unwrap_or_default()
    )
}

/// Returns the href with ASCII whitespace and control characters removed when
/// it names a scheme other than http/https (DR-0014). Browsers ignore those
/// characters when reading the scheme, so they are removed before the check.
/// A scheme-less href (relative path, fragment) returns `None`.
fn non_http_scheme_href(href: &str) -> Option<String> {
    let cleaned: String = href
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && !ch.is_ascii_control())
        .collect();
    let scheme = &cleaned[..cleaned.find(':')?];
    let named = scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'));
    let http = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    (named && !http).then_some(cleaned)
}

/// A line break in the title would let its next line open a CommonMark block,
/// so both CommonMark line endings, LF and CR, fold into spaces. Escaping
/// line-start markers was rejected: it must list every construct that can
/// interrupt a paragraph, and one omission reopens the injection.
///
/// `\` is escaped before `"` so a trailing backslash cannot escape the closing
/// quote.
fn normalize_title(title: &str) -> String {
    title
        .split(['\n', '\r'])
        .map(trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn destination(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace(['\n', '\r'], "");
    if escaped.contains(' ') {
        format!("<{escaped}>")
    } else {
        escaped
    }
}

fn render_list(node: NodeRef<'_>) -> String {
    let ordered = tag(node) == "ol";
    let start = node
        .attr("start")
        .and_then(|start| start.parse::<usize>().ok())
        .unwrap_or(1);
    let mut out = String::new();
    let mut index = start;
    for child in node.children_it(false) {
        if tag(child) != "li" {
            continue;
        }
        let marker = if ordered {
            format!("{index}.  ")
        } else {
            "*   ".into()
        };
        let content = children(child, false);
        let loose = content.ends_with("\n\n");
        let content = trim(&content);
        let indent = " ".repeat(marker.len());
        out.push_str(&marker);
        for (line_index, line) in content.lines().enumerate() {
            if line_index > 0 {
                out.push('\n');
                if !line.is_empty() {
                    out.push_str(&indent);
                }
            }
            out.push_str(line.trim_end_matches([' ', '\t', '\r']));
        }
        out.push('\n');
        if loose {
            out.push('\n');
        }
        index = index.saturating_add(1);
    }
    if node.parent().is_some_and(|parent| tag(parent) == "li") {
        format!("\n{}\n", out.trim_end_matches('\n'))
    } else {
        block(out.trim_end_matches('\n'))
    }
}

fn trim(value: &str) -> &str {
    value.trim_matches([' ', '\n', '\r', '\t'])
}
fn block(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!("\n\n{value}\n\n")
    }
}

fn collapse(text: &str) -> String {
    let mut out = String::new();
    let mut whitespace = false;
    for ch in text.chars() {
        if ch.is_ascii_whitespace() {
            if !whitespace {
                out.push(' ');
            }
            whitespace = true;
        } else {
            out.push(ch);
            whitespace = false;
        }
    }
    out
}

fn escape_text(text: &str) -> String {
    let mut out = String::new();
    // `children` drops a text node's leading spaces at a line start, so block
    // markers are looked for behind them.
    let indent = text.len() - text.trim_start_matches(' ').len();
    let body = &text[indent..];
    let leading = match body.chars().next() {
        Some('=' | '~' | '>') => true,
        Some('-' | '+') => body.as_bytes().get(1) == Some(&b' '),
        Some('#') => body.trim_start_matches('#').starts_with(' '),
        _ => false,
    };
    let ordered = indent + body.bytes().take_while(u8::is_ascii_digit).count();
    for (offset, ch) in text.char_indices() {
        if leading && offset == indent {
            out.push('\\');
        }
        let html = ch == '<'
            && text[offset + 1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?'));
        if matches!(ch, '\\' | '*' | '_' | '`' | '[' | ']')
            || html
            || (ordered > indent
                && offset == ordered
                && ch == '.'
                && text.as_bytes().get(offset + 1) == Some(&b' '))
        {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
