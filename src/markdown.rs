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

/// Sanitize user input for embedding in a Markdown heading.
/// Replaces newlines (which would break heading structure) with spaces.
pub(crate) fn sanitize_heading(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// Shift all Markdown heading levels deeper by `levels` (e.g., `# Foo` → `#### Foo`
/// with `levels = 3`).  Skips lines inside fenced code blocks so that comment
/// lines like `# TODO` are not affected.
pub(crate) fn shift_headings(markdown: &str, levels: usize) -> String {
    if levels == 0 {
        return markdown.to_string();
    }
    let prefix = "#".repeat(levels);
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
        if !in_code_block && trimmed.starts_with('#') {
            // Preserve leading whitespace (rare but possible).
            let indent = &line[..line.len() - trimmed.len()];
            out.push_str(indent);
            out.push_str(&prefix);
            out.push_str(trimmed);
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
}
