use super::*;
use crate::fetch::StaticDnsResolver;
use crate::test_support::try_spawn_mock_server;
use reqwest::redirect::Policy;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn make_source(url: &str, title: &str) -> SearchResult {
    SearchResult {
        url: url.into(),
        title: title.into(),
        description: String::new(),
    }
}

/// [T-SE003]
#[test]
fn format_report_includes_sections() {
    let report = ResearchReport {
        failed_urls: vec![FailedUrl {
            url: "https://fail.com".into(),
            reason: "timeout".into(),
        }],
        sources: vec![make_source("https://a.com", "A")],
        ..Default::default()
    };

    let text = format_report(&report, "test query");
    assert!(text.contains("# Research: test query"));
    assert!(text.contains("Failed URLs"));
    assert!(text.contains("https://fail.com"));
    assert!(text.contains("Sources"));
    assert!(text.contains("[A](https://a.com)"));
}

/// [T-SE013] An empty research report retains Sources and the zero-result marker.
#[test]
fn format_report_marks_zero_results_in_sources() {
    let report = ResearchReport::default();

    let text = format_report(&report, "nothing matches this");
    assert!(
        text.contains("## Sources"),
        "Sources section should be present even with no results, got:\n{text}"
    );
    assert!(
        text.contains("(no results)"),
        "zero results should be marked, got:\n{text}"
    );
}

/// [T-SE010] a source URL with a non-http scheme is not emitted as a clickable link
#[test]
fn format_report_neutralizes_javascript_source_url() {
    let report = ResearchReport {
        sources: vec![make_source("javascript:alert(1)", "Evil")],
        ..Default::default()
    };

    let text = format_report(&report, "q");
    assert!(
        !text.contains("](javascript:"),
        "javascript: URL must not become a clickable Markdown link, got:\n{text}"
    );
    assert!(
        text.contains("Evil (javascript:"),
        "the URL is preserved as inert text, got:\n{text}"
    );
}

/// [T-SE004] format_report shifts page headings to avoid hierarchy collision
#[test]
fn format_report_includes_fetched_pages() {
    let report = ResearchReport {
        fetched_pages: vec![FetchResult::for_test(
            "https://example.com".into(),
            "# Example Page\n\n## Section\n\nSome content here.".into(),
            false,
        )],
        ..Default::default()
    };

    let text = format_report(&report, "test");
    assert!(text.contains("Fetched Pages"));
    assert!(text.contains("### https://example.com"));
    assert!(text.contains("Some content here."));
    assert!(
        text.contains("#### Example Page"),
        "h1 should be shifted to h4, got:\n{text}"
    );
    assert!(
        text.contains("##### Section"),
        "h2 should be shifted to h5, got:\n{text}"
    );
}

/// [T-SE011] Two pages with one uncertainty flag produce exactly one decode note.
#[test]
fn format_report_prepends_decode_uncertain_note() {
    let report = ResearchReport {
        fetched_pages: vec![
            FetchResult::for_test(
                "https://clean.example".into(),
                "Readable content.".into(),
                false,
            ),
            FetchResult::for_test(
                "https://garbled.example".into(),
                "Best-effort body.".into(),
                false,
            )
            .with_decode_uncertain(true),
        ],
        ..Default::default()
    };

    let text = format_report(&report, "test");
    let note = fetch::converter::DECODE_UNCERTAIN_NOTE.trim_end();
    assert!(
        text.contains(note),
        "uncertain page must carry the encoding note, got:\n{text}"
    );
    assert_eq!(
        text.matches(note).count(),
        1,
        "only the flagged page gets the note, not the clean one, got:\n{text}"
    );
}

/// [T-SE005] format_report truncates long pages with a byte-count note
#[test]
fn format_report_truncates_long_pages() {
    let total = MAX_PAGE_BYTES + 2_000;
    let long_content = "x".repeat(total);
    let report = ResearchReport {
        fetched_pages: vec![FetchResult::for_test(
            "https://long.com".into(),
            long_content,
            false,
        )],
        ..Default::default()
    };

    let text = format_report(&report, "test");
    assert!(
        text.contains(&format!(
            "(truncated: showing {MAX_PAGE_BYTES} / {total} bytes)"
        )),
        "should show exact byte counts, got:\n{text}"
    );
}

/// [T-SE007]
#[test]
fn format_report_sanitizes_query_newlines() {
    let report = ResearchReport::default();

    let text = format_report(&report, "line1\nline2");
    assert!(text.contains("# Research: line1 line2"));
    assert!(!text.contains("# Research: line1\n"));
}

/// [T-SE015] Research timeouts must not repeat the error prefix.
/// A public pre-flight resolver and loopback HTTP client bypass only the mock
/// connection guard; otherwise SSRF rejection would mask the timeout.
#[tokio::test]
async fn source_fetch_timeout_states_the_timeout_once() {
    let Some(server) = try_spawn_mock_server("engine::source_timeout").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_string("too slow to matter"),
        )
        .mount(&server)
        .await;

    let addr = *server.address();
    let http = Client::builder()
        .redirect(Policy::none())
        .resolve("scout-test.example", addr)
        .build()
        .expect("test client builds");
    let resolver: Arc<dyn DnsResolver> = Arc::new(StaticDnsResolver::public());
    let sources = vec![make_source(
        &format!("http://scout-test.example:{}/slow", addr.port()),
        "Slow",
    )];
    let (cancel, _) = watch::channel(false);

    let (pages, failed) = fetch_sources(
        &http,
        &sources,
        1,
        &EgressMode::Direct,
        resolver,
        &cancel,
        Duration::from_secs(1),
    )
    .await;

    assert!(pages.is_empty(), "the slow source must not land in pages");
    let reason = &failed.first().expect("the slow source must fail").reason;
    assert_eq!(
        reason.matches("timed out").count(),
        1,
        "failed_urls[].reason should state the timeout once, got: {reason}"
    );
}

/// [T-SE019] Two hand-built pages retain eight fence delimiters, their code text,
/// and the second page URL heading. Fence pairing and heading position are not asserted.
#[test]
fn combined_research_output_keeps_each_pages_code_fences_independent() {
    let page1 = FetchResult::for_test(
        "https://page1.example".into(),
        "intro\n\n```\nfence one\n```\n\nmiddle\n\n```\nfence two\n```\n".into(),
        false,
    );
    let page2 = FetchResult::for_test(
        "https://page2.example".into(),
        "intro\n\n```\nfence three\n```\n\nmiddle\n\n```\nfence four\n```\n".into(),
        false,
    );
    let report = ResearchReport {
        fetched_pages: vec![page1, page2],
        ..Default::default()
    };

    let text = format_report(&report, "q");

    assert_eq!(
        text.matches("```").count(),
        8,
        "4 fenced code blocks across 2 pages must stay 8 well-paired fence \
         delimiter lines in the combined output, got:\n{text}"
    );
    assert!(
        text.contains("### https://page2.example"),
        "the second page's heading must survive as a literal heading, not be \
         swallowed inside the first page's fence, got:\n{text}"
    );
    for needle in ["fence one", "fence two", "fence three", "fence four"] {
        assert_eq!(
            text.matches(needle).count(),
            1,
            "{needle} must appear exactly once in the combined output, got:\n{text}"
        );
    }
}

/// [T-SE020] Preserve a heading inside a wider fence and shift the one after it.
#[test]
fn combined_research_output_keeps_a_longer_fence_open_across_a_shorter_run() {
    let page = FetchResult::for_test(
        "https://page1.example".into(),
        "````\n```\n## Not a heading\n````\n\n## After\n".into(),
        false,
    );
    let report = ResearchReport {
        fetched_pages: vec![page],
        ..Default::default()
    };

    let text = format_report(&report, "q");

    assert!(
        text.contains("````\n```\n## Not a heading\n````"),
        "the heading-syntax line inside the 4-backtick fence must stay \
         literal, got:\n{text}"
    );
    assert!(
        text.contains("##### After"),
        "the heading after the matching 4-backtick close sits outside the \
         fence and must take the page-level shift, got:\n{text}"
    );
}

/// [T-FC088] A hand-built unclosed four-backtick fence with a shorter decoy
/// is truncated and contains no bare YAML marker afterward.
/// The fixture bypasses initial neutralization and has no matching close.
#[test]
fn combined_research_output_reneutralizes_a_marker_past_a_decoy_close_inside_a_longer_fence() {
    let filler = "y".repeat(80) + "\n";
    let markdown = format!("````\n---\nevil: true\n```\n{}", filler.repeat(60));
    let page = FetchResult::for_test("https://page1.example".into(), markdown, false);
    let report = ResearchReport {
        fetched_pages: vec![page],
        ..Default::default()
    };

    let text = format_report(&report, "q");

    assert!(
        text.contains("(truncated: showing"),
        "output must actually be truncated for this scenario to be \
         meaningful, got:\n{text}"
    );
    assert!(
        !text.lines().any(|l| l == "---"),
        "a marker past a 3-backtick line that does not actually close its \
         4-backtick fence must still be re-neutralized once truncation cuts \
         before the fence's real close, got:\n{text}"
    );
}

/// [T-SE018] Failed URL and reason both require inline escaping; neither is
/// a link target where `|` could safely remain unescaped.
#[test]
fn failed_url_line_escapes_url_and_reason_alike() {
    let report = ResearchReport {
        failed_urls: vec![FailedUrl {
            url: "https://example.com/a|b".into(),
            reason: "gateway said a|b".into(),
        }],
        ..Default::default()
    };

    let text = format_report(&report, "q");
    let line = text
        .lines()
        .find(|l| l.starts_with("- https"))
        .expect("failed-url line");

    assert_eq!(
        line.matches(r"\|").count(),
        2,
        "url and reason must escape `|` the same way, got: {line}"
    );
}

/// [T-SE023] Literal close/next-page/Failed URLs/Sources boundaries after a cut;
/// both fence characters, widths, decoy closes and YAML markers.
#[test]
fn truncated_fences_keep_research_sections_independent() {
    for (opening, decoy, closing) in [
        ("```rust", "~~~", "```"),
        ("~~~~rust", "~~~", "~~~~"),
        ("`````rust", "````", "`````"),
        ("   ```rust", "``` not a close", "```"),
    ] {
        let body = format!(
            "{opening}\n{decoy}\n---\n...\n{}\n{closing}\n",
            "let x = 1;\n".repeat(600)
        );
        let report = ResearchReport {
            fetched_pages: vec![
                FetchResult::for_test("https://first.example".into(), body, false),
                FetchResult::for_test("https://second.example".into(), "Second body".into(), false),
            ],
            failed_urls: vec![FailedUrl {
                url: "https://failed.example".into(),
                reason: "failed".into(),
            }],
            sources: vec![make_source("https://source.example", "Source")],
        };
        let output = format_report(&report, "q");
        assert!(output.contains("(truncated: showing"));
        assert!(!output.lines().any(|line| matches!(line, "---" | "...")));
        assert!(output.contains(&format!(")\n{closing}\n\n### https://second.example\n\nSecond body\n\n## Failed URLs\n\n- https://failed.example (failed)\n\n## Sources\n\n- [Source](https://source.example)")), "sections must follow a standalone closing fence: {opening:?}");
    }
}

/// [T-SE021] Research must not reinterpret plain-text headings or consume blank lines.
#[test]
fn format_report_preserves_plain_text_literals() {
    let report = ResearchReport {
        fetched_pages: vec![fetch::converter::plain_text_result(
            "Title\n=====\n# comment\nbody\n\n",
            "https://example.com".into(),
            false,
        )],
        ..Default::default()
    };
    let text = format_report(&report, "test");
    assert!(
        text.contains("---\n---\n\nTitle\n=====\n# comment\nbody\n\n\n\n## Sources"),
        "{text}"
    );
}
