use super::*;
use crate::fetch::StaticDnsResolver;
use crate::test_support::try_spawn_mock_server;
use reqwest::redirect::Policy;
use std::collections::VecDeque;
use std::sync::Mutex;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn real_resolver() -> Arc<dyn DnsResolver> {
    Arc::new(fetch::TokioDnsResolver)
}

struct MockSearch {
    responses: Mutex<VecDeque<Result<Vec<SearchResult>, BraveError>>>,
    captured: Mutex<Vec<(String, Option<String>)>>,
}

impl MockSearch {
    fn with_results(results: Vec<SearchResult>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([Ok(results)])),
            captured: Mutex::new(Vec::new()),
        }
    }

    fn all_fail(error: BraveError) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([Err(error)])),
            captured: Mutex::new(Vec::new()),
        }
    }

    fn captured(&self) -> Vec<(String, Option<String>)> {
        self.captured.lock().unwrap().clone()
    }
}

impl SearchClient for MockSearch {
    async fn search(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<Vec<SearchResult>, BraveError> {
        self.captured
            .lock()
            .unwrap()
            .push((query.to_owned(), search_lang.map(str::to_owned)));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(BraveError::RateLimited { retry_after: None }))
    }
}

fn make_source(url: &str, title: &str) -> SearchResult {
    SearchResult {
        url: url.into(),
        title: title.into(),
        description: String::new(),
    }
}

/// [T-SE022] Depth limits actual requests, not just the displayed result count.
#[tokio::test]
async fn fetch_sources_requests_only_the_ranked_depth_prefix() {
    let Some(server) = try_spawn_mock_server("engine::depth_prefix").await else {
        return;
    };
    // A substantive article keeps this depth test on the HTTP path even when
    // js-rendering is enabled; a thin fixture would launch a real browser.
    let body = format!(
        "<article><h1>Fixture article</h1><p>{}</p></article>",
        "This is a complete paragraph describing the fixture article in detail. ".repeat(30)
    );
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(2)
        .mount(&server)
        .await;
    let addr = *server.address();
    let http = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .resolve("scout-test.example", addr)
        .build()
        .expect("test client builds");
    let sources: Vec<_> = ["first", "second", "third"]
        .iter()
        .map(|name| {
            make_source(
                &format!("http://scout-test.example:{}/{name}", addr.port()),
                name,
            )
        })
        .collect();
    let (cancel, _) = watch::channel(false);
    let (pages, failures) = fetch_sources(
        &http,
        &sources,
        2,
        &EgressMode::Direct,
        Arc::new(StaticDnsResolver::single("93.184.216.34")),
        &cancel,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        failures.is_empty(),
        "unexpected fetch failures: {failures:?}"
    );
    assert_eq!(
        pages.iter().map(FetchResult::url).collect::<Vec<_>>(),
        [sources[0].url.as_str(), sources[1].url.as_str()],
        "exactly the first two ranked sources must be fetched"
    );
    let mut requested: Vec<_> = server
        .received_requests()
        .await
        .expect("request recording is enabled")
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    requested.sort();
    assert_eq!(requested, ["/first", "/second"]);
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

/// [T-SE014] Hand-built out-of-order outcomes are partitioned in source rank order.
#[test]
fn partition_by_rank_orders_failures_like_pages() {
    let timeout = || fetch::FetchError::DnsResolution("no such host".into());
    let page = |url: &str| FetchResult::for_test(url.to_owned(), "body".to_owned(), false);

    let outcomes = vec![
        (3, "https://d.example", Err(timeout())),
        (0, "https://a.example", Ok(page("https://a.example"))),
        (2, "https://c.example", Err(timeout())),
        (1, "https://b.example", Ok(page("https://b.example"))),
    ];

    let (pages, failed) = partition_by_rank(outcomes);

    assert_eq!(
        pages.iter().map(FetchResult::url).collect::<Vec<_>>(),
        ["https://a.example", "https://b.example"]
    );
    assert_eq!(
        failed.iter().map(|f| f.url.as_str()).collect::<Vec<_>>(),
        ["https://c.example", "https://d.example"],
        "failed URLs carry search ranking too, not completion order"
    );
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

/// [T-SE016] format_report does not emit a "## Search Result" header
#[test]
fn format_report_omits_search_result_header() {
    let report = ResearchReport {
        sources: vec![make_source("https://a.com", "A")],
        ..Default::default()
    };

    let text = format_report(&report, "test");
    assert!(
        !text.contains("## Search Result"),
        "report must not contain the obsolete Search Result header, got:\n{text}"
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

/// [T-SE008] research forwards the English query and delivers the fetched source body.
#[tokio::test]
async fn research_with_mock_returns_report() {
    let Some(server) = try_spawn_mock_server("engine::research_report").await else {
        return;
    };
    // A full article avoids automatic JS fallback under all-features.
    let body = "Primary source fixture describes the research result in detail. ".repeat(30);
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<article><h1>Source article</h1><p>{body}</p></article>"
        )))
        .expect(1)
        .mount(&server)
        .await;
    let addr = *server.address();
    let url = format!("http://scout-test.example:{}/article", addr.port());
    let mock = MockSearch::with_results(vec![make_source(&url, "Source article")]);
    let http = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .resolve("scout-test.example", addr)
        .build()
        .expect("test client builds");
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));

    let req = ResearchRequest {
        query: "test",
        depth: 3,
        lang: Lang::En,
        egress: EgressMode::Direct,
    };
    let (cancel, _) = watch::channel(false);
    let report = research(&mock, &http, &req, resolver, &cancel)
        .await
        .unwrap();

    assert_eq!(report.sources.len(), 1);
    assert_eq!(report.sources[0].url, url);
    assert_eq!(report.sources[0].title, "Source article");
    assert!(report.failed_urls.is_empty(), "{:?}", report.failed_urls);
    assert_eq!(report.fetched_pages.len(), 1);
    assert_eq!(report.fetched_pages[0].url(), url);
    assert!(
        report.fetched_pages[0].markdown().contains(body.trim()),
        "the report must deliver the fixture body, not merely its source URL"
    );

    let captured = mock.captured();
    assert_eq!(
        captured.len(),
        1,
        "research must issue exactly one Brave query"
    );
    assert_eq!(captured[0].0, "test", "query must be sent verbatim");
    assert_eq!(
        captured[0].1,
        Some("en".to_owned()),
        "Lang::En -> search_lang=en"
    );
}

/// [T-SE017] Lang::Auto issues exactly one Brave call, with no bilingual expansion
#[tokio::test]
async fn research_auto_lang_issues_single_call() {
    let mock = MockSearch::with_results(vec![]);
    let http = Client::new();
    let resolver = real_resolver();

    let req = ResearchRequest {
        query: "型安全 TypeScript",
        depth: 3,
        lang: Lang::Auto,
        egress: EgressMode::Direct,
    };
    let (cancel, _) = watch::channel(false);
    let report = research(&mock, &http, &req, resolver, &cancel)
        .await
        .unwrap();

    assert!(report.sources.is_empty());
    assert!(report.fetched_pages.is_empty());
    assert!(report.failed_urls.is_empty());

    let captured = mock.captured();
    assert_eq!(
        captured.len(),
        1,
        "Lang::Auto must NOT trigger bilingual expansion under Brave"
    );
    assert_eq!(
        captured[0].0, "型安全 TypeScript",
        "query must be sent verbatim"
    );
    assert_eq!(captured[0].1, None, "Lang::Auto -> search_lang omitted");
}

/// [T-SE012] research surfaces the underlying Brave error when search fails
#[tokio::test]
async fn research_search_failure_returns_error() {
    let mock = MockSearch::all_fail(BraveError::RateLimited { retry_after: None });
    let http = Client::new();
    let resolver = real_resolver();

    let req = ResearchRequest {
        query: "test",
        depth: 3,
        lang: Lang::En,
        egress: EgressMode::Direct,
    };
    let (cancel, _) = watch::channel(false);
    let err = research(&mock, &http, &req, resolver, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, BraveError::RateLimited { .. }));
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
    let resolver: Arc<dyn DnsResolver> = Arc::new(StaticDnsResolver::single("93.184.216.34"));
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
