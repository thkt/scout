//! Frontmatter YAML neutralization helpers shared by the fetch and Slack paths.
//!
//! Limited to frontmatter neutralization; this module does not parse or serialize YAML.

use std::borrow::Cow;
use std::fmt::Write;

use crate::markdown::{track_fence, truncate_with_note};
use crate::search::engine::MAX_PAGE_BYTES;

/// Neutralize YAML document markers in untrusted body text appended after a
/// `---`-delimited frontmatter block.
///
/// `***` renders as the same thematic break as `---` but is not a YAML marker,
/// so the body cannot forge a document boundary. Indented and inline `---` are
/// left intact: they are ordinary content to a YAML reader.
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
/// [`neutralize_yaml_markers_outside_fences`].
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

/// Apply [`neutralize_yaml_markers`]'s rewrite rule only outside a fenced code
/// block, so a marker quoted inside a closed fence stays as written.
///
/// A body ending with a fence still open falls back to the whole-body rewrite
/// rather than keeping the partial result: an unclosed fence is more likely a
/// stray backtick run than a real code block, and the partial result would
/// leave every line after it unprotected.
///
/// `src/slack/format.rs` does not use this variant. Slack message text reaches
/// the leaf nearly raw, so one attacker-authored unclosed fence would turn off
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
/// [`neutralize_yaml_markers_outside_fences`] leaves a marker verbatim when a
/// fence closes around it. A byte cap applied later can cut past that marker
/// but before the closing delimiter, exposing it at column 0.
///
/// The scan runs forward rather than backward: read from the end, a line that
/// looks like a fence delimiter could be the dangling block's own opening line
/// or an inner line resembling one, and only a forward pass carries the
/// open/close state that separates the two.
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

/// [`truncate_with_note`] followed by [`reneutralize_dangling_fence`].
///
/// Every caller that truncates already fence-neutralized markdown needs both
/// steps, so they are not offered separately: chaining them at each call site
/// is how one site ends up with only the first half.
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

/// Per-field byte cap for a frontmatter string value, applied before escaping.
///
/// Derived from `search::engine::MAX_PAGE_BYTES` (4_500), the tightest budget a
/// frontmatter block competes with. One field at 1/10 of it holds title, author
/// and date to 3/10 before escaping. [`escape_yaml`] can double a value, so the
/// worst case — every byte a backslash — reaches 6/10 and still leaves the body
/// room inside the page budget.
const MAX_FIELD_BYTES: usize = MAX_PAGE_BYTES / 10;

/// Truncate `value` to [`MAX_FIELD_BYTES`] before it reaches [`escape_yaml`].
///
/// Mirrors `truncate_with_note`'s (`src/markdown.rs`) use of
/// [`str::floor_char_boundary`] to land the cut on a char boundary, but
/// truncates the raw value rather than an escaped one: cutting an
/// already-escaped value can split a doubled `\\\\` in half, leaving a lone
/// trailing `\` that escapes the closing quote and never lets the scalar
/// close. A truncated value gets an ellipsis appended as the sole signal —
/// not `truncate_with_note`'s byte-count note, which does not fit inside a
/// single-line `key: "value"` scalar.
fn truncate_field(value: &str) -> Cow<'_, str> {
    if value.len() <= MAX_FIELD_BYTES {
        return Cow::Borrowed(value);
    }
    let boundary = value.floor_char_boundary(MAX_FIELD_BYTES);
    let mut out = value[..boundary].to_string();
    out.push('…');
    Cow::Owned(out)
}

/// Write one frontmatter key whose value is a string.
///
/// The double quotes and [`escape_yaml`] are one contract, not two steps:
/// `escape_yaml`'s escape set is exactly what a double-quoted YAML scalar needs,
/// so emitting the quotes without the escape (or the reverse) is how a value
/// containing `"` or a newline breaks out of the block. Keeping them in one
/// place means a call site cannot do half of it.
pub(crate) fn write_yaml_str(out: &mut String, key: &str, value: &str) {
    let _ = writeln!(out, "{key}: \"{}\"", escape_yaml(&truncate_field(value)));
}

/// Escape a string for use inside a double-quoted YAML scalar.
///
/// The value-side half of [`write_yaml_str`]'s contract, so a caller writing a
/// frontmatter field reaches for that function instead.
fn escape_yaml(s: &str) -> Cow<'_, str> {
    // A plain title or date carries no escapable char, so the loop below would
    // allocate a copy identical to its input.
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

    /// [T-FC100] 上限を超える title は切り詰められる
    ///
    /// The frontmatter-closes-and-body-survives property this test used to
    /// assert on a hand-built `doc` string never depended on the cap (nothing
    /// here truncates `doc` itself); T-FC104 now owns that property end to
    /// end through `to_fetch_result`.
    #[test]
    fn truncates_title_over_the_cap() {
        let long_title = "A".repeat(10_000);
        let mut fields = String::new();
        write_yaml_str(&mut fields, "title", &long_title);

        assert!(
            fields.len() < long_title.len(),
            "a title over the cap must be truncated, not passed through whole"
        );
    }

    /// [T-FC101] byline と published_time でも上限を超えた値が切り詰められる
    ///
    /// `write_yaml_str` carries no per-key logic, so proving truncation on one
    /// key (title, T-FC100) does not prove it on the other two call sites in
    /// `format_with_frontmatter` (`author` for byline, `date` for
    /// published_time) — each is its own call.
    #[test]
    fn truncates_byline_and_published_time_over_the_cap() {
        let long_byline = "b".repeat(10_000);
        let mut byline_field = String::new();
        write_yaml_str(&mut byline_field, "author", &long_byline);
        assert!(
            byline_field.len() < long_byline.len(),
            "a byline over the cap must be truncated (author field)"
        );

        let long_published_time = format!("2026-08-16T{}", "0".repeat(10_000));
        let mut date_field = String::new();
        write_yaml_str(&mut date_field, "date", &long_published_time);
        assert!(
            date_field.len() < long_published_time.len(),
            "a published_time over the cap must be truncated (date field)"
        );
    }

    /// [T-FC102] escape 対象文字だけの上限超の値でも title 行が引用符で閉じ、末尾が単独のバックスラッシュにならない
    ///
    /// Guards the ordering the contract requires: truncate the raw value, then
    /// escape it. Truncating an already-escaped value instead can cut a
    /// doubled `\\\\` in half, leaving a lone trailing backslash that escapes
    /// the closing quote and never lets the YAML scalar close.
    #[test]
    fn truncated_all_escapable_value_still_closes_the_quote() {
        let long_backslashes = "\\".repeat(10_000);
        let mut fields = String::new();
        write_yaml_str(&mut fields, "title", &long_backslashes);
        let title_line = fields.lines().next().expect("title line");

        assert!(
            title_line.starts_with("title: \""),
            "title line must open its quoted scalar: {title_line}"
        );
        assert!(
            title_line.ends_with('"'),
            "title line must close its quoted scalar, not trail off unterminated: {title_line}"
        );
        let before_closing_quote = &title_line[..title_line.len() - 1];
        let trailing_backslashes = before_closing_quote
            .chars()
            .rev()
            .take_while(|&c| c == '\\')
            .count();
        assert!(
            trailing_backslashes % 2 == 0,
            "an odd run of backslashes right before the closing quote would escape it: {title_line}"
        );
        assert!(
            fields.len() < long_backslashes.len(),
            "an all-backslash title over the cap must be truncated"
        );
    }

    /// [T-FC103] 上限以下の値は切り詰められず省略記号も付かない
    #[test]
    fn value_within_the_cap_is_not_truncated_and_carries_no_ellipsis() {
        let title = "A plain title well under any byte cap";
        let mut fields = String::new();
        write_yaml_str(&mut fields, "title", title);
        assert_eq!(fields, format!("title: \"{title}\"\n"));
    }
}
