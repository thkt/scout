use std::borrow::Cow;

/// Escape characters that break Markdown link syntax, folding newlines to
/// spaces so an untrusted value cannot inject block Markdown.
///
/// `|` is absent: a URL inside `[](…)` has no table column to break out of.
/// Text that is not a link target wants [`escape_md_inline`], which does escape
/// it, so this stays private and reachable only through [`md_link`].
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

/// Render a Markdown link `[text](url)` only when `url` carries an http/https
/// scheme, emitting anything else as inert `text (url)`.
///
/// The scheme check is an allowlist rather than a `javascript:`/`data:` denylist,
/// so an obfuscated or whitespace-prefixed scheme fails to match and lands in
/// the inert branch instead of slipping past a pattern nobody listed.
pub(crate) fn md_link(text: &str, url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("http://") || lower.starts_with("https://");
    // A real URL carries no ASCII whitespace or control chars, and a raw newline
    // here would break out of `[](…)`.
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

/// Replace newlines, which would break heading structure, with spaces.
///
/// Returns the input borrowed when it carries no newline. Headings are titles
/// and URLs, so that is the common path and it allocates nothing.
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

/// Truncate a string at a line boundary and append a byte-count note.
///
/// The cut lands on a line boundary, not the byte cap: cutting mid-line can
/// leave a partial line that reads as syntax it was not, such as `-------`
/// becoming a bare `---` that the appended note then terminates. Falling back
/// to the byte cap happens only when no newline precedes it, and there the
/// line starts at offset 0, so no partial line can form.
///
/// Returns the input borrowed if it fits within `max_bytes`.
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

/// Number of leading lines occupied by a `---`-delimited frontmatter block, or
/// 0 when the input does not open with one.
///
/// scout's own fetch output carries this block (ADR-0014), and its lines are not
/// content: `author: "Jane"` above the closing `---` reads as a setext h2 by the
/// CommonMark rule, which would rewrite the key as a heading and consume the
/// delimiter that closes the block. An unterminated opening `---` yields 0, so a
/// body that merely starts with a thematic break is still shifted.
fn frontmatter_len(lines: &[&str]) -> usize {
    if lines.first() != Some(&"---") {
        return 0;
    }
    lines[1..]
        .iter()
        .position(|l| *l == "---")
        .map_or(0, |p| p + 2)
}

/// Return the setext heading level if `text` followed by `underline` forms one
/// (CommonMark §4.3): `=` underlines an h1, `-` an h2.
///
/// `-` is also a thematic break and a list bullet, and what separates them is
/// the line the underline sits under: a setext underline follows a paragraph
/// line, a thematic break follows a blank one. Hence the check on `text`.
fn setext_heading_level(text: &str, underline: &str) -> Option<usize> {
    let text = text.trim();
    if text.is_empty() || text.starts_with(['#', '-', '*', '+', '>', '|', '=']) {
        return None;
    }
    if text.starts_with("```") || text.starts_with("~~~") {
        return None;
    }
    let underline = underline.trim_end();
    // Column 0 only, matching `atx_heading_level`'s treatment of the marker: an
    // indented underline is a code block or list continuation, not a heading.
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

/// Advance `fence` across one line and report whether that line is
/// fence-protected: inside a fenced code block, or the delimiter line itself.
///
/// A fence closes only at a line whose run of the same character is at least
/// as long as the one that opened it (CommonMark §4.5), so a 4-backtick fence
/// stays open through a nested 3-backtick line.
///
/// Callers must not pre-trim. The trim happens here, and an indented delimiter
/// would otherwise arrive with the caller's indent handling already applied.
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

/// Return the fence character and run length if `trimmed` opens or closes a
/// fenced code block (CommonMark §4.5: a run of 3+ backticks or tildes).
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let c = trimmed.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|&x| x == c).count();
    (run >= 3).then_some((c, run))
}

/// Shift all Markdown heading levels deeper by `levels` (e.g., `# Foo` → `#### Foo`
/// with `levels = 3`).  Clamps output at h6 (CommonMark maximum).
///
/// Only valid ATX headings (CommonMark §4.2: 1–6 `#` + space/tab/EOL) are
/// shifted; lines like `#include` or `#123` are left unchanged.
///
/// A setext heading (`Title` over `=====`) is rewritten to its ATX equivalent
/// before being shifted, because past h2 there is no setext form to shift into.
/// The underline line is consumed. Some READMEs are written almost entirely in
/// setext, so skipping the form would drop a whole document's structure a level
/// below the `## README` it sits under.
///
/// Skips lines inside fenced code blocks so that comment lines like `# TODO`
/// are not affected.  The fence tracks the opening marker's character and
/// length (CommonMark §4.5): a fence only closes at a line whose run of the
/// same character is at least as long as the one that opened it, so a
/// 4-backtick fence stays open through a nested 3-backtick line.
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

    /// [T-MD001] escape_md_link brackets and parens
    #[test]
    fn escapes_special_chars() {
        assert_eq!(escape_md_link("normal text"), "normal text");
        assert_eq!(escape_md_link("a[b]c(d)e"), r"a\[b\]c\(d\)e");
        // Left as-is, the newline would open a new Markdown line inside the link.
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

    /// [T-MD021] setext headings are rewritten to ATX and shifted
    ///
    /// `=` underlines an h1 and `-` an h2 (CommonMark §4.3). Past h2 there is no
    /// setext form to shift into, so both become ATX and the underline is
    /// consumed.
    #[test]
    fn shift_headings_converts_setext_to_atx() {
        let input = "Title\n=====\n\nBody\n\nSection\n-------\n\nmore";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "### Title\n\nBody\n\n#### Section\n\nmore",
            "setext h1/h2 must shift to ATX h3/h4 with the underline consumed"
        );
    }

    /// [T-MD022] a `---` that follows a blank line stays a thematic break
    ///
    /// The rule separating a setext underline from a thematic break is what sits
    /// above it. Without this case, widening the underline test would silently
    /// turn every horizontal rule in a README into a heading.
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

    /// [T-MD026] a leading frontmatter block survives the shift intact
    ///
    /// Regression: when setext support landed, the closing `---` read as an
    /// underline for the `author:` line above it, so the key became a heading and
    /// the delimiter that closes the block disappeared. `tests/output_injection.rs`
    /// caught it end-to-end; this pins the same thing at the function boundary.
    #[test]
    fn shift_headings_leaves_frontmatter_intact() {
        let input = "---\ntitle: \"T\"\nauthor: \"Jane\"\n---\n\nBody\n\n# Heading";
        let result = shift_headings(input, 2);
        assert_eq!(
            result, "---\ntitle: \"T\"\nauthor: \"Jane\"\n---\n\nBody\n\n### Heading",
            "frontmatter keys are not headings and its closing --- is not an underline"
        );
    }

    /// [T-MD027] an unterminated leading `---` is body content, not frontmatter
    ///
    /// Without the closing delimiter there is no block to protect, so the usual
    /// rules apply and the rest of the document still shifts.
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

    /// [T-MD020] A cap that lands mid-character cuts at the boundary below it
    ///
    /// Every other truncation test feeds ASCII, where every byte index is a char
    /// boundary — so none of them fails if `floor_char_boundary` is dropped for a
    /// plain `&s[..max_bytes]`, which panics on the first multi-byte page scout
    /// fetches. 100 falls inside the 34th 3-byte character, so the cut must land
    /// on 99.
    #[test]
    fn truncate_with_note_cuts_on_a_char_boundary() {
        let input = "あ".repeat(50);
        let result = truncate_with_note(&input, 100);
        assert!(
            result.contains("showing 99 / 150 bytes"),
            "cut must snap to the boundary below 100, got: {result}"
        );
    }

    /// [T-MD036] 改行を含む入力の打ち切りが行境界で切れ、部分行を残さない
    ///
    /// 6 バイトの行を 20 本並べた 120 バイト入力を max_bytes=100 で切ると、
    /// 100 バイト目は 17 本目の行の途中 (先頭から 4 バイト目) に落ちる。行境界へ
    /// 寄せる実装は直前の改行 (95 バイト目) まで戻り、96 バイトで 16 行分を残す。
    #[test]
    fn truncate_with_note_with_newlines_cuts_at_line_boundary_leaving_no_partial_line() {
        let line = "01234\n"; // 6 bytes per line
        let input = line.repeat(20); // 120 bytes, 20 complete lines
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

    /// [T-MD037] "-------" を含む入力を行の途中で切っても出力の末尾行が --- にならない
    ///
    /// 6 バイトの行を 15 本 (90 バイト) 並べたあとに 7 個のダッシュの行を続け、
    /// max_bytes=93 で切る。素朴な floor_char_boundary(93) はダッシュ行の 3 バイト
    /// 目 (`---`) で止まり、その 3 文字だけの行が Markdown の thematic break として
    /// 独立して解釈されてしまう。行境界へ寄せる実装は 90 バイト目 (15 行分) まで戻り、
    /// ダッシュ行自体を丸ごと落とす。
    #[test]
    fn truncate_with_note_cutting_mid_dash_run_does_not_leave_a_lone_thematic_break() {
        let lines = "01234\n".repeat(15); // 90 bytes, 15 complete lines
        let input = format!("{lines}-------\n"); // + a 7-dash line, 98 bytes total
        let result = truncate_with_note(&input, 93);
        let content = result.split("\n\n(truncated").next().unwrap();
        assert!(
            !content.ends_with("---"),
            "truncated content must not end in a bare --- line: {content:?}"
        );
        assert_eq!(content, &input[..90]);
    }

    /// [T-MD028] 最長 4 個のバッククォート列を含む内容に対して区切りは 5 個になる
    #[test]
    fn fence_delimiter_returns_five_backticks_for_content_with_longest_run_of_four() {
        let content = "some ```` text";
        let delim = fence_delimiter(content);
        assert_eq!(delim, "`".repeat(5));
    }

    /// [T-MD029] バッククォートを含まない内容に対して区切りは 3 個になる
    #[test]
    fn fence_delimiter_returns_three_backticks_for_content_without_backticks() {
        assert_eq!(fence_delimiter("plain content, no backticks here"), "```");
    }

    /// [T-MD030] 4 個のフェンスで囲まれた中の 3 個のバッククォート行が閉じ扱いされない
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

    /// [T-MD031] 4 個のフェンスの中にある見出し記法の行が見出しとして繰り下げられない
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

    /// [T-MD032] バッククォート 3 個の行がフェンス開始として認識される
    #[test]
    fn fence_marker_recognizes_three_backticks_as_fence_start() {
        assert_eq!(fence_marker("```"), Some(('`', 3)));
    }

    /// [T-MD033] バッククォート 2 個の行はフェンス開始として認識されない
    #[test]
    fn fence_marker_does_not_recognize_two_backticks_as_fence_start() {
        assert_eq!(fence_marker("``"), None);
    }

    /// [T-MD034] チルダ 3 個の行がフェンス開始として認識される
    #[test]
    fn fence_marker_recognizes_three_tildes_as_fence_start() {
        assert_eq!(fence_marker("~~~"), Some(('~', 3)));
    }

    /// [T-MD035] 4 スペースでインデントされたフェンス行もフェンス開始として認識される
    ///
    /// `fence_marker` takes an already-trimmed line, so asserting on it
    /// directly would only restate T-MD032. `track_fence` owns the trim, so
    /// the indent has to be observed through a caller: a `#` line between two
    /// indented fences must keep its level, since it sits inside a code block.
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
