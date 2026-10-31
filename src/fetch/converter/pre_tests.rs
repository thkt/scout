use super::test_helpers::article;
use super::*;

/// [T-FC015] Code-block text must not acquire ordinary Markdown backslash escapes.
#[test]
fn pre_code_escape_target_chars_are_not_backslash_escaped() {
    let article = article(r#"<pre><code>\ * _ ` [ ] end</code></pre>"#);

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains(r"\ * _ ` [ ] end"),
        "the six escape-target characters must survive unescaped inside a code block:\n{}",
        result.markdown()
    );
}

/// [T-FC016] Content backticks must not close the surrounding code fence.
#[test]
fn code_block_with_three_backticks_widens_fence_to_four() {
    let article = article("<pre><code>a ``` b</code></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("````\na ``` b\n````"),
        "a 3-backtick run in the content must widen the fence to 4 backticks:\n{}",
        result.markdown()
    );
}

/// [T-FC017] A language class supplies the fence info string. This fixture
/// bypasses Readability; normal extraction strips classes (see `article`).
#[test]
fn code_block_with_language_class_gets_language_info_string() {
    let article = article(r#"<pre><code class="language-rust">fn main() {}</code></pre>"#);

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("```rust\nfn main() {}\n```"),
        "a `language-rust` class must attach `rust` as the fence info string:\n{}",
        result.markdown()
    );
}

/// [T-FC019] Bare pre content needs a fence even without a code child.
#[test]
fn pre_without_code_child_is_wrapped_in_a_fence() {
    let article = article("<pre>plain text</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("```\nplain text\n```"),
        "a <pre> with no <code> child must be wrapped in a fenced code block:\n{}",
        result.markdown()
    );
}

/// [T-FC020] Pre/code outside table cells must retain a single fenced block.
#[test]
fn pre_code_is_fenced_exactly_once() {
    let article = article("<pre><code>fn main() {}</code></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("```\nfn main() {}\n```"),
        "the pre>code block must still be fenced once:\n{}",
        result.markdown()
    );
    assert_eq!(
        result.markdown().matches("```").count(),
        2,
        "an already-fenced pre>code block must not gain a second fence:\n{}",
        result.markdown()
    );
}

/// [T-FC021] A leading backtick in bare-pre text survives unescaped inside the
/// fence, also when a comment node precedes the text.
#[test]
fn leading_backtick_of_bare_pre_text_survives_unescaped_inside_the_fence() {
    for html in ["<pre>`hello</pre>", "<pre><!-- c -->`hello</pre>"] {
        let article = article(html);

        let result = to_fetch_result(&article, "https://example.com".into(), false);
        let markdown = result.markdown();

        assert!(
            markdown.contains("```\n`hello\n```"),
            "the leading backtick must survive unescaped inside the fence for {html}:\n{markdown}"
        );
        assert!(
            !markdown.contains("\\`hello"),
            "no backslash may precede the leading backtick for {html}:\n{markdown}"
        );
    }
}

/// [T-FC022] Source backslashes inside pre content must survive unchanged.
#[test]
fn literal_backslash_backtick_pair_mid_content_survives_unstripped() {
    let article = article("<pre>abc\n\\` def</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("```\nabc\n\\` def\n```"),
        "a literal backslash-backtick pair not at the content's head must survive as written:\n{}",
        result.markdown()
    );
}

/// [T-FC024] A nested pre must stay inside its list item.
#[test]
fn li_pre_stays_in_the_same_item_as_the_list_marker() {
    let article = article("<ul><li>intro<pre><code>line1\nline2</code></pre></li></ul>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    let marker_line = markdown
        .lines()
        .find(|line| line.contains("intro"))
        .expect("the list marker line must carry the li's leading text");
    assert!(
        marker_line.trim_start().starts_with(['-', '*']),
        "the li's leading text must carry a list marker:\n{markdown}"
    );

    let fence_body_lines: Vec<&str> = markdown
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed == "```" || trimmed == "line1" || trimmed == "line2"
        })
        .collect();
    assert_eq!(
        fence_body_lines.len(),
        4,
        "expected two fence delimiters and two content lines:\n{markdown}"
    );
    for line in fence_body_lines {
        assert!(
            line.starts_with(' '),
            "a <pre> block inside <li> must stay indented under the list marker, \
             not break out as a column-0 block: {line:?}\n{markdown}"
        );
    }
}

/// [T-FC027] A source backslash before a leading backtick is literal content.
#[test]
fn source_leading_backslash_backtick_pair_survives_unstripped() {
    let article = article("<pre>\\`hello</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("```\n\\`hello\n```"),
        "a source backslash before the leading backtick must survive as written:\n{}",
        result.markdown()
    );
}

/// [T-FC028] Highlighted bare-pre text starting with backticks still needs fencing.
#[test]
fn pre_with_nested_inline_element_is_wrapped_in_a_fence() {
    let article = article("<pre><span>`x`</span></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(
        result.markdown().contains("```\n`x`\n```"),
        "a <pre> whose fence-leading text comes from a nested element must still be fenced:\n{}",
        result.markdown()
    );
}

/// [T-FC034] A backtick-led pre text node after an element child survives
/// unescaped when text precedes that element; [T-FC035] the same holds when
/// the element is the first child.
#[test]
fn backtick_led_pre_text_after_an_element_child_survives_unescaped() {
    for (html, fenced, escaped) in [
        (
            "<pre>abc<span>X</span>`def</pre>",
            "```\nabcX`def\n```",
            r"abcX\`def",
        ),
        (
            "<pre><span>abc</span>`def</pre>",
            "```\nabc`def\n```",
            r"abc\`def",
        ),
    ] {
        let article = article(html);

        let result = to_fetch_result(&article, "https://example.com".into(), false);
        let markdown = result.markdown();

        assert!(
            markdown.contains(fenced),
            "the backtick after the element child must survive unescaped inside the \
             fence for {html}:\n{markdown}"
        );
        assert!(
            !markdown.contains(escaped),
            "no backslash may precede the backtick after the element child for \
             {html}:\n{markdown}"
        );
    }
}

/// [T-FC036] DOM-based text preservation must hold through nested pre handling.
#[test]
fn inner_pre_backtick_survives_unstripped_when_pre_is_nested() {
    let article = article("<pre><pre>abc<span>X</span>`def</pre></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("abcX`def"),
        "the inner pre's second text node's leading backtick must survive unescaped:\n{markdown}"
    );
    assert!(
        !markdown.contains(r"abcX\`def"),
        "htmd's escape on the inner pre's second text node must not survive as a literal \
         backslash:\n{markdown}"
    );
}

/// [T-FC037] Per-line spans inside pre keep each line on its own output line
/// when the spans share attributes; [T-FC053] the same holds when each span
/// carries a distinct `data-line` value.
#[test]
fn per_line_spans_inside_pre_keep_each_line_on_its_own_output_line() {
    for html in [
        "<pre><span>line1\n</span><span>line2\n</span><span>line3</span></pre>",
        "<pre><span data-line=\"1\">line1\n</span>\
         <span data-line=\"2\">line2\n</span>\
         <span data-line=\"3\">line3</span></pre>",
    ] {
        let article = article(html);

        let result = to_fetch_result(&article, "https://example.com".into(), false);
        let markdown = result.markdown();

        assert!(
            markdown.contains("line1\nline2\nline3"),
            "each line-span's content must land on its own output line, in order, \
             for {html}:\n{markdown}"
        );
    }
}

/// [T-FC038] Adjacent block children must not accumulate extra blank lines.
#[test]
fn adjacent_block_children_boundary_keeps_newlines_capped_at_two() {
    let article = article("<pre><div>ALPHA</div><div>BETA</div></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    let alpha_end = markdown
        .find("ALPHA")
        .expect("first block child's text must be present")
        + "ALPHA".len();
    let beta_start = markdown
        .find("BETA")
        .expect("second block child's text must be present");
    let between = &markdown[alpha_end..beta_start];
    let newline_run = between.chars().filter(|&c| c == '\n').count();

    assert!(
        newline_run <= 2,
        "the boundary between adjacent block children must carry at most 2 newlines, \
         got {newline_run}:\n{markdown}"
    );
}

/// [T-FC039] A direct pre child br must retain its Markdown hard break.
#[test]
fn br_directly_under_pre_survives_as_two_trailing_spaces_and_a_newline() {
    let article = article("<pre>line1<br>line2</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("line1  \nline2"),
        "a <br> directly under <pre> must leave two trailing spaces before the newline \
         it introduces:\n{markdown}"
    );
}

/// [T-FC040] Unregistered child tags must still preserve pre text without escaping.
#[test]
fn unregistered_tag_child_text_inside_pre_is_not_escaped() {
    let article = article("<pre><mark>a_b*c</mark></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("a_b*c"),
        "an unregistered tag's text content inside <pre> must survive without \
         escape-target characters gaining a backslash:\n{markdown}"
    );
}

/// [T-FC052] Span-edge newlines must separate following pre text.
#[test]
fn trailing_newline_at_the_end_of_a_span_inside_pre_survives_in_the_output() {
    let article = article("<pre><span>line1\n</span>line2</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("line1\nline2"),
        "the span's trailing newline must reach the sibling text as a real line break:\n{markdown}"
    );
}

/// [T-FC054] Inline-code spans fold newlines to spaces.
/// The newline must be inside the span to exercise that handler.
#[test]
fn span_inside_inline_code_outside_pre_folds_the_newline_to_a_space() {
    let article = article("<p><code><span>line1\n</span>line2</code></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("`line1 line2`"),
        "a span inside inline code with no <pre> ancestor must preserve its newline as a space, preserving the newline for the code handler to fold \
         to a space:\n{markdown}"
    );
    assert!(
        !markdown.contains("line1\nline2"),
        "the newline must not survive raw:\n{markdown}"
    );
}

/// [T-FC055] Pre spans must still convert nested elements, not copy raw markup.
#[test]
fn pre_newline_survives_when_the_neighboring_span_has_an_element_child() {
    let article = article("<pre><span>line1\n</span><span><b>line2</b></span></pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("line1\n**line2**"),
        "the newline before a line-span with an element child must survive, and that \
         child element must still be converted to Markdown:\n{markdown}"
    );
}

/// [T-FC083] Bare-pre fencing must use a delimiter wider than content backticks.
#[test]
fn pre_without_code_child_widens_its_fence_past_a_backtick_run_in_the_content() {
    let article = article("<pre>a ``` b</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("````\na ``` b\n````"),
        "a <pre> whose content holds a 3-backtick run must be fenced with 4:\n{markdown}"
    );
}
