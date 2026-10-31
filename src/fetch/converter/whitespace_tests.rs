use super::test_helpers::article;
use super::*;

/// [T-FC041] Ordinary paragraph newlines fold to spaces.
#[test]
fn a_newline_inside_a_paragraph_collapses_to_a_single_space() {
    let article = article("<p>line1\nline2</p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("line1 line2"),
        "a newline inside a paragraph's text must collapse to a single space:\n{markdown}"
    );
    assert!(
        !markdown.contains("line1\nline2"),
        "the source newline must not survive as a literal line break:\n{markdown}"
    );
}

/// [T-FC042] A paragraph br must retain the default Markdown hard break.
#[test]
fn br_inside_a_paragraph_survives_as_two_trailing_spaces_and_a_newline() {
    let article = article("<p>line1<br>line2</p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("line1  \nline2"),
        "a <br> inside a paragraph must leave two trailing spaces before the newline it \
         introduces:\n{markdown}"
    );
}

/// [T-FC043] Bare-pre source newlines must survive inside the fence.
#[test]
fn pre_content_is_not_collapsed_and_keeps_its_newline() {
    let article = article("<pre>line1\nline2</pre>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("```\nline1\nline2\n```"),
        "a <pre> block's internal newline must survive as a real line break, not collapse \
         to a space:\n{markdown}"
    );
}

/// [T-FC044] Inline code must not collapse consecutive spaces.
#[test]
fn consecutive_spaces_inside_inline_code_survive_unchanged() {
    let article = article("<p>a <code>x   y</code> b</p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("`x   y`"),
        "three consecutive spaces inside inline code must survive without collapsing to a \
         single space:\n{markdown}"
    );
}

/// [T-FC045] A cell br must not introduce a row-breaking newline.
#[test]
fn br_inside_a_table_cell_loses_the_line_break_and_collapses_to_whitespace() {
    let article = article("<table><tr><td>line1<br>line2</td></tr></table>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("| line1   line2 |"),
        "a <br> inside a table cell must collapse to a run of spaces between the text on \
         either side of it, not survive as a line break:\n{markdown}"
    );
    assert!(
        !markdown.contains("line1  \nline2"),
        "the hard-break form a <br> takes in a paragraph or <pre> must not survive inside \
         a table cell, which cannot hold a literal newline:\n{markdown}"
    );
}

/// [T-FC046] List-item br output retains four-space indentation and trims trailing spaces.
#[test]
fn br_inside_a_list_item_loses_its_trailing_spaces_and_becomes_an_indented_newline() {
    let article = article("<ul><li>line1<br>line2</li></ul>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("line1\n    line2"),
        "a <br> inside a list item must leave no trailing spaces on the line before it, \
         and the text after it must reappear indented under the bullet on its own \
         line:\n{markdown}"
    );
    assert!(
        !markdown.contains("line1  \n"),
        "the hard-break form a <br> takes in a paragraph must not survive inside a list \
         item, where the indent step trims it away:\n{markdown}"
    );
}

/// [T-FC047] A heading br leaves the next line unmarked.
#[test]
fn text_after_a_br_inside_a_heading_lands_outside_the_heading_line() {
    let article = article("<h2>line1<br>line2</h2>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("## line1  \nline2"),
        "text after a <br> inside a heading must land on its own line below the '#' \
         marker, carrying the same hard-break form a <br> produces inside a paragraph:\n\
         {markdown}"
    );
    assert!(
        !markdown
            .lines()
            .any(|line| line.starts_with('#') && line.contains("line2")),
        "the text after a <br> inside a heading must not end up inside the heading's own \
         '#'-prefixed line:\n{markdown}"
    );
}
