use super::*;

/// [T-F020]
#[test]
fn all_spa_frameworks_detected() {
    for id in SPA_ROOT_IDS {
        let html = format!(
            r#"<html><head><script src="app.js"></script></head>
                <body><div {id}></div></body></html>"#
        );
        assert!(is_js_dependent(&html), "should detect SPA with {id}");
    }
}

/// [T-F075] an uppercase `<SCRIPT>` tag is detected like its lowercase form
///
/// HTML tag names are case-insensitive and `has_thin_body` already matches them
/// that way, so a shell written with uppercase tags passed the thin-body gate and
/// then failed the script check. The exposed window is body text of roughly 50-99
/// visible bytes — under BODY_TEXT_THRESHOLD but over EXTRACT_TEXT_THRESHOLD, so
/// the post-extraction fallback misses it too — and any such page under `--raw`,
/// where that second gate is off entirely.
#[test]
fn uppercase_script_tag_is_detected() {
    let html = r#"<html><HEAD><SCRIPT src="bundle.js"></SCRIPT></HEAD>
        <BODY><div class="app"></div></BODY></html>"#;
    assert!(
        is_js_dependent(html),
        "tag case must not decide whether the page needs JS rendering"
    );
}

/// [T-F021]
#[test]
fn normal_html_not_detected() {
    let html = r#"<html><body><article>
        <h1>Title</h1><p>Long paragraph with enough content to exceed
        the threshold of one hundred characters easily.</p>
        </article></body></html>"#;
    assert!(!is_js_dependent(html));
}

/// [T-F022]
#[test]
fn script_without_spa_pattern_but_empty_body() {
    let html = r#"<html><head><script src="bundle.js"></script></head>
        <body><div class="app"></div></body></html>"#;
    assert!(is_js_dependent(html));
}

/// [T-F023]
#[test]
fn spa_pattern_without_script_but_empty_body() {
    let html = r#"<html><body><div id="root"></div></body></html>"#;
    assert!(is_js_dependent(html));
}

/// [T-F024]
#[test]
fn rich_body_with_scripts_not_detected() {
    let content = "x".repeat(200);
    let html = format!(
        r#"<html><head><script src="app.js"></script></head>
            <body><div id="root"><p>{content}</p></div></body></html>"#
    );
    assert!(!is_js_dependent(&html));
}

/// [T-F025]
#[test]
fn thin_body_without_script_or_spa_pattern_not_detected() {
    let html = "<html><body><p>short</p></body></html>";
    assert!(!is_js_dependent(html));
}

/// [T-F026]
#[test]
fn no_body_tag_falls_back_to_full_html() {
    let html = r#"<div id="root"></div><script src="app.js"></script>"#;
    assert!(is_js_dependent(html));
}
