use super::*;

/// [T-F027]
#[test]
fn style_content_excluded_from_visible_text() {
    let html = "<html><body><style>.big{font-size:9999px;color:red;margin:0 auto;padding:10px 20px 30px 40px}</style><p>hi</p></body></html>";
    assert!(has_thin_body(html));
}

/// [T-F028]
#[test]
fn uppercase_script_tag_excluded() {
    let html = "<html><body><SCRIPT>var x = 'lots of javascript code that should be ignored by the parser';</SCRIPT><p>hi</p></body></html>";
    assert!(has_thin_body(html));
}

/// [T-F029]
#[test]
fn uppercase_body_tag_found() {
    let content = "x".repeat(200);
    let html = format!("<html><BODY><p>{content}</p></BODY></html>");
    assert!(!has_thin_body(&html));
}

/// [T-F030] exactly_at_threshold_is_not_thin (body)
#[test]
fn exactly_at_threshold_is_not_thin() {
    let content = "x".repeat(BODY_TEXT_THRESHOLD);
    let html = format!("<html><body><p>{content}</p></body></html>");
    assert!(!has_thin_body(&html));
}

/// [T-F031] just_below_threshold_is_thin (body)
#[test]
fn just_below_threshold_is_thin() {
    let content = "x".repeat(BODY_TEXT_THRESHOLD - 1);
    let html = format!("<html><body><p>{content}</p></body></html>");
    assert!(has_thin_body(&html));
}

/// [T-F032]
#[test]
fn whitespace_only_body_is_thin() {
    let html = "<html><body>   \n\t  \n   </body></html>";
    assert!(has_thin_body(html));
}

/// [T-F078] the threshold counts characters, so a script does not shift it
///
/// Counting bytes puts the same prose on opposite sides of the line depending
/// on the writing system: 34 CJK characters are 102 bytes and clear a 100-byte
/// bar that 34 Latin characters (34 bytes) do not. A Japanese SPA would pass as
/// "has content" on a third of the text an English one needs, and never reach
/// the JS-rendering fallback. Every other
/// test here feeds `"x".repeat(...)`, where the two units coincide.
#[test]
fn threshold_is_the_same_length_in_any_script() {
    let page = |body: &str| format!("<html><body><p>{body}</p></body></html>");
    let below = BODY_TEXT_THRESHOLD - 1;

    assert!(
        has_thin_body(&page(&"a".repeat(below))),
        "Latin text below the threshold is thin"
    );
    assert!(
        has_thin_body(&page(&"\u{3042}".repeat(below))),
        "the same number of CJK characters must also be thin"
    );
    assert!(
        !has_thin_body(&page(&"\u{3042}".repeat(BODY_TEXT_THRESHOLD))),
        "and reaching the threshold must clear it, in either script"
    );
}
