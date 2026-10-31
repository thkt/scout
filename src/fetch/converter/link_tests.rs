use super::test_helpers::article;
use super::*;
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

/// [T-FC026]
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

/// [T-FC073]
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

/// [T-FC075]
#[test]
fn empty_anchor_title_folds_trimmed_non_blank_lines_onto_one_line() {
    let article = article(
        "<p><a href=\"https://example.com/target\" title=\"  line1  \n\n   line2 \"></a></p>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("[](https://example.com/target \"line1 line2\")"),
        "the title must join its trimmed, non-blank lines with one space:\n{markdown}"
    );
}

/// [T-FC076]
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

/// [T-FC077]
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

fn body_events(markdown: &str) -> Vec<Event<'_>> {
    let body = markdown
        .strip_prefix("---\n---\n\n")
        .expect("an article without metadata must start with an empty frontmatter");
    Parser::new(body).collect()
}

fn lone_paragraph(element: Tag<'_>) -> Vec<Event<'_>> {
    let end = element.to_end();
    vec![
        Event::Start(Tag::Paragraph),
        Event::Start(element),
        Event::End(end),
        Event::End(TagEnd::Paragraph),
    ]
}

/// A lone CR reaches the attribute only through `&#13;`; html5ever turns a
/// literal CR into LF.
const LINE_BREAK_TITLES: [(&str, &str); 10] = [
    ("a&#10;# Heading", "a # Heading"),
    ("a&#10;```rust", "a ```rust"),
    ("a&#10;~~~", "a ~~~"),
    ("a&#10;> quote", "a > quote"),
    ("a&#10;- item", "a - item"),
    ("a&#10;---&#10;b", "a --- b"),
    ("a&#10;...&#10;b", "a ... b"),
    ("a&#13;# Heading", "a # Heading"),
    ("a&#13;&#10;# Heading", "a # Heading"),
    ("&#10;# Heading&#10;", "# Heading"),
];

const BACKSLASH_TITLES: [(&str, &str); 3] = [
    (r"a\", r"a\"),
    (r"a\&quot;b", r#"a\"b"#),
    (r"a\*b", r"a\*b"),
];

/// [T-FC106]
#[test]
fn image_title_with_a_line_break_cannot_open_a_block() {
    for (attribute, title) in LINE_BREAK_TITLES {
        let article = article(&format!(
            "<p><img src=\"https://example.com/x.png\" title=\"{attribute}\"></p>"
        ));

        let result = to_fetch_result(&article, "https://example.com".into(), false);
        let markdown = result.markdown();

        assert_eq!(
            body_events(markdown),
            lone_paragraph(Tag::Image {
                link_type: LinkType::Inline,
                dest_url: "https://example.com/x.png".into(),
                title: title.into(),
                id: "".into(),
            }),
            "title attribute {attribute:?} must stay inside the image:\n{markdown}"
        );
    }
}

/// [T-FC107]
#[test]
fn empty_anchor_title_with_a_line_break_cannot_open_a_block() {
    for (attribute, title) in LINE_BREAK_TITLES {
        let article = article(&format!(
            "<p><a href=\"https://example.com/target\" title=\"{attribute}\"></a></p>"
        ));

        let result = to_fetch_result(&article, "https://example.com".into(), false);
        let markdown = result.markdown();

        assert_eq!(
            body_events(markdown),
            lone_paragraph(Tag::Link {
                link_type: LinkType::Inline,
                dest_url: "https://example.com/target".into(),
                title: title.into(),
                id: "".into(),
            }),
            "title attribute {attribute:?} must stay inside the link:\n{markdown}"
        );
    }
}

/// [T-FC108]
#[test]
fn title_with_a_backslash_closes_and_reads_back_unchanged() {
    for (attribute, title) in BACKSLASH_TITLES {
        for (html, expected) in [
            (
                format!("<p><img src=\"https://example.com/x.png\" title=\"{attribute}\"></p>"),
                Tag::Image {
                    link_type: LinkType::Inline,
                    dest_url: "https://example.com/x.png".into(),
                    title: title.into(),
                    id: "".into(),
                },
            ),
            (
                format!("<p><a href=\"https://example.com/target\" title=\"{attribute}\"></a></p>"),
                Tag::Link {
                    link_type: LinkType::Inline,
                    dest_url: "https://example.com/target".into(),
                    title: title.into(),
                    id: "".into(),
                },
            ),
        ] {
            let result = to_fetch_result(&article(&html), "https://example.com".into(), false);
            let markdown = result.markdown();

            assert_eq!(
                body_events(markdown),
                lone_paragraph(expected),
                "title attribute {attribute:?} must close and read back unchanged:\n{markdown}"
            );
        }
    }
}
