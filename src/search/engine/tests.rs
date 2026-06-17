use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;

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

/// [T-SE003] format_report includes Sources section with shaped data
#[test]
fn format_report_includes_sections() {
    let report = ResearchReport {
        fetched_pages: vec![],
        failed_urls: vec![FailedUrl {
            url: "https://fail.com".into(),
            reason: "timeout".into(),
        }],
        sources: vec![make_source("https://a.com", "A")],
    };

    let text = format_report(&report, "test query");
    assert!(text.contains("# Research: test query"));
    assert!(text.contains("Failed URLs"));
    assert!(text.contains("https://fail.com"));
    assert!(text.contains("Sources"));
    assert!(text.contains("[A](https://a.com)"));
}

/// [T-SE010] a source URL with a non-http scheme is not emitted as a clickable link
#[test]
fn format_report_neutralizes_javascript_source_url() {
    let report = ResearchReport {
        fetched_pages: vec![],
        failed_urls: vec![],
        sources: vec![make_source("javascript:alert(1)", "Evil")],
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

/// [T-7] AC-3.1: format_report does not emit "## Search Result" header
#[test]
fn format_report_omits_search_result_header() {
    let report = ResearchReport {
        fetched_pages: vec![],
        failed_urls: vec![],
        sources: vec![make_source("https://a.com", "A")],
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
        failed_urls: vec![],
        sources: vec![],
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

/// [T-SE011] format_report prepends the decode-uncertain note for a flagged page (issue #241)
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
        failed_urls: vec![],
        sources: vec![],
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
        failed_urls: vec![],
        sources: vec![],
    };

    let text = format_report(&report, "test");
    assert!(
        text.contains(&format!(
            "(truncated: showing {MAX_PAGE_BYTES} / {total} bytes)"
        )),
        "should show exact byte counts, got:\n{text}"
    );
}

/// [T-SE007] format_report sanitizes newline characters in the heading
#[test]
fn format_report_sanitizes_query_newlines() {
    let report = ResearchReport {
        fetched_pages: vec![],
        failed_urls: vec![],
        sources: vec![],
    };

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

/// [T-8] AC-3: bilingual expansion is gone; Lang::Auto issues exactly one Brave call
#[tokio::test]
async fn research_auto_lang_issues_single_call() {
    let mock = MockSearch::with_results(vec![make_source("https://a.com", "A")]);
    let http = Client::new();
    let resolver = real_resolver();

    let req = ResearchRequest {
        query: "型安全 TypeScript",
        depth: 3,
        lang: Lang::Auto,
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

/// [T-SE010] research surfaces the underlying Brave error when search fails
#[tokio::test]
async fn research_search_failure_returns_error() {
    let mock = MockSearch::all_fail(BraveError::RateLimited { retry_after: None });
    let http = Client::new();
    let resolver = real_resolver();

    let req = ResearchRequest {
        query: "test",
        depth: 3,
        lang: Lang::En,
    };
    let (cancel, _) = watch::channel(false);
    let err = research(&mock, &http, &req, resolver, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, BraveError::RateLimited { .. }));
}
