use super::test_helpers::article;
use super::*;

/// [T-FC026] Parentheses in a destination must not close the Markdown link early.
#[test]
fn link_target_with_parens_is_not_cut_off_before_the_parenthesis() {
    let article = article(r#"<p><a href="https://example.com/wiki/Foo_(bar)">Foo</a></p>"#);

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains(r"[Foo](https://example.com/wiki/Foo_\(bar\))"),
        "the link destination must carry the full URL past the parenthesis, \
         not truncate at it:\n{markdown}"
    );
}

/// [T-FC048] Empty per-line highlighter anchors must not add links to code.
#[test]
fn empty_anchor_inside_pre_disappears_leaving_the_original_line_and_indentation() {
    let article = article(
        "<pre><a href=\"#__codelineno-0-1\"></a>    def foo():\n\
         <a href=\"#__codelineno-0-2\"></a>        return 1\n</pre>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        !markdown.contains("__codelineno"),
        "an empty anchor pointing only at a fragment must leave no trace of its href:\n{markdown}"
    );
    assert!(
        markdown.lines().any(|line| line == "    def foo():"),
        "the first original line and its indentation must survive with the anchor removed:\n{markdown}"
    );
    assert!(
        markdown.lines().any(|line| line == "        return 1"),
        "the second original line and its indentation must survive with the anchor removed:\n{markdown}"
    );
}

/// [T-FC049] A fragment-anchor control distinguishes selective suppression
/// from dropping every empty anchor, including absolute links with titles.
#[test]
fn empty_anchor_to_absolute_url_keeps_destination_and_title() {
    let article = article(
        "<p>See <a href=\"https://example.com/target\" title=\"Target page\"></a> \
         and <a href=\"#top\"></a> for details.</p>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[](https://example.com/target \"Target page\")"),
        "an empty anchor pointing at an absolute URL must keep its destination and title:\n{markdown}"
    );
    assert!(
        !markdown.contains("#top"),
        "the fragment-only sibling anchor must be suppressed, not just the absolute one kept:\n{markdown}"
    );
}

/// [T-FC050]
#[test]
fn titled_headerlink_with_empty_content_is_removed() {
    let article = article(
        "<h2>Section<a class=\"headerlink\" href=\"#section\" \
         title=\"Link to this heading\"></a></h2>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("## Section"),
        "the heading text must survive with the headerlink removed:\n{markdown}"
    );
    assert!(
        !markdown.contains("Link to this heading"),
        "the headerlink's title must not survive:\n{markdown}"
    );
    assert!(
        !markdown.contains("#section"),
        "the headerlink's href must not survive:\n{markdown}"
    );
}

/// [T-FC051] Text in a missing-href anchor survives without link brackets;
/// the empty fragment-only sibling is suppressed.
#[test]
fn anchor_without_href_delegates_to_the_builtin_handler() {
    let article = article("<p><a>plain text</a> and <a href=\"#nav\"></a></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("plain text") && !markdown.contains("[plain text]"),
        "an <a> with no href must fall through to the builtin handler's walk-children \
         behavior, not gain link brackets:\n{markdown}"
    );
    assert!(
        !markdown.contains("#nav"),
        "the fragment-only sibling anchor must be suppressed, not just the hrefless one delegated:\n{markdown}"
    );
}

/// [T-FC073] Delegated nonempty links must not retain redundant title suffixes.
#[test]
fn title_disappears_from_the_output_of_a_link_that_has_link_text() {
    let article =
        article("<p><a href=\"https://example.com/target\" title=\"My Title\">link text</a></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[link text](https://example.com/target)"),
        "a titled link with link text must lose its title, leaving a bare \
         `[text](url)`:\n{markdown}"
    );
    assert!(
        !markdown.contains("My Title"),
        "the title text must not survive anywhere in the output:\n{markdown}"
    );
}

/// [T-FC074] Escaped quotes in a delegated title must not defeat suffix removal.
#[test]
fn title_containing_double_quotes_does_not_escape_the_rewrite() {
    let article = article(
        "<p><a href=\"https://example.com/target\" title=\"say &quot;hi&quot;\">\
         link text</a></p>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[link text](https://example.com/target)"),
        "a title holding double quotes must still be dropped in full, leaving a bare \
         `[text](url)`:\n{markdown}"
    );
    assert!(
        !markdown.contains("hi"),
        "no fragment of the quoted title text must survive:\n{markdown}"
    );
}

/// [T-FC075] Title reflow must not defeat suffix matching across lines.
#[test]
fn title_containing_a_newline_does_not_escape_the_rewrite() {
    let article = article(
        "<p><a href=\"https://example.com/target\" title=\"line1\nline2\">\
         link text</a></p>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[link text](https://example.com/target)"),
        "a title holding a newline must still be dropped in full, leaving a bare \
         `[text](url)`:\n{markdown}"
    );
    assert!(
        !markdown.contains("line1") && !markdown.contains("line2"),
        "no fragment of the multi-line title text must survive:\n{markdown}"
    );
}

/// [T-FC076] Whitespace-only titles still produce an empty suffix in htmd;
/// that suffix must be removed from nonempty links.
#[test]
fn whitespace_only_title_is_treated_as_no_title() {
    let article =
        article("<p><a href=\"https://example.com/target\" title=\"   \">link text</a></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[link text](https://example.com/target)"),
        "a whitespace-only title must leave a bare `[text](url)` with no empty title \
         syntax:\n{markdown}"
    );
    assert!(
        !markdown.contains("\"\""),
        "no empty title marker must survive:\n{markdown}"
    );
}

/// [T-FC077] Empty href resolves to the same page and must not emit an empty link.
#[test]
fn anchor_with_an_empty_href_and_no_content_is_suppressed() {
    let article = article("<p><a href=\"\" title=\"here\"></a>tail</p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        !markdown.contains("[]("),
        "an empty-href anchor with no content must be suppressed, not emitted as an \
         empty link carrying its title:\n{markdown}"
    );
    assert!(
        markdown.contains("tail"),
        "the text after the suppressed anchor must survive:\n{markdown}"
    );
}
