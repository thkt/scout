use std::borrow::Cow;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

/// Escape link delimiters and fold CR/LF to prevent block injection.
/// `|` is safe in link destinations; visible text uses [`escape_md_inline`].
fn escape_md_link(s: &str) -> String {
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

/// Allow only http/https links without ASCII whitespace or controls;
/// render other URLs as escaped, inert text.
pub(crate) fn md_link(text: &str, url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("http://") || lower.starts_with("https://");
    let clean = !url
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control());
    if scheme_ok && clean {
        format!("[{}]({})", escape_md_inline(text), escape_md_link(url))
    } else {
        format!("{} ({})", escape_md_inline(text), escape_md_inline(url))
    }
}

/// Prevent table column/row breaks and link injection in untrusted inline text.
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

/// Fold CR/LF to spaces; borrow input that needs no change.
pub(crate) fn sanitize_heading(s: &str) -> Cow<'_, str> {
    if !s.contains(['\n', '\r']) {
        return Cow::Borrowed(s);
    }
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// Shared byte-count note for callers that transform text after the cut.
pub(crate) fn truncation_note(shown: usize, total: usize) -> String {
    format!("\n\n(truncated: showing {shown} / {total} bytes)")
}

/// Cut at a UTF-8/line boundary and append a byte-count note.
///
/// Prefer a complete line so cutting `-------` cannot manufacture `---`.
/// Use the byte boundary only if no preceding newline exists; borrow uncut input.
pub(crate) fn truncate_with_note(s: &str, max_bytes: usize) -> Cow<'_, str> {
    if s.len() <= max_bytes {
        return Cow::Borrowed(s);
    }
    let total = s.len();
    let boundary = s.floor_char_boundary(max_bytes);
    let end = s[..boundary].rfind('\n').map(|p| p + 1).unwrap_or(boundary);
    let mut out = s[..end].to_string();
    out.push_str(&truncation_note(end, total));
    Cow::Owned(out)
}

/// Count leading `---`-delimited frontmatter lines; return 0 if unclosed.
/// Skipping this block prevents a closing `---` from becoming the setext
/// underline of a metadata field (DR-0014).
fn frontmatter_len(lines: &[&str]) -> usize {
    if lines.first() != Some(&"---") {
        return 0;
    }
    lines[1..]
        .iter()
        .position(|l| *l == "---")
        .map_or(0, |p| p + 2)
}

/// Recognize setext h1/h2 underlines after paragraph-like text (CommonMark §4.3).
/// A blank predecessor makes dashes a thematic break instead.
fn setext_heading_level(text: &str, underline: &str) -> Option<usize> {
    let text = text.trim();
    if text.is_empty() || text.starts_with(['#', '-', '*', '+', '>', '|', '=']) {
        return None;
    }
    if text.starts_with("```") || text.starts_with("~~~") {
        return None;
    }
    let underline = underline.trim_end();
    let mut chars = underline.chars();
    let first = chars.next()?;
    if !matches!(first, '=' | '-') || !chars.all(|c| c == first) {
        return None;
    }
    Some(if first == '=' { 1 } else { 2 })
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

/// Build a safe fenced code block delimiter that is longer than any backtick
/// run found in `content`.
pub(crate) fn fence_delimiter(content: &str) -> String {
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
    "`".repeat(max_run.max(2) + 1)
}

/// Track same-character fence runs; shorter runs cannot close a wider opener.
/// Return true for protected content and delimiter lines. This conservative
/// tracker also accepts fence-looking inline code; YAML defense relies on it.
pub(crate) fn track_fence(fence: &mut Option<(char, usize)>, line: &str) -> bool {
    let marker = fence_marker(line.trim_start());
    match (*fence, marker) {
        (None, Some((c, len))) => *fence = Some((c, len)),
        (Some((open_c, open_len)), Some((c, len))) if c == open_c && len >= open_len => {
            *fence = None;
        }
        _ => {}
    }
    fence.is_some() || marker.is_some()
}

/// Find a dangling top-level fence at a report composition boundary.
/// Parse block context before examining delimiters: fence-looking lines in
/// HTML, lists, blockquotes or indented code must not manufacture a new block.
/// YAML defense continues to use the separate conservative `track_fence`.
pub(crate) fn dangling_report_fence(body: &str) -> Option<(usize, char, usize)> {
    let parsed = report_parser_input(body);
    let mut depth = 0;
    for (event, range) in Parser::new(&parsed).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 && matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(_))) {
                    let block = &parsed[range.clone()];
                    let (marker, width) = fence_marker(block)?;
                    let closed = block.split_once('\n').is_some_and(|(_, rest)| {
                        rest.lines().last().is_some_and(|line| {
                            let indent = line.bytes().take_while(|&b| b == b' ').count();
                            let trimmed = &line[indent..];
                            indent <= 3
                                && fence_marker(trimmed).is_some_and(|(c, len)| {
                                    c == marker
                                        && len >= width
                                        && trimmed[len..].trim_matches([' ', '\t', '\r']).is_empty()
                                })
                        })
                    });
                    if !closed {
                        let line_start = parsed[..range.start].rfind('\n').map_or(0, |p| p + 1);
                        return Some((line_start, marker, width));
                    }
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    None
}

/// pulldown-cmark 0.13.4 scans block lines by LF and closing whitespace by
/// spaces. Normalize lone CR and trailing tabs only for parsing. Replacing
/// these ASCII bytes preserves source offsets and leaves emitted text intact.
fn report_parser_input(body: &str) -> Cow<'_, str> {
    if !body.contains(['\r', '\t']) {
        return Cow::Borrowed(body);
    }
    let mut bytes = body.as_bytes().to_vec();
    for i in 0..bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
            bytes[i] = b'\n';
        }
    }
    let mut trailing = true;
    for byte in bytes.iter_mut().rev() {
        match *byte {
            b'\n' | b'\r' => trailing = true,
            b'\t' if trailing => *byte = b' ',
            b' ' => {}
            _ => trailing = false,
        }
    }
    Cow::Owned(String::from_utf8(bytes).expect("ASCII replacements preserve UTF-8"))
}

/// Return the character and width of a leading 3+ backtick or tilde run.
/// This lexical check does not validate indentation or info strings.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let c = trimmed.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|&x| x == c).count();
    (run >= 3).then_some((c, run))
}

/// Deepen ATX and setext headings, clamping at h6.
/// Convert setext to ATX; preserve frontmatter and conservative fence content.
pub(crate) fn shift_headings(markdown: &str, levels: usize) -> String {
    if levels == 0 {
        return markdown.to_owned();
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let body_start = frontmatter_len(&lines);
    let mut fence: Option<(char, usize)> = None;
    let mut out = String::with_capacity(markdown.len() + levels * 40);
    let mut first = true;
    let mut skip_underline = false;

    for (i, line) in lines.iter().enumerate() {
        if skip_underline {
            skip_underline = false;
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;

        if i < body_start {
            out.push_str(line);
            continue;
        }

        if track_fence(&mut fence, line) {
            out.push_str(line);
            continue;
        }

        let trimmed = line.trim_start();

        let setext = lines
            .get(i + 1)
            .and_then(|next| setext_heading_level(line, next));
        if let Some(orig_level) = setext {
            let new_level = (orig_level + levels).min(6);
            out.push_str(&"######"[..new_level]);
            out.push(' ');
            out.push_str(line.trim());
            skip_underline = true;
        } else if let Some(orig_hashes) = atx_heading_level(trimmed) {
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
    use crate::yaml::{ReportBody, finish_report_body};

    /// [T-MD038] Literal completion controls: widths, decoy closes, indentation,
    /// invalid info, CR/CRLF and trailing tabs.
    #[test]
    fn report_body_preserves_body_and_adds_only_a_valid_close() {
        for (body, expected) in [
            ("plain", "plain"),
            ("```", "```\n```"),
            ("```\n", "```\n```"),
            ("~~~", "~~~\n~~~"),
            ("~~~\n", "~~~\n~~~"),
            ("```rust\ncode", "```rust\ncode\n```"),
            ("~~~rust\ncode\n", "~~~rust\ncode\n~~~"),
            ("````\n```\ncode", "````\n```\ncode\n````"),
            ("~~~\n```\ncode", "~~~\n```\ncode\n~~~"),
            ("```\n```rust", "```\n```rust\n```"),
            ("```\n    ```", "```\n    ```\n```"),
            ("    ```\ncode", "    ```\ncode"),
            ("\t~~~\ncode", "\t~~~\ncode"),
            ("\u{a0}```\ncode", "\u{a0}```\ncode"),
            ("```bad`info\ncode", "```bad`info\ncode"),
            ("   ~~~~\ncode\n ~~~~~\t", "   ~~~~\ncode\n ~~~~~\t"),
            ("```\r\ncode\r\n```\r\n", "```\r\ncode\r\n```\r\n"),
            ("```\rcode\r```", "```\rcode\r```"),
            ("```\ncode\n```\t\nprose", "```\ncode\n```\t\nprose"),
        ] {
            assert_eq!(
                finish_report_body(body, ReportBody::Fetched),
                expected,
                "body: {body:?}"
            );
        }
    }

    /// [T-MD001] escape_md_link brackets and parens
    #[test]
    fn escapes_special_chars() {
        assert_eq!(escape_md_link("normal text"), "normal text");
        assert_eq!(escape_md_link("a[b]c(d)e"), r"a\[b\]c\(d\)e");
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

    /// [T-MD021] Setext h1/h2 become ATX h3/h4 without underline lines.
    #[test]
    fn shift_headings_converts_setext_to_atx() {
        let input = "Title\n=====\n\nBody\n\nSection\n-------\n\nmore";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "### Title\n\nBody\n\n#### Section\n\nmore",
            "setext h1/h2 must shift to ATX h3/h4 with the underline consumed"
        );
    }

    /// [T-MD022] Dashes after a blank line remain a thematic break.
    #[test]
    fn shift_headings_leaves_thematic_break_alone() {
        let input = "Para\n\n---\n\nNext";
        assert_eq!(shift_headings(input, 2), input);
    }

    /// [T-MD023] list items, quotes and table rows above dashes are not headings
    #[test]
    fn shift_headings_leaves_non_paragraph_lines_above_dashes() {
        for input in [
            "- item\n---",
            "> quote\n---",
            "| a | b |\n|---|---|",
            "# Already ATX\n---",
        ] {
            let out = shift_headings(input, 2);
            assert!(
                !out.contains("#### "),
                "no setext h2 should be produced for {input:?}, got: {out}"
            );
        }
    }

    /// [T-MD024] setext underlines inside a fenced block are left as content
    #[test]
    fn shift_headings_ignores_setext_inside_code_fence() {
        let input = "```\nTitle\n=====\n```\n# Real";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "```\nTitle\n=====\n```\n### Real",
            "fenced content must survive untouched"
        );
    }

    /// [T-MD025] a setext heading shifted past h6 clamps like an ATX one
    #[test]
    fn shift_headings_setext_clamps_at_h6() {
        let input = "Deep\n----";
        assert_eq!(shift_headings(input, 5), "###### Deep");
    }

    /// [T-MD026] Preserve frontmatter delimiters and fields while shifting the body.
    #[test]
    fn shift_headings_leaves_frontmatter_intact() {
        let input = "---\ntitle: \"T\"\nauthor: \"Jane\"\n---\n\nBody\n\n# Heading";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "---\ntitle: \"T\"\nauthor: \"Jane\"\n---\n\nBody\n\n### Heading",
            "frontmatter keys are not headings and its closing --- is not an underline"
        );
    }

    /// [T-MD027] An unclosed leading `---` does not prevent body heading shifts.
    #[test]
    fn shift_headings_unterminated_frontmatter_still_shifts() {
        let input = "---\n\n# Heading";
        assert_eq!(shift_headings(input, 2), "---\n\n### Heading");
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

    /// [T-MD020] Byte 100 lies inside a three-byte character; retain 99 bytes.
    #[test]
    fn truncate_with_note_cuts_on_a_char_boundary() {
        let input = "あ".repeat(50);
        let result = truncate_with_note(&input, 100);
        assert!(
            result.contains("showing 99 / 150 bytes"),
            "cut must snap to the boundary below 100, got: {result}"
        );
    }

    /// [T-MD036] Keep 16 complete six-byte lines under a 100-byte cap.
    #[test]
    fn truncate_with_note_with_newlines_cuts_at_line_boundary_leaving_no_partial_line() {
        let line = "01234\n";
        let input = line.repeat(20);
        let result = truncate_with_note(&input, 100);
        assert!(
            result.contains("showing 96 / 120 bytes"),
            "cut must back up to the last newline before byte 100, got: {result}"
        );
        let content = result.split("\n\n(truncated").next().unwrap();
        assert_eq!(content, &input[..96]);
        assert!(
            content.ends_with('\n'),
            "must not leave a partial line: {content:?}"
        );
    }

    /// [T-MD037] Drop the dash line rather than cut it into a bare `---`.
    #[test]
    fn truncate_with_note_cutting_mid_dash_run_does_not_leave_a_lone_thematic_break() {
        let lines = "01234\n".repeat(15);
        let input = format!("{lines}-------\n");
        let result = truncate_with_note(&input, 93);
        let content = result.split("\n\n(truncated").next().unwrap();
        assert!(
            !content.ends_with("---"),
            "truncated content must not end in a bare --- line: {content:?}"
        );
        assert_eq!(content, &input[..90]);
    }

    /// [T-MD028]
    #[test]
    fn fence_delimiter_returns_five_backticks_for_content_with_longest_run_of_four() {
        let content = "some ```` text";
        let delim = fence_delimiter(content);
        assert_eq!(delim, "`".repeat(5));
    }

    /// [T-MD029]
    #[test]
    fn fence_delimiter_returns_three_backticks_for_content_without_backticks() {
        assert_eq!(fence_delimiter("plain content, no backticks here"), "```");
    }

    /// [T-MD030]
    #[test]
    fn shift_headings_does_not_close_four_backtick_fence_on_shorter_backtick_run() {
        let input = "````\n```\ncontent\n````\n## After";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "````\n```\ncontent\n````\n#### After",
            "the fence should close only at the matching 4-backtick line, so \
             '## After' sits outside the fence and must shift, got: {result}"
        );
    }

    /// [T-MD031]
    #[test]
    fn shift_headings_leaves_heading_syntax_line_inside_four_backtick_fence_unshifted() {
        let input = "````\n```\n## Not a heading\n````";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, input,
            "the heading-syntax line remains inside the still-open fence and \
             must not shift, got: {result}"
        );
    }

    /// [T-MD032]
    #[test]
    fn fence_marker_recognizes_three_backticks_as_fence_start() {
        assert_eq!(fence_marker("```"), Some(('`', 3)));
    }

    /// [T-MD033]
    #[test]
    fn fence_marker_does_not_recognize_two_backticks_as_fence_start() {
        assert_eq!(fence_marker("``"), None);
    }

    /// [T-MD034]
    #[test]
    fn fence_marker_recognizes_three_tildes_as_fence_start() {
        assert_eq!(fence_marker("~~~"), Some(('~', 3)));
    }

    /// [T-MD035] Observe conservative indented-fence tracking through heading shifts.
    #[test]
    fn fence_marker_recognizes_four_space_indented_fence_line() {
        let shifted = shift_headings("    ```\n# not a heading\n    ```\n# heading\n", 1);

        assert!(
            shifted.contains("\n# not a heading\n"),
            "a line inside an indented fence must keep its level:\n{shifted}"
        );
        assert!(
            shifted.contains("\n## heading"),
            "a heading past the closed indented fence must still shift:\n{shifted}"
        );
    }
}
