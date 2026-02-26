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
}
