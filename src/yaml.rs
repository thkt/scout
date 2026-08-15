//! Frontmatter YAML neutralization helpers shared by the fetch and Slack paths.
//!
//! Limited to frontmatter neutralization; this module does not parse or serialize YAML.

use std::borrow::Cow;
use std::fmt::Write;

use crate::markdown::{track_fence, truncate_with_note};

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
        append_marker_rewritten(&mut out, line);
    }
    out
}

/// The rewrite rule shared by [`neutralize_yaml_markers`] and
/// [`neutralize_yaml_markers_outside_fences`]: append `line` to `out`, rewriting
/// a bare YAML document marker (see [`yaml_marker_rest`]) to `***` and leaving
/// every other line untouched.
fn append_marker_rewritten(out: &mut String, line: &str) {
    match yaml_marker_rest(line) {
        Some(rest) if rest.trim_matches([' ', '\t', '\r']).is_empty() => out.push_str("***"),
        Some(rest) => {
            out.push_str("***");
            out.push_str(rest);
        }
        None => out.push_str(line),
    }
}

/// Apply [`neutralize_yaml_markers`]'s rewrite rule only to lines outside a
/// fenced code block, so a marker line quoted inside a closed fence (e.g. shown
/// as sample output in a code block) is left as ordinary content instead of
/// being mistaken for an actual YAML document boundary.
///
/// A body ending with a fence still open falls back to the whole-body rewrite
/// rather than keeping the partial per-line result: an unclosed fence is more
/// likely a stray backtick run than a real code block, and a partial result
/// would leave every line after it unprotected.
///
/// `src/slack/format.rs` keeps calling the fence-unaware
/// [`neutralize_yaml_markers`]. Slack message text passes to the leaf nearly
/// raw, so an attacker-authored unclosed fence there would turn off
/// neutralization for everything after it.
pub(crate) fn neutralize_yaml_markers_outside_fences(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<(char, usize)> = None;
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if track_fence(&mut fence, line) {
            out.push_str(line);
        } else {
            append_marker_rewritten(&mut out, line);
        }
    }
    if fence.is_some() {
        return neutralize_yaml_markers(body);
    }
    out
}

/// Re-neutralize the tail of already-truncated output that a byte-cap cut left
/// with a fenced code block open.
///
/// [`neutralize_yaml_markers_outside_fences`] runs once, over the whole page
/// body, before any output cap is applied: a marker line inside a fence that
/// closes before the body ends is left verbatim, because at that point the
/// fence genuinely protects it as quoted content rather than a forged
/// document boundary. A later byte-cap truncation (`truncate_with_note`) can
/// then cut the already-neutralized text past that marker but before the
/// fence's own closing delimiter. The fence that was closed when
/// neutralization ran is left dangling open in the truncated text, so the
/// marker inside it is exposed as a live, unprotected column-0 `---`/`...`
/// line — this function closes that gap.
///
/// Scans `truncated` forward once with [`track_fence`] and records the byte
/// offset of the line that last opened a fence (`None` → `Some`) without that
/// fence closing again before the text ends. A backward scan cannot make this
/// distinction: reading from the end toward the front, a line that looks like
/// a fence delimiter could be the dangling block's own opening line or an
/// inner line that merely resembles one, and only a forward pass carries the
/// open/close state needed to tell them apart.
///
/// Once a dangling fence is found, [`neutralize_yaml_markers`] (the
/// fence-unaware rewrite) reruns over the byte range from that fence's own
/// opening line to the end, so any marker line in that range — which lost its
/// protection along with the closing delimiter that used to follow it — is
/// rewritten to `***` like ordinary unfenced content.
pub(crate) fn reneutralize_dangling_fence(truncated: &str) -> Cow<'_, str> {
    let mut fence: Option<(char, usize)> = None;
    let mut dangling_start: Option<usize> = None;
    let mut offset = 0usize;
    for line in truncated.split('\n') {
        let was_open = fence.is_some();
        track_fence(&mut fence, line);
        match (was_open, fence.is_some()) {
            (false, true) => dangling_start = Some(offset),
            (true, false) => dangling_start = None,
            _ => {}
        }
        offset += line.len() + 1;
    }
    match dangling_start {
        Some(start) => {
            let mut out = String::with_capacity(truncated.len());
            out.push_str(&truncated[..start]);
            out.push_str(&neutralize_yaml_markers(&truncated[start..]));
            Cow::Owned(out)
        }
        None => Cow::Borrowed(truncated),
    }
}

/// [`truncate_with_note`] followed by [`reneutralize_dangling_fence`] — the
/// pairing every caller that truncates already fence-neutralized markdown
/// needs, since a byte-cap cut can dangle open a fence that was closed when
/// neutralization ran (see `reneutralize_dangling_fence`'s doc). Both fetch
/// output and the research report's per-page rendering truncate that way, so
/// they share this one call instead of each chaining the two steps itself.
pub(crate) fn truncate_and_reneutralize(s: &str, max_bytes: usize) -> Cow<'_, str> {
    let truncated = truncate_with_note(s, max_bytes);
    match reneutralize_dangling_fence(&truncated) {
        Cow::Borrowed(_) => truncated,
        Cow::Owned(rewritten) => Cow::Owned(rewritten),
    }
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
fn escape_yaml(s: &str) -> Cow<'_, str> {
    // The common frontmatter value (a plain title/author/date) carries no escapable
    // char, so the loop below would allocate a copy identical to its input.
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

    /// [T-FC013] C0 control characters other than `\0\n\r\t` pass through
    ///
    /// ADR-0014 accepts this: the escape set covers what breaks out of a
    /// double-quoted scalar, and the primary consumer is an agent rather than a
    /// terminal. Two things follow that the ADR does not state — the value stays
    /// borrowed (ESC is not in the escape scan), and the emitted scalar carries a
    /// byte YAML 1.2 excludes from c-printable, so a strict parser rejects it.
    /// Pinned here so a later change to either behaviour has to revisit the ADR
    /// rather than pass unnoticed.
    #[test]
    fn escape_yaml_passes_control_characters_through() {
        let with_esc = "title\u{1b}[31m";
        assert!(
            matches!(escape_yaml(with_esc), Cow::Borrowed(_)),
            "ESC is outside the escape scan, so the value is not copied"
        );
        assert_eq!(escape_yaml(with_esc), with_esc);
        assert_eq!(escape_yaml("bell\u{7}"), "bell\u{7}");
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

    /// [T-FC030] 閉じないフェンスの後ろにある YAML マーカー行も *** に書き換わる
    #[test]
    fn yaml_marker_after_unclosed_fence_is_still_rewritten() {
        assert_eq!(
            neutralize_yaml_markers_outside_fences("```yaml\n---\nfoo"),
            "```yaml\n***\nfoo"
        );
    }

    /// [T-FC031] 閉じたフェンスの内側にある YAML マーカー行は原文のまま残る
    #[test]
    fn yaml_marker_inside_closed_fence_is_preserved() {
        assert_eq!(
            neutralize_yaml_markers_outside_fences("```\n---\n```\n...\nafter"),
            "```\n---\n```\n***\nafter"
        );
    }

    /// [T-FC032] フェンスの外側にある YAML マーカー行は *** に書き換わる
    #[test]
    fn yaml_marker_outside_fence_is_rewritten() {
        assert_eq!(
            neutralize_yaml_markers_outside_fences("---\n```\ncode\n```\n..."),
            "***\n```\ncode\n```\n***"
        );
    }

    /// [T-FC033] 4 個のフェンスの内側にある 3 個のバッククォート行は閉じと見なされない
    #[test]
    fn three_backtick_line_inside_four_backtick_fence_does_not_close_it() {
        assert_eq!(
            neutralize_yaml_markers_outside_fences("````\n```\n---\n````\n..."),
            "````\n```\n---\n````\n***"
        );
    }
}
