/// Escape characters that break Markdown link syntax: `[`, `]`, `(`, `)`.
pub(crate) fn escape_md_link(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '[' | ']' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
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
pub(crate) fn sanitize_heading(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// Truncate a string at a char boundary and append a byte-count note.
///
/// Returns the input borrowed if it fits within `max_bytes`.
pub(crate) fn truncate_with_note(s: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if s.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(s);
    }
    let total = s.len();
    let end = s.floor_char_boundary(max_bytes);
    let mut out = s[..end].to_string();
    use std::fmt::Write;
    let _ = write!(out, "\n\n(truncated: showing {end} / {total} bytes)");
    std::borrow::Cow::Owned(out)
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
        return markdown.to_string();
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

    #[test]
    fn escapes_special_chars() {
        assert_eq!(escape_md_link("normal text"), "normal text");
        assert_eq!(escape_md_link("a[b]c(d)e"), r"a\[b\]c\(d\)e");
    }

    #[test]
    fn escape_md_inline_pipes_and_newlines() {
        assert_eq!(escape_md_inline("col1 | col2"), r"col1 \| col2");
        assert_eq!(escape_md_inline("line1\nline2"), "line1 line2");
        assert_eq!(escape_md_inline("a\r\nb"), "a  b");
    }

    #[test]
    fn escape_md_inline_link_syntax() {
        assert_eq!(
            escape_md_inline("[click](http://evil)"),
            r"\[click\]\(http://evil\)"
        );
    }

    #[test]
    fn escape_md_inline_passthrough() {
        assert_eq!(escape_md_inline("normal text"), "normal text");
    }

    #[test]
    fn sanitize_heading_replaces_newlines() {
        assert_eq!(sanitize_heading("line1\nline2\rline3"), "line1 line2 line3");
        assert_eq!(sanitize_heading("no newlines"), "no newlines");
    }

    #[test]
    fn shift_headings_basic() {
        let input = "# H1\n## H2\nParagraph\n### H3";
        let result = shift_headings(input, 3);
        assert_eq!(result, "#### H1\n##### H2\nParagraph\n###### H3");
    }

    #[test]
    fn shift_headings_zero_is_noop() {
        let input = "# Title\nBody";
        assert_eq!(shift_headings(input, 0), input);
    }

    #[test]
    fn shift_headings_skips_code_blocks() {
        let input = "# Real heading\n```\n# comment in code\n```\n## Another heading";
        let result = shift_headings(input, 2);
        assert_eq!(
            result,
            "### Real heading\n```\n# comment in code\n```\n#### Another heading"
        );
    }

    #[test]
    fn shift_headings_preserves_trailing_content() {
        let input = "No headings here\nJust text";
        assert_eq!(shift_headings(input, 3), input);
    }

    /// [T-005] Non-ATX-heading `#` lines must not be shifted.
    #[test]
    fn shift_headings_skips_non_atx_lines() {
        let input = "#include <stdio.h>\n# Real heading\n#123 issue ref\n## Also real";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "#include <stdio.h>\n### Real heading\n#123 issue ref\n#### Also real",
            "only ATX headings (# + space/EOL) should be shifted"
        );
    }

    #[test]
    fn shift_headings_clamps_at_h6() {
        let input = "##### H5\n###### H6\n# H1";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "###### H5\n###### H6\n### H1",
            "shifted headings must clamp at h6 (6 hashes max)"
        );
    }

    #[test]
    fn truncate_with_note_short_input_unchanged() {
        assert_eq!(truncate_with_note("hello", 100), "hello");
    }

    #[test]
    fn truncate_with_note_truncates_with_message() {
        let input = "x".repeat(200);
        let result = truncate_with_note(&input, 100);
        assert!(result.len() < 200);
        assert!(result.contains("(truncated: showing 100 / 200 bytes)"));
    }
}
