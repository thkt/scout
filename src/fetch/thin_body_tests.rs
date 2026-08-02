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
