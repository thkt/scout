use super::test_helpers::article;
use super::*;
use crate::search::engine::MAX_PAGE_BYTES;
use crate::yaml::truncate_and_reneutralize;

/// [T-FC001]
#[test]
fn always_includes_frontmatter() {
    let article = ExtractedArticle {
        title: Some("My Title".into()),
        byline: Some("Jane Doe".into()),
        published_time: Some("2026-01-15".into()),
        content_html: "<p>Body text</p>".into(),
        used_raw_fallback: false,
    };

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(result.markdown().starts_with("---\n"));
    assert!(result.markdown().contains("\n---\n\n"));
    assert!(result.markdown().contains("title: \"My Title\""));
    assert!(result.markdown().contains("author: \"Jane Doe\""));
    assert!(result.markdown().contains("date: \"2026-01-15\""));
    assert!(result.markdown().contains("Body text"));
}

/// [T-FC002]
#[test]
fn frontmatter_omits_missing_fields() {
    let article = ExtractedArticle {
        title: Some("Only Title".into()),
        ..article("<p>Text</p>")
    };

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    assert!(result.markdown().contains("title: \"Only Title\""));
    assert!(!result.markdown().contains("author:"));
    assert!(!result.markdown().contains("date:"));
}

/// [T-FC008] format_with_frontmatter neutralizes doc markers in the page body
#[test]
fn frontmatter_body_cannot_inject_document_marker() {
    let article = ExtractedArticle {
        title: Some("T".into()),
        ..article("")
    };
    let body = "intro\n---\ninjected: pwned\nreal";
    let out = format_with_frontmatter(&article, body);

    let after_fm = out.split("---\n\n").nth(1).expect("body after frontmatter");
    assert!(
        !after_fm.lines().any(|l| l == "---" || l == "..."),
        "page body must not introduce a bare YAML document marker:\n{out}"
    );
    assert!(after_fm.contains("injected: pwned"));
}

/// [T-FC014] Raw-fallback and decode-uncertain flags have different sources;
/// swapping them or dropping either must be observable.
#[test]
fn to_fetch_result_carries_both_flags() {
    let raw_fallback_only = ExtractedArticle {
        used_raw_fallback: true,
        ..article("<p>x</p>")
    };
    let result = to_fetch_result(&raw_fallback_only, "https://example.com".into(), false);
    assert!(
        result.used_raw_fallback(),
        "the article's raw-fallback flag must reach the result"
    );
    assert!(
        !result.decode_uncertain(),
        "the caller passed decode_uncertain=false"
    );

    let decode_uncertain_only = article("<p>x</p>");
    let result = to_fetch_result(&decode_uncertain_only, "https://example.com".into(), true);
    assert!(
        !result.used_raw_fallback(),
        "the article carried no raw-fallback flag"
    );
    assert!(
        result.decode_uncertain(),
        "the caller's decode_uncertain must reach the result"
    );
}

/// [T-FC104] Field caps must leave a closed frontmatter and body after
/// the caller's output cap, not merely shorten individual fields.
#[test]
fn a_title_over_the_cap_still_yields_a_closed_frontmatter_and_a_body() {
    let article = ExtractedArticle {
        title: Some("T".repeat(120_000)),
        byline: None,
        published_time: None,
        content_html: "<p>body text</p>".to_owned(),
        used_raw_fallback: false,
    };

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let cut = truncate_and_reneutralize(result.markdown(), MAX_PAGE_BYTES);
    let delimiters = cut.lines().filter(|l| *l == "---").count();

    assert_eq!(
        delimiters, 2,
        "the frontmatter must open and close within the cap:\n{cut}"
    );
    assert!(
        cut.contains("body text"),
        "the body must survive a title that would otherwise fill the cap:\n{cut}"
    );
}

/// [T-FC109] A lone CR in a plain-text body cannot start a YAML document marker line.
#[test]
fn plain_text_lone_cr_cannot_start_a_yaml_marker_line() {
    let result = plain_text_result(
        "a\r--- evil: true\r\nb\r...\r",
        "https://example.com".into(),
        false,
    );

    assert_eq!(result.markdown(), "---\n---\n\na\n*** evil: true\nb\n***\n");
}

/// [T-FC110] A lone CR inside inline code cannot start a YAML document marker line.
#[test]
fn inline_code_lone_cr_cannot_start_a_yaml_marker_line() {
    let result = to_fetch_result(
        &article("<p><code>x&#13;--- evil: true</code></p>"),
        "https://example.com".into(),
        false,
    );

    let body = result.markdown().strip_prefix("---\n---\n\n").unwrap();
    assert!(!body.contains('\r'), "{body:?}");
    assert!(
        !body.lines().any(|line| line.starts_with("---")),
        "{body:?}"
    );
}

/// [T-FC101] Author and date frontmatter values are capped like the title.
#[test]
fn frontmatter_caps_byline_and_published_time() {
    let article = ExtractedArticle {
        byline: Some("b".repeat(10_000)),
        published_time: Some(format!("2026-08-16T{}", "0".repeat(10_000))),
        ..article("<p>Text</p>")
    };

    let result = to_fetch_result(&article, "https://example.com".into(), false);

    for key in ["author: ", "date: "] {
        let line = result
            .markdown()
            .lines()
            .find(|line| line.starts_with(key))
            .unwrap_or_else(|| panic!("frontmatter must carry {key}"));
        assert!(line.len() < 10_000, "{key} is {} bytes", line.len());
    }
}
