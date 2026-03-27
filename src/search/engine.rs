use std::fmt::Write;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use reqwest::Client;
use tracing::warn;

use crate::fetch;
use crate::fetch::DnsResolver;
use crate::fetch::converter::FetchResult;
use crate::gemini::client::{GeminiError, SearchClient};
use crate::gemini::types::{GroundedResult, Source};
use crate::markdown::{escape_md_link, sanitize_heading, shift_headings, truncate_with_note};
use crate::search::Lang;
use crate::search::bilingual::expand_bilingual;
use crate::search::topical::expand_topical;

use crate::retry::{self, retry_with};

const MAX_PAGE_BYTES: usize = 3000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub(crate) struct ResearchReport {
    pub(crate) search_results: Vec<GroundedResult>,
    pub(crate) fetched_pages: Vec<FetchResult>,
    pub(crate) failed_urls: Vec<FailedUrl>,
    pub(crate) all_sources: Vec<Source>,
}

#[derive(Debug)]
pub(crate) struct FailedUrl {
    pub(crate) url: String,
    pub(crate) reason: String,
}

pub(crate) struct ResearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) depth: u8,
    pub(crate) breadth: u8,
    pub(crate) lang: Lang,
}

pub async fn research(
    gemini: &impl SearchClient,
    http: &Client,
    req: &ResearchRequest<'_>,
    resolver: &impl DnsResolver,
) -> Result<ResearchReport, GeminiError> {
    let topical = expand_topical(req.query, req.breadth);
    let queries: Vec<String> = topical
        .iter()
        .flat_map(|q| match req.lang {
            Lang::Auto => expand_bilingual(q),
            _ => vec![req.lang.apply_to_query(q)],
        })
        .collect();

    let search_results = run_searches(gemini, &queries).await?;
    let all_sources = collect_unique_sources(&search_results);

    let urls: Vec<String> = all_sources
        .iter()
        .take(req.depth as usize)
        .map(|s| s.url.clone())
        .collect();

    let (fetched_pages, failed_urls) = fetch_sources(http, urls, resolver).await;

    Ok(ResearchReport {
        search_results,
        fetched_pages,
        failed_urls,
        all_sources,
    })
}

const SEARCH_CONCURRENCY: usize = 3;
const FETCH_CONCURRENCY: usize = 5;

async fn run_searches(
    gemini: &impl SearchClient,
    queries: &[String],
) -> Result<Vec<GroundedResult>, GeminiError> {
    let search_outcomes: Vec<_> = stream::iter(queries)
        .map(|q| gemini.search(q))
        .buffer_unordered(SEARCH_CONCURRENCY)
        .collect()
        .await;

    let (successes, failures): (Vec<_>, Vec<_>) =
        search_outcomes.into_iter().partition(Result::is_ok);

    if successes.is_empty() {
        let first_err = failures
            .into_iter()
            .find_map(Result::err)
            .unwrap_or(GeminiError::RateLimited);
        warn!(
            queries = ?queries,
            error = %first_err,
            "all search queries failed"
        );
        return Err(first_err);
    }

    for e in failures.iter().filter_map(|r| r.as_ref().err()) {
        warn!(error = %e, "partial search failure (continuing with other results)");
    }

    Ok(successes.into_iter().map(Result::unwrap).collect())
}

fn is_transient_fetch(e: &fetch::FetchError) -> bool {
    matches!(
        e,
        fetch::FetchError::Http(re) if retry::is_transient_network(re)
    ) || matches!(
        e,
        fetch::FetchError::Timeout(_)
            | fetch::FetchError::DnsResolution(_)
            | fetch::FetchError::Status(500..=599)
    )
}

async fn fetch_sources(
    http: &Client,
    urls: Vec<String>,
    resolver: &impl DnsResolver,
) -> (Vec<FetchResult>, Vec<FailedUrl>) {
    let fetch_outcomes: Vec<_> = stream::iter(urls)
        .map(|url| async {
            let result = retry_with(
                || async {
                    tokio::time::timeout(
                        FETCH_TIMEOUT,
                        fetch::fetch_page(
                            http,
                            &url,
                            fetch::FetchOptions::default(),
                            resolver,
                        ),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        Err(fetch::FetchError::Timeout(format!(
                            "page fetch timed out after {}s",
                            FETCH_TIMEOUT.as_secs()
                        )))
                    })
                },
                is_transient_fetch,
                || {
                    fetch::FetchError::Timeout("all retries exhausted".into())
                },
            )
            .await;
            (url, result)
        })
        .buffer_unordered(FETCH_CONCURRENCY)
        .collect()
        .await;

    let mut fetched_pages = Vec::new();
    let mut failed_urls = Vec::new();

    for (url, outcome) in fetch_outcomes {
        match outcome {
            Ok(page) => fetched_pages.push(page),
            Err(e) => {
                warn!(url = %url, error = %e, "page fetch failed");
                failed_urls.push(FailedUrl {
                    url,
                    reason: e.to_string(),
                });
            }
        }
    }

    if !failed_urls.is_empty() {
        warn!(
            failed = failed_urls.len(),
            total = failed_urls.len() + fetched_pages.len(),
            "partial page fetch failures"
        );
    }

    (fetched_pages, failed_urls)
}

const MAX_PER_DOMAIN: usize = 2;

fn collect_unique_sources(results: &[GroundedResult]) -> Vec<Source> {
    use crate::search::url::{canonicalize_url, select_diverse_sources};

    let mut seen = std::collections::HashSet::new();
    let mut sources = Vec::new();

    for result in results {
        for source in &result.sources {
            if source.url.is_empty() {
                continue;
            }
            let canonical = canonicalize_url(&source.url);
            if seen.insert(canonical.clone()) {
                sources.push(Source {
                    url: canonical,
                    title: source.title.clone(),
                });
            }
        }
    }

    select_diverse_sources(sources, MAX_PER_DOMAIN)
}

pub fn format_report(report: &ResearchReport, query: &str) -> String {
    let mut out = format!("# Research: {}\n\n", sanitize_heading(query));
    format_search_results(&report.search_results, &mut out);
    format_fetched_pages(&report.fetched_pages, &mut out);
    format_failed_urls(&report.failed_urls, &mut out);
    format_sources(&report.all_sources, &mut out);
    out
}

fn format_search_results(results: &[GroundedResult], out: &mut String) {
    for (i, result) in results.iter().enumerate() {
        if results.len() > 1 {
            let _ = writeln!(out, "## Search Result {}\n", i + 1);
        }
        match &result.answer {
            Some(answer) => out.push_str(answer),
            None => out.push_str(
                "(No answer returned — the query may have been filtered by safety settings.)\n",
            ),
        }
        out.push_str("\n\n");
    }
}

fn format_fetched_pages(pages: &[FetchResult], out: &mut String) {
    if pages.is_empty() {
        return;
    }
    out.push_str("---\n\n## Fetched Pages\n\n");
    for page in pages {
        let _ = writeln!(out, "### {}\n", escape_md_link(&page.url));
        if page.used_raw_fallback {
            out.push_str(fetch::converter::RAW_FALLBACK_NOTE);
        }
        let content = shift_headings(&page.markdown, 3);
        out.push_str(&truncate_with_note(&content, MAX_PAGE_BYTES));
        out.push_str("\n\n");
    }
}

fn format_failed_urls(failed: &[FailedUrl], out: &mut String) {
    if failed.is_empty() {
        return;
    }
    out.push_str("## Failed URLs\n\n");
    for f in failed {
        let _ = writeln!(out, "- {} ({})", escape_md_link(&f.url), f.reason);
    }
    out.push('\n');
}

fn format_sources(sources: &[Source], out: &mut String) {
    if sources.is_empty() {
        return;
    }
    out.push_str("## Sources\n\n");
    for source in sources {
        let _ = writeln!(
            out,
            "- [{}]({})",
            escape_md_link(&source.title),
            escape_md_link(&source.url)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct MockSearch {
        responses: Mutex<VecDeque<Result<GroundedResult, GeminiError>>>,
        queries: Mutex<Vec<String>>,
    }

    impl MockSearch {
        fn with_results(results: Vec<GroundedResult>) -> Self {
            Self {
                responses: Mutex::new(results.into_iter().map(Ok).collect()),
                queries: Mutex::new(Vec::new()),
            }
        }

        fn success_then_failure(first: GroundedResult, failure: GeminiError) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from([Ok(first), Err(failure)])),
                queries: Mutex::new(Vec::new()),
            }
        }

        fn all_fail(error: GeminiError) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from([Err(error)])),
                queries: Mutex::new(Vec::new()),
            }
        }

        fn captured_queries(&self) -> Vec<String> {
            self.queries.lock().unwrap().clone()
        }
    }

    impl SearchClient for MockSearch {
        async fn search(&self, query: &str) -> Result<GroundedResult, GeminiError> {
            self.queries.lock().unwrap().push(query.to_string());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(GeminiError::RateLimited))
        }
    }

    fn make_grounded(sources: Vec<(&str, &str)>) -> GroundedResult {
        GroundedResult {
            answer: Some("test answer".into()),
            sources: sources
                .into_iter()
                .map(|(url, title)| Source {
                    url: url.into(),
                    title: title.into(),
                })
                .collect(),
        }
    }

    #[test]
    fn collect_sources_deduplicates() {
        let results = vec![
            make_grounded(vec![("https://a.com", "A"), ("https://b.com", "B")]),
            make_grounded(vec![("https://a.com", "A"), ("https://c.com", "C")]),
        ];

        let sources = collect_unique_sources(&results);
        assert_eq!(sources.len(), 3);
        // URLs are canonicalized (url crate adds root "/")
        assert_eq!(sources[0].url, "https://a.com/");
        assert_eq!(sources[1].url, "https://b.com/");
        assert_eq!(sources[2].url, "https://c.com/");
    }

    #[test]
    fn collect_sources_skips_empty_urls() {
        let results = vec![make_grounded(vec![("", "Empty"), ("https://a.com", "A")])];

        let sources = collect_unique_sources(&results);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].url, "https://a.com/");
    }

    #[test]
    fn format_report_includes_sections() {
        let report = ResearchReport {
            search_results: vec![make_grounded(vec![("https://a.com", "A")])],
            fetched_pages: vec![],
            failed_urls: vec![FailedUrl {
                url: "https://fail.com".into(),
                reason: "timeout".into(),
            }],
            all_sources: vec![Source {
                url: "https://a.com".into(),
                title: "A".into(),
            }],
        };

        let text = format_report(&report, "test query");
        assert!(text.contains("# Research: test query"));
        assert!(text.contains("test answer"));
        assert!(text.contains("Failed URLs"));
        assert!(text.contains("https://fail.com"));
        assert!(text.contains("Sources"));
        assert!(text.contains("[A](https://a.com)"));
    }

    #[test]
    fn format_report_includes_fetched_pages() {
        let report = ResearchReport {
            search_results: vec![make_grounded(vec![])],
            fetched_pages: vec![FetchResult {
                url: "https://example.com".into(),
                markdown: "# Example Page\n\n## Section\n\nSome content here.".into(),
                used_raw_fallback: false,
            }],
            failed_urls: vec![],
            all_sources: vec![],
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
        assert!(
            !text.contains("\n# Example Page"),
            "original h1 should not remain"
        );
    }

    #[test]
    fn format_report_truncates_long_pages() {
        let long_content = "x".repeat(5000);
        let report = ResearchReport {
            search_results: vec![make_grounded(vec![])],
            fetched_pages: vec![FetchResult {
                url: "https://long.com".into(),
                markdown: long_content,
                used_raw_fallback: false,
            }],
            failed_urls: vec![],
            all_sources: vec![],
        };

        let text = format_report(&report, "test");
        // Verify truncation message includes both shown and total byte counts
        assert!(
            text.contains("(truncated: showing 3000 / 5000 bytes)"),
            "should show exact byte counts, got:\n{text}"
        );
    }

    #[test]
    fn format_report_multiple_search_results_numbered() {
        let report = ResearchReport {
            search_results: vec![
                make_grounded(vec![("https://a.com", "A")]),
                make_grounded(vec![("https://b.com", "B")]),
            ],
            fetched_pages: vec![],
            failed_urls: vec![],
            all_sources: vec![],
        };

        let text = format_report(&report, "test");
        assert!(text.contains("## Search Result 1"));
        assert!(text.contains("## Search Result 2"));
    }

    #[test]
    fn format_report_sanitizes_query_newlines() {
        let report = ResearchReport {
            search_results: vec![make_grounded(vec![])],
            fetched_pages: vec![],
            failed_urls: vec![],
            all_sources: vec![],
        };

        let text = format_report(&report, "line1\nline2");
        assert!(text.contains("# Research: line1 line2"));
        assert!(!text.contains("# Research: line1\n"));
    }

    #[tokio::test]
    async fn research_with_mock_returns_report() {
        let mock = MockSearch::with_results(vec![make_grounded(vec![("https://a.com", "A")])]);
        let http = Client::new();
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "test",
            depth: 3,
            breadth: 1,
            lang: Lang::En,
        };
        let report = research(&mock, &http, &req, &resolver).await.unwrap();

        assert_eq!(report.search_results.len(), 1);
        assert_eq!(report.all_sources.len(), 1);

        let queries = mock.captured_queries();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0], "test (answer in English)");
    }

    #[tokio::test]
    async fn research_partial_search_failure_still_returns() {
        let mock = MockSearch::success_then_failure(
            make_grounded(vec![("https://a.com", "A")]),
            GeminiError::RateLimited,
        );
        let http = Client::new();
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "テスト query",
            depth: 3,
            breadth: 1,
            lang: Lang::Auto,
        };
        let report = research(&mock, &http, &req, &resolver).await.unwrap();

        assert_eq!(report.search_results.len(), 1);

        let queries = mock.captured_queries();
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], "テスト query");
        assert!(queries[1].contains("query"));
    }

    #[tokio::test]
    async fn research_all_searches_fail_returns_error() {
        let mock = MockSearch::all_fail(GeminiError::RateLimited);
        let http = Client::new();
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "test",
            depth: 3,
            breadth: 1,
            lang: Lang::En,
        };
        let err = research(&mock, &http, &req, &resolver).await.unwrap_err();
        assert!(err.to_string().contains("rate limit"));
    }

    #[tokio::test]
    async fn t_006_cross_product_auto_japanese_doubles_queries() {
        let mock = MockSearch::with_results(
            (0..6)
                .map(|i| make_grounded(vec![(&format!("https://{i}.com"), &format!("S{i}"))]))
                .collect(),
        );
        let http = Client::new();
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "WebAssembly セキュリティ",
            depth: 3,
            breadth: 3,
            lang: Lang::Auto,
        };
        let _report = research(&mock, &http, &req, &resolver).await.unwrap();

        let queries = mock.captured_queries();
        assert_eq!(
            queries.len(),
            6,
            "3 topical x 2 bilingual = 6 queries, got: {queries:?}"
        );
        let has_japanese = queries.iter().any(|q| q.contains("セキュリティ"));
        let has_english = queries.iter().any(|q| !q.contains("セキュリティ"));
        assert!(has_japanese, "should include Japanese queries");
        assert!(has_english, "should include English-extracted queries");
    }

    #[tokio::test]
    async fn t_007_cross_product_en_no_bilingual_expansion() {
        let mock = MockSearch::with_results(
            (0..3)
                .map(|i| make_grounded(vec![(&format!("https://{i}.com"), &format!("S{i}"))]))
                .collect(),
        );
        let http = Client::new();
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "WebAssembly security",
            depth: 3,
            breadth: 3,
            lang: Lang::En,
        };
        let _report = research(&mock, &http, &req, &resolver).await.unwrap();

        let queries = mock.captured_queries();
        assert_eq!(
            queries.len(),
            3,
            "3 topical x 1 bilingual = 3 queries, got: {queries:?}"
        );
        for q in &queries {
            assert!(
                q.contains("answer in English"),
                "Lang::En should append instruction, got: {q:?}"
            );
        }
    }

    #[test]
    fn t_019_canonicalize_deduplicates_tracking_param_variants() {
        let url_a = "https://example.com/article?utm_source=twitter&id=42";
        let url_b = "https://example.com/article?utm_source=facebook&id=42";

        let results = vec![
            make_grounded(vec![(url_a, "Article from Twitter")]),
            make_grounded(vec![(url_b, "Article from Facebook")]),
        ];
        let sources = collect_unique_sources(&results);
        assert_eq!(
            sources.len(),
            1,
            "tracking-param-only variants should canonicalize and deduplicate to 1 source, got: {sources:?}"
        );
    }

    #[tokio::test]
    async fn t_008_run_searches_limits_concurrency_to_3() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::sync::Barrier;

        struct ConcurrencyTrackingSearch {
            peak_concurrent: Arc<AtomicUsize>,
            active: Arc<AtomicUsize>,
            barrier: Arc<Barrier>,
        }

        impl ConcurrencyTrackingSearch {
            fn new(concurrency: usize) -> (Self, Arc<AtomicUsize>) {
                let peak = Arc::new(AtomicUsize::new(0));
                let active = Arc::new(AtomicUsize::new(0));
                (
                    Self {
                        peak_concurrent: Arc::clone(&peak),
                        active: Arc::clone(&active),
                        barrier: Arc::new(Barrier::new(concurrency)),
                    },
                    peak,
                )
            }
        }

        impl SearchClient for ConcurrencyTrackingSearch {
            async fn search(&self, _query: &str) -> Result<GroundedResult, GeminiError> {
                let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak_concurrent.fetch_max(current, Ordering::SeqCst);

                self.barrier.wait().await;

                self.active.fetch_sub(1, Ordering::SeqCst);

                Ok(GroundedResult {
                    answer: Some("result".into()),
                    sources: vec![],
                })
            }
        }

        let (mock, peak) = ConcurrencyTrackingSearch::new(SEARCH_CONCURRENCY);
        let queries: Vec<String> = (0..6).map(|i| format!("query {i}")).collect();

        let results = run_searches(&mock, &queries).await.unwrap();

        assert_eq!(results.len(), 6, "all 6 queries should return results");
        assert!(
            peak.load(Ordering::SeqCst) <= 3,
            "peak concurrency should be at most 3, got: {}",
            peak.load(Ordering::SeqCst)
        );
        assert!(
            peak.load(Ordering::SeqCst) > 1,
            "should have some concurrency (>1), got: {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn t_017_is_transient_fetch_true_for_connect_and_timeout() {
        let timeout_err = fetch::FetchError::Timeout("timed out".into());
        assert!(is_transient_fetch(&timeout_err), "Timeout should be transient");

        let dns_err = fetch::FetchError::DnsResolution("lookup failed".into());
        assert!(is_transient_fetch(&dns_err), "DNS resolution failure should be transient");

        let status_502 = fetch::FetchError::Status(502);
        assert!(is_transient_fetch(&status_502), "502 should be transient");

        let status_503 = fetch::FetchError::Status(503);
        assert!(is_transient_fetch(&status_503), "503 should be transient");
    }

    #[test]
    fn t_018_is_transient_fetch_false_for_status_and_scheme() {
        let status_err = fetch::FetchError::Status(404);
        assert!(!is_transient_fetch(&status_err), "404 should not be transient");

        let status_429 = fetch::FetchError::Status(429);
        assert!(!is_transient_fetch(&status_429), "429 should not be transient");

        let scheme_err = fetch::FetchError::InvalidScheme;
        assert!(!is_transient_fetch(&scheme_err), "InvalidScheme should not be transient");

        let content_err = fetch::FetchError::UnsupportedContentType("image/png".into());
        assert!(!is_transient_fetch(&content_err), "UnsupportedContentType should not be transient");

        let too_large = fetch::FetchError::TooLarge;
        assert!(!is_transient_fetch(&too_large), "TooLarge should not be transient");

        let internal = fetch::FetchError::InternalHost;
        assert!(!is_transient_fetch(&internal), "InternalHost should not be transient");
    }
}
