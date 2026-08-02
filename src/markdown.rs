use std::borrow::Cow;

/// Escape characters that break Markdown link syntax: `[`, `]`, `(`, `)`.
/// Newlines are folded to spaces so an untrusted value cannot break onto a new
/// line and inject block Markdown (a heading or list item).
pub(crate) fn escape_md_link(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '[' | ']' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Render a Markdown link `[text](url)` only when `url` carries an http/https
/// scheme.  Untrusted URLs with any other scheme (`javascript:`, `data:`, …) are
/// emitted as inert escaped text `text (url)` so they can never become a
/// clickable/executable link target.  Allowlisting fails closed: obfuscated or
/// whitespace-prefixed schemes do not match http/https and are neutralized.
pub(crate) fn md_link(text: &str, url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("http://") || lower.starts_with("https://");
    // A real URL carries no ASCII whitespace or control chars; a raw newline in the
    // link target would break out of `[](…)` and inject Markdown, so route any such
    // value to the inert branch (where newlines are collapsed to spaces).
    let clean = !url
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control());
    if scheme_ok && clean {
        format!("[{}]({})", escape_md_inline(text), escape_md_link(url))
    } else {
        format!("{} ({})", escape_md_inline(text), escape_md_inline(url))
    }
}

/// Sanitize untrusted input for embedding in Markdown table cells and inline text.
/// Prevents column breaks (`|`), row breaks (newlines), and link injection (`[]()`).
pub(crate) fn escape_md_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '|' | '[' | ']' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Sanitize user input for embedding in a Markdown heading.
/// Replaces newlines (which would break heading structure) with spaces.
///
/// Returns the input borrowed when it carries no newline, which is the common case
/// for headings (titles, URLs), so no allocation happens on that path.
pub(crate) fn sanitize_heading(s: &str) -> Cow<'_, str> {
    if !s.contains(['\n', '\r']) {
        return Cow::Borrowed(s);
    }
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// The note appended wherever output is cut at a byte cap.
///
/// Callers that cannot use [`truncate_with_note`] — because they transform the
/// text between the cut and the note — still emit this exact wording, so the
/// two halves cannot drift into two different messages.
pub(crate) fn truncation_note(shown: usize, total: usize) -> String {
    format!("\n\n(truncated: showing {shown} / {total} bytes)")
}

/// Truncate a string at a char boundary and append a byte-count note.
///
/// Returns the input borrowed if it fits within `max_bytes`.
pub(crate) fn truncate_with_note(s: &str, max_bytes: usize) -> Cow<'_, str> {
    if s.len() <= max_bytes {
        return Cow::Borrowed(s);
    }
    let total = s.len();
    let end = s.floor_char_boundary(max_bytes);
    let mut out = s[..end].to_string();
    out.push_str(&truncation_note(end, total));
    Cow::Owned(out)
}

/// Return the heading level (1–6) if `trimmed` is a valid ATX heading
/// (CommonMark §4.2), or `None` otherwise.
fn atx_heading_level(trimmed: &str) -> Option<usize> {
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    (rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')).then_some(hashes)
}

/// Shift all Markdown heading levels deeper by `levels` (e.g., `# Foo` → `#### Foo`
/// with `levels = 3`).  Clamps output at h6 (CommonMark maximum).
///
/// Only valid ATX headings (CommonMark §4.2: 1–6 `#` + space/tab/EOL) are
/// shifted; lines like `#include` or `#123` are left unchanged.
///
/// Skips lines inside fenced code blocks so that comment lines like `# TODO`
/// are not affected.  Note: the fence toggle is simplified — it does not track
/// opening fence character or length, so a 4-backtick fence closed by 3 backticks
/// would mis-toggle.  This is acceptable for LLM/web-fetched markdown input.
pub(crate) fn shift_headings(markdown: &str, levels: usize) -> String {
    if levels == 0 {
        return markdown.to_owned();
    }
    let mut in_code_block = false;
    let mut out = String::with_capacity(markdown.len() + levels * 40);

    for (i, line) in markdown.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
        }
        if let Some(orig_hashes) = (!in_code_block)
            .then(|| atx_heading_level(trimmed))
            .flatten()
        {
            let indent = &line[..line.len() - trimmed.len()];
            let new_level = (orig_hashes + levels).min(6);
            let heading_text = &trimmed[orig_hashes..];
            out.push_str(indent);
            out.push_str(&"######"[..new_level]);
            out.push_str(heading_text);
        } else {
            out.push_str(line);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-MD001] escape_md_link brackets and parens
    #[test]
    fn escapes_special_chars() {
        assert_eq!(escape_md_link("normal text"), "normal text");
        assert_eq!(escape_md_link("a[b]c(d)e"), r"a\[b\]c\(d\)e");
        // Newlines fold to spaces so the value cannot inject a new Markdown line.
        assert_eq!(escape_md_link("a\n## h"), "a ## h");
    }

    /// [T-MD002]
    #[test]
    fn escape_md_inline_pipes_and_newlines() {
        assert_eq!(escape_md_inline("col1 | col2"), r"col1 \| col2");
        assert_eq!(escape_md_inline("line1\nline2"), "line1 line2");
        assert_eq!(escape_md_inline("a\r\nb"), "a  b");
    }

    /// [T-MD003] escape_md_inline escapes link syntax
    #[test]
    fn escape_md_inline_link_syntax() {
        assert_eq!(
            escape_md_inline("[click](http://evil)"),
            r"\[click\]\(http://evil\)"
        );
    }

    /// [T-MD004] escape_md_inline passes normal text through
    #[test]
    fn escape_md_inline_passthrough() {
        assert_eq!(escape_md_inline("normal text"), "normal text");
    }

    /// [T-MD005] sanitize_heading replaces newlines with spaces
    #[test]
    fn sanitize_heading_replaces_newlines() {
        assert_eq!(sanitize_heading("line1\nline2\rline3"), "line1 line2 line3");
        assert_eq!(sanitize_heading("no newlines"), "no newlines");
    }

    /// [T-MD011] sanitize_heading borrows input without newlines (no allocation)
    #[test]
    fn sanitize_heading_borrows_when_no_newline() {
        assert!(matches!(
            sanitize_heading("plain heading"),
            Cow::Borrowed(_)
        ));
    }

    /// [T-MD006] shift_headings deepens levels by N
    #[test]
    fn shift_headings_basic() {
        let input = "# H1\n## H2\nParagraph\n### H3";
        let result = shift_headings(input, 3);
        assert_eq!(result, "#### H1\n##### H2\nParagraph\n###### H3");
    }

    /// [T-MD007]
    #[test]
    fn shift_headings_zero_is_noop() {
        let input = "# Title\nBody";
        assert_eq!(shift_headings(input, 0), input);
    }

    /// [T-MD008] shift_headings skips lines inside fenced code blocks
    #[test]
    fn shift_headings_skips_code_blocks() {
        let input = "# Real heading\n```\n# comment in code\n```\n## Another heading";
        let result = shift_headings(input, 2);
        assert_eq!(
            result,
            "### Real heading\n```\n# comment in code\n```\n#### Another heading"
        );
    }

    /// [T-MD009] shift_headings preserves lines without headings
    #[test]
    fn shift_headings_preserves_trailing_content() {
        let input = "No headings here\nJust text";
        assert_eq!(shift_headings(input, 3), input);
    }

    /// [T-MD013] Non-ATX-heading `#` lines must not be shifted.
    #[test]
    fn shift_headings_skips_non_atx_lines() {
        let input = "#include <stdio.h>\n# Real heading\n#123 issue ref\n## Also real";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "#include <stdio.h>\n### Real heading\n#123 issue ref\n#### Also real",
            "only ATX headings (# + space/EOL) should be shifted"
        );
    }

    /// [T-MD010]
    #[test]
    fn shift_headings_clamps_at_h6() {
        let input = "##### H5\n###### H6\n# H1";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "###### H5\n###### H6\n### H1",
            "shifted headings must clamp at h6 (6 hashes max)"
        );
    }

    /// [T-MD014] md_link renders a clickable link for http/https targets
    #[test]
    fn md_link_renders_safe_scheme() {
        assert_eq!(md_link("A", "https://a.com"), "[A](https://a.com)");
        assert_eq!(md_link("A", "http://a.com"), "[A](http://a.com)");
    }

    /// [T-MD015] md_link neutralizes javascript:/data: targets to inert text
    #[test]
    fn md_link_neutralizes_unsafe_scheme() {
        assert_eq!(
            md_link("click", "javascript:alert(1)"),
            r"click (javascript:alert\(1\))"
        );
        assert_eq!(
            md_link("x", "data:text/html,<script>"),
            "x (data:text/html,<script>)"
        );
    }

    /// [T-MD016] md_link fails closed on scheme obfuscation (case, leading space)
    #[test]
    fn md_link_unsafe_scheme_obfuscation_fails_closed() {
        assert_eq!(
            md_link("x", "JaVaScRiPt:alert(1)"),
            r"x (JaVaScRiPt:alert\(1\))"
        );
        // Leading whitespace is not http/https -> inert (whitespace newlines collapsed).
        assert_eq!(md_link("x", " javascript:1"), "x ( javascript:1)");
    }

    /// [T-MD017] md_link escapes link syntax in the visible text
    #[test]
    fn md_link_escapes_text() {
        assert_eq!(md_link("a]b", "https://a.com"), r"[a\]b](https://a.com)");
    }

    /// [T-MD018] md_link routes a newline-bearing URL to the inert branch so it
    /// cannot break out of `[](…)` and inject Markdown
    #[test]
    fn md_link_newline_in_url_cannot_break_out() {
        let out = md_link("x", "https://a.com\n## Injected");
        assert!(!out.contains("](https://"), "must not stay a link: {out}");
        assert!(!out.contains('\n'), "newline must be collapsed: {out}");
        assert_eq!(out, "x (https://a.com ## Injected)");
    }

    /// [T-MD019]
    #[test]
    fn truncate_with_note_short_input_unchanged() {
        assert_eq!(truncate_with_note("hello", 100), "hello");
    }

    /// [T-MD012] truncate_with_note appends byte-count note when truncated
    #[test]
    fn truncate_with_note_truncates_with_message() {
        let input = "x".repeat(200);
        let result = truncate_with_note(&input, 100);
        assert!(result.len() < 200);
        assert!(result.contains("(truncated: showing 100 / 200 bytes)"));
    }
}
