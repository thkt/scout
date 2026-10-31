use super::test_helpers::article;
use super::*;
use crate::fetch::extractor::extract_article;

/// [T-FC084] Pure-mode block handling must not leak script or style source.
#[test]
fn script_and_style_content_left_in_content_html_does_not_reach_the_body() {
    let article = article(
        "<div><p>Visible text</p>\
         <script>var scriptSecret = 'do-not-show';</script>\
         <style>.hiddenStyleRule { color: red; }</style></div>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Visible text"),
        "ordinary sibling text must still reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("scriptSecret"),
        "a <script> element's source text must not reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("hiddenStyleRule"),
        "a <style> element's source text must not reach the body:\n{markdown}"
    );
}

/// [T-FC085] Content of noscript, textarea, iframe, and SVG title and desc
/// elements does not reach the body. With scripting enabled, noscript and
/// iframe children are parsed as raw text.
#[test]
fn noscript_textarea_iframe_child_and_svg_desc_title_content_do_not_reach_the_body() {
    let article = article(
        "<div><p>Visible text</p>\
         <noscript>noscript fallback content</noscript>\
         <textarea>textarea leaked content</textarea>\
         <iframe><p>iframe fallback content</p></iframe>\
         <svg><title>svg title content</title><desc>svg desc content</desc></svg>\
         </div>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Visible text"),
        "ordinary sibling text must still reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("noscript fallback content"),
        "a <noscript> element's content must not reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("textarea leaked content"),
        "a <textarea> element's content must not reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("iframe fallback content"),
        "an <iframe> element's child content must not reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("svg title content"),
        "an <svg> element's <title> content must not reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("svg desc content"),
        "an <svg> element's <desc> content must not reach the body:\n{markdown}"
    );
}

/// [T-FC086] Raw extraction bypasses Readability cleanup; suppression must
/// still hold through the real extraction-to-conversion seam.
#[test]
fn raw_extraction_end_to_end_does_not_leak_script_content_into_the_body() {
    let html = "<html><head><title>Page</title></head><body>\
         <p>Visible text</p>\
         <script>var rawPathSecret = 'do-not-show';</script>\
         </body></html>";
    let raw_article = super::super::extractor::extract_raw(html);

    let result = to_fetch_result(&raw_article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Visible text"),
        "ordinary sibling text must still reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("rawPathSecret"),
        "a <script> element's source text must not reach the body on the raw \
         end-to-end path:\n{markdown}"
    );
}

/// [T-FC089] XHTML self-closed raw-text tags must not swallow the rest of the
/// body; an HTML parser ignores their self-closing slash. Cover script and iframe.
#[test]
fn body_after_a_self_closed_raw_text_tag_still_reaches_the_body() {
    let article = article(
        "<div><p>Before script</p><script src=\"app.js\" />\
         <p>After script</p><iframe src=\"embed.html\"/>\
         <p>After iframe</p></div>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Before script"),
        "text ahead of the self-closed tag must reach the body:\n{markdown}"
    );
    assert!(
        markdown.contains("After script"),
        "text after a self-closed <script /> must reach the body:\n{markdown}"
    );
    assert!(
        markdown.contains("After iframe"),
        "text after a self-closed <iframe /> must reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("app.js"),
        "the rewritten <script> must still be suppressed, attributes \
         included:\n{markdown}"
    );
}

/// [T-FC090] Tag-name prefixes and quoted `>` must not confuse the byte scan;
/// ordinary closing tags must not receive another end tag.
#[test]
fn self_closed_tag_rewrite_respects_name_boundaries_and_quoted_attributes() {
    assert_eq!(
        close_self_closed_raw_text_tags("<scriptlet a=\"b\" />x"),
        "<scriptlet a=\"b\" />x",
        "a longer tag name starting with a target name must not be rewritten"
    );
    assert_eq!(
        close_self_closed_raw_text_tags("<script data-x=\"a>b\" />x"),
        "<script data-x=\"a>b\" ></script>x",
        "a > inside a quoted attribute value must not end the start tag"
    );
    assert_eq!(
        close_self_closed_raw_text_tags("<script>var a = 1;</script>x"),
        "<script>var a = 1;</script>x",
        "an ordinarily closed tag must pass through unchanged"
    );
}

/// [T-FC091] Namespace-specific suppression must preserve visible HTML desc
/// text while removing SVG desc text.
#[test]
fn desc_outside_the_svg_namespace_keeps_its_text_in_the_body() {
    let article = article(
        "<div><p>before <desc>html desc content</desc> after</p>\
         <svg><desc>svg desc content</desc></svg></div>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("html desc content"),
        "a <desc> outside the SVG namespace must keep its text:\n{markdown}"
    );
    assert!(
        !markdown.contains("svg desc content"),
        "an SVG <desc> must still be suppressed:\n{markdown}"
    );
}

/// [T-FC092] A self-closing tag inside a JS string must stay raw text;
/// rewriting it would end the script early and leak the remaining source.
#[test]
fn a_script_tag_inside_js_source_is_not_rewritten_into_an_early_close() {
    let article = article(
        "<div><p>Visible text</p>\
         <script>document.write('<script src=\"x\" />'); var leaked = 'nestedSecret';</script>\
         </div>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Visible text"),
        "ordinary sibling text must still reach the body:\n{markdown}"
    );
    assert!(
        !markdown.contains("nestedSecret"),
        "script source following a <script … /> written inside it must not \
         reach the body:\n{markdown}"
    );
}

/// [T-FC097] Hidden-source suppression must hold through Readability's raw
/// fallback, not only hand-built converter fixtures.
#[test]
fn readability_failure_path_still_drops_script_content() {
    // Avoid `_` in the leak marker: body text escapes it as `\_`, so a literal
    // substring assertion could then miss leaked script text.
    let html = "<script>SCRIPTMARKER</script>";
    let article = extract_article(html, Some("https://example.com"));
    assert!(
        article.used_raw_fallback,
        "fixture must take the fallback for this test to cover that path"
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        !markdown.contains("SCRIPTMARKER"),
        "a script body must not reach the output on the fallback path:\n{markdown}"
    );
}
