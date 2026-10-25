use super::convert;

/// [T-HMD001] Nested emphasis, links and images remain ordinary Markdown.
#[test]
fn renders_inline_formatting_and_link_destinations() {
    let html = r#"<p>Hello <strong>bold <em>and italic</em></strong>. <a href="https://example.com/a(b)" title="omit">Link</a> <img src="https://example.com/image.png" alt="an [image]"></p>"#;
    assert_eq!(
        convert(html),
        r"Hello **bold *and italic***. [Link](https://example.com/a\(b\)) ![an \[image\]](https://example.com/image.png)"
    );
}

/// [T-HMD002] A nested list remains inside the parent item; ordered starts survive.
#[test]
fn renders_nested_lists_and_ordered_start() {
    let markdown =
        convert("<ol start=3><li>first<ul><li>nested</li></ul></li><li>second</li></ol>");
    assert!(markdown.starts_with("3.  first"), "{markdown}");
    assert!(markdown.contains("\n    *   nested"), "{markdown}");
    assert!(markdown.contains("\n4.  second"), "{markdown}");
}

/// [T-HMD003] Quotes retain inline markup and nested paragraph boundaries.
#[test]
fn renders_blockquotes() {
    assert_eq!(
        convert("<blockquote><p>one <em>two</em></p><p>three</p></blockquote>"),
        "> one *two*\n> \n> three"
    );
}

/// [T-HMD004] Math spans preserve the prior syntax, except inside code blocks.
#[test]
fn preserves_math_spans_outside_preformatted_content() {
    assert_eq!(
        convert(
            r#"<p><span class="math math-inline">a_b</span> <span class="math math-display">c_d</span></p><pre><span class="math math-inline">a_b</span></pre>"#
        ),
        "$a_b$ $$c_d$$\n\n```\na_b\n```"
    );
}

/// [T-HMD005] Templates and comments never contribute text, including within code.
#[test]
fn suppresses_inert_content() {
    assert_eq!(
        convert(
            "<p>visible<!-- secret --><template>secret</template></p><pre>x<template>secret</template>y</pre>"
        ),
        "visible\n\n```\nxy\n```"
    );
}

/// [T-HMD006] Representative tags retain byte-for-byte output from htmd 0.5.6.
#[test]
fn matches_the_previous_converter_corpus() {
    let cases: Vec<(String, String)> =
        serde_json::from_str(include_str!("compatibility.json")).expect("compatibility fixtures");
    for (html, expected) in cases {
        assert_eq!(convert(&html), expected, "HTML: {html}");
    }
}
