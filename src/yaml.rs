//! Frontmatter YAML neutralization helpers shared by the fetch and Slack paths.
//!
//! Limited to frontmatter neutralization; this module does not parse or serialize YAML.

use std::borrow::Cow;
use std::fmt::Write;

/// Neutralize YAML document markers in untrusted body text appended after a
/// `---`-delimited frontmatter block.  A line that is exactly `---` or `...` (a
/// YAML document start/end marker, and also a Markdown thematic break) is rewritten
/// to `***`, which renders as the same thematic break but is not a YAML marker, so
/// the body cannot inject a document boundary or a forged frontmatter block.  Only
/// column-0 markers are rewritten; indented or inline `---` is ordinary content and
/// left intact.
pub(crate) fn neutralize_yaml_markers(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match yaml_marker_rest(line) {
            Some(rest) if rest.trim_matches([' ', '\t', '\r']).is_empty() => out.push_str("***"),
            Some(rest) => {
                out.push_str("***");
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// If `line` is a YAML document marker (`---` start or `...` end) at column 0 —
/// the three chars followed by end-of-line or whitespace — return the text after
/// the marker (`""` for a bare marker).  `----` and `...foo` are not markers.
fn yaml_marker_rest(line: &str) -> Option<&str> {
    let token = line
        .strip_prefix("---")
        .or_else(|| line.strip_prefix("..."))?;
    (token.is_empty() || token.starts_with([' ', '\t', '\r'])).then_some(token)
}

/// Write one frontmatter key whose value is a string.
///
/// The double quotes and [`escape_yaml`] are one contract, not two steps:
/// `escape_yaml`'s escape set is exactly what a double-quoted YAML scalar needs,
/// so emitting the quotes without the escape (or the reverse) is how a value
/// containing `"` or a newline breaks out of the block. Keeping them in one
/// place means a call site cannot do half of it.
pub(crate) fn write_yaml_str(out: &mut String, key: &str, value: &str) {
    let _ = writeln!(out, "{key}: \"{}\"", escape_yaml(value));
}

/// Escape a string for use inside a double-quoted YAML scalar.
///
/// The value-side half of [`write_yaml_str`]'s contract, so a caller writing a
/// frontmatter field reaches for that function instead. ADR-0014's
/// neutralization table pins this escape set separately from the quoting.
pub(crate) fn escape_yaml(s: &str) -> Cow<'_, str> {
    // The common frontmatter value (a plain title/author/date) carries no escapable
    // char, so borrow it untouched instead of allocating a copy.
    if !s
        .bytes()
        .any(|b| matches!(b, b'\\' | b'"' | b'\n' | b'\r' | b'\t' | b'\0'))
    {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => {}
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-FC003]
    #[test]
    fn escapes_yaml_special_chars() {
        assert_eq!(escape_yaml(r#"He said "hello""#), r#"He said \"hello\""#);
        assert_eq!(escape_yaml(r"back\slash"), r"back\\slash");
        assert_eq!(escape_yaml("line\nbreak"), "line\\nbreak");
        assert_eq!(escape_yaml("cr\rreturn"), "cr\\rreturn");
        assert_eq!(escape_yaml("tab\there"), "tab\\there");
        assert_eq!(escape_yaml("null\0byte"), "nullbyte");
    }

    /// [T-FC004]
    #[test]
    fn escapes_combined_special_chars() {
        // Backslash-first ordering prevents double-escape: \" must not become \\\"
        assert_eq!(
            escape_yaml("She said \"hi\"\nand left\\"),
            "She said \\\"hi\\\"\\nand left\\\\"
        );
    }

    /// [T-FC012] escape_yaml borrows input that needs no escaping (no allocation)
    #[test]
    fn escape_yaml_borrows_when_no_escape_needed() {
        assert!(matches!(escape_yaml("plain title 2026"), Cow::Borrowed(_)));
    }

    /// [T-FC005] neutralize_yaml_markers rewrites bare ---/... lines to ***
    #[test]
    fn neutralize_yaml_markers_rewrites_bare_doc_markers() {
        assert_eq!(
            neutralize_yaml_markers("before\n---\nmiddle\n...\nafter"),
            "before\n***\nmiddle\n***\nafter"
        );
        assert_eq!(neutralize_yaml_markers("---  "), "***");
        assert_eq!(neutralize_yaml_markers("...\r"), "***");
    }

    /// [T-FC006] neutralize_yaml_markers leaves indented and inline --- intact
    #[test]
    fn neutralize_yaml_markers_preserves_non_markers() {
        // Indented `---` is not a YAML document marker (must be at column 0).
        assert_eq!(neutralize_yaml_markers("  ---"), "  ---");
        assert_eq!(neutralize_yaml_markers("a --- b"), "a --- b");
        assert_eq!(neutralize_yaml_markers("----"), "----");
        assert_eq!(neutralize_yaml_markers("...foo"), "...foo");
        assert_eq!(neutralize_yaml_markers("plain"), "plain");
    }

    /// [T-FC007] neutralize_yaml_markers catches markers carrying inline content
    /// (`--- evil: true`), which a YAML parser still honors as a document start
    #[test]
    fn neutralize_yaml_markers_rewrites_marker_with_content() {
        assert_eq!(neutralize_yaml_markers("--- evil: true"), "*** evil: true");
        assert_eq!(neutralize_yaml_markers("--- #x"), "*** #x");
        assert_eq!(neutralize_yaml_markers("---\tfoo"), "***\tfoo");
        assert_eq!(neutralize_yaml_markers("... bar"), "*** bar");
    }
}
