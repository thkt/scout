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
fn anchor_without_href_keeps_its_text_without_link_brackets() {
    let article = article("<p><a>plain text</a> and <a href=\"#nav\"></a></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("plain text") && !markdown.contains("[plain text]"),
        "an <a> with no href must keep its text without gaining link brackets:\n{markdown}"
    );
    assert!(
        !markdown.contains("#nav"),
        "the fragment-only sibling anchor must be suppressed, not just the hrefless one \
         kept as text:\n{markdown}"
    );
}

/// [T-FC073] A link with link text drops its title attribute from the output.
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

/// [T-FC074] Double quotes in an empty anchor's title are backslash-escaped,
/// so they cannot close the Markdown title early.
#[test]
fn empty_anchor_title_backslash_escapes_double_quotes() {
    let article =
        article("<p><a href=\"https://example.com/target\" title=\"say &quot;hi&quot;\"></a></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains(r#"[](https://example.com/target "say \"hi\"")"#),
        "each double quote inside the title must gain a backslash:\n{markdown}"
    );
}

/// [T-FC075] A multi-line title on an empty anchor keeps its line breaks,
/// trims spaces around each line, and drops blank lines.
#[test]
fn empty_anchor_title_trims_each_line_and_drops_blank_lines() {
    let article = article(
        "<p><a href=\"https://example.com/target\" title=\"  line1  \n\n   line2 \"></a></p>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[](https://example.com/target \"line1\nline2\")"),
        "the title must keep one line break between its trimmed, non-blank lines:\n{markdown}"
    );
}

/// [T-FC076] A whitespace-only title on an empty anchor becomes an empty
/// title marker `""` after the destination.
#[test]
fn whitespace_only_title_on_an_empty_anchor_becomes_an_empty_title() {
    let article = article("<p><a href=\"https://example.com/target\" title=\"   \"></a></p>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[](https://example.com/target \"\")"),
        "a whitespace-only title must trim to an empty title, not keep its spaces:\n{markdown}"
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
