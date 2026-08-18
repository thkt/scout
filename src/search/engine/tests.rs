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

/// [T-SE014] partition_by_rank restores search ranking in both report sections
///
/// Fetches resolve in completion order, so the outcomes arrive shuffled. Sorting
/// only the successes left `## Failed URLs` in timeout-return order, printing the
/// same two failures in a different order on each run.
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

/// [T-SE013] a zero-result run still emits the Sources section, marked `(no results)`
///
/// DR-0005 fixes this as the zero-result contract: without the marker, a report
/// with nothing found is byte-identical to one whose sections were dropped by a
/// formatting fault, so a markdown reader cannot tell the two apart. This is
/// deliberately the opposite of `search`, which DR-0020 pins to true empty output.
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

/// [T-SE011] format_report prepends the decode-uncertain note for a flagged page
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

/// [T-SE008] research returns a populated report when search succeeds
#[tokio::test]
async fn research_with_mock_returns_report() {
    let mock = MockSearch::with_results(vec![make_source("https://a.com", "A")]);
    let http = Client::new();
    let resolver = real_resolver();

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
    let mock = MockSearch::with_results(vec![make_source("https://a.com", "A")]);
    let http = Client::new();
    let resolver = real_resolver();

    let req = ResearchRequest {
        query: "型安全 TypeScript",
        depth: 3,
        lang: Lang::Auto,
        egress: EgressMode::Direct,
    };
    let (cancel, _) = watch::channel(false);
    let _ = research(&mock, &http, &req, resolver, &cancel)
        .await
        .unwrap();

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

/// [T-SE015] Pins the payload rule stated on `FetchError::Timeout`
/// (src/fetch.rs) for the research call site, which reported a source that
/// can otherwise double the payload into "fetch timed out: page fetch timed
/// out after 15s".
///
/// The client reaches the loopback wiremock the way `scout_reaching`
/// (src/tools/test_helpers.rs) does — a `.resolve()` client paired with a
/// public-address pre-flight resolver — because the SSRF guard would otherwise
/// reject the loopback address before the budget ever elapsed.
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

/// [T-SE019] Combined research output keeps each pages code fences independent
///
/// Each page's Markdown is independently well-formed (every fence it opens,
/// it also closes), so `format_fetched_pages` concatenating several such
/// pages — with `shift_headings` run fresh per page rather than over the
/// combined string — must not let one page's fence swallow the next page's
/// heading or code content when the combined report is read as one Markdown
/// document.
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

/// [T-SE020] Combined research output keeps a longer fence open across a shorter run
///
/// `format_fetched_pages` runs `shift_headings` per page, so the fence-length
/// tracking has to hold on the path the report takes, not only on a direct
/// `shift_headings` call: a heading-syntax line inside the 4-backtick fence
/// stays literal, and only the line after the matching 4-backtick close takes
/// the page-level shift.
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

/// [T-FC088] Combined research output reneutralizes a marker past a decoy close inside a longer fence
///
/// The fixture stands in for text `neutralize_yaml_markers_outside_fences`
/// (src/yaml.rs) already ran over: `---` survived verbatim because, at
/// neutralization time, its 4-backtick fence was closed later on. Between
/// the marker and that real close sits a 3-backtick line, which a fence
/// tracker keyed on run length alone (not `markdown::track_fence`'s
/// length-matching rule) could mistake for the close. Truncation then cuts
/// well past that 3-backtick decoy but well before the real 4-backtick
/// close, so the fence is genuinely still open at the cut.
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

/// [T-SE018] both halves of a failed-URL line take the same escape
///
/// The URL went through `escape_md_link` and the reason through
/// `escape_md_inline`, which differ on `|`: the first leaves it, since a link
/// target has no table column to break out of. Nothing on this line is a link
/// target, so one line could carry `a|b` beside `err \| msg`.
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
