use std::fmt::Write;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use reqwest::Client;
use tokio::time::timeout;
use tracing::warn;

use crate::brave::client::{BraveError, SearchClient};
use crate::brave::types::SearchResult;
use crate::fetch;
use crate::fetch::DnsResolver;
use crate::fetch::converter::FetchResult;
use crate::markdown::{
    escape_md_inline, escape_md_link, sanitize_heading, shift_headings, truncate_with_note,
};
use crate::search::Lang;

const MAX_PAGE_BYTES: usize = 4_500;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Aggregated output of a research session: search hits + their fetched bodies.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ResearchReport {
    pub(crate) fetched_pages: Vec<FetchResult>,
    pub(crate) failed_urls: Vec<FailedUrl>,
    pub(crate) all_sources: Vec<SearchResult>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct FailedUrl {
    pub(crate) url: String,
    pub(crate) reason: String,
}

/// Parameters for a research session (query, depth, language).
pub(crate) struct ResearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) depth: u8,
    pub(crate) lang: Lang,
}

pub(crate) async fn research(
    brave: &impl SearchClient,
    http: &Client,
    req: &ResearchRequest<'_>,
    resolver: &impl DnsResolver,
) -> Result<ResearchReport, BraveError> {
    let search_lang = req.lang.to_brave_param();
    let all_sources = brave.search(req.query, search_lang).await?;

    let urls: Vec<String> = all_sources
        .iter()
        .take(req.depth as usize)
        .map(|s| s.url.clone())
        .collect();

    let (fetched_pages, failed_urls) = fetch_sources(http, urls, resolver).await;

    Ok(ResearchReport {
        fetched_pages,
        failed_urls,
        all_sources,
    })
}

async fn fetch_sources(
    http: &Client,
    urls: Vec<String>,
    resolver: &impl DnsResolver,
) -> (Vec<FetchResult>, Vec<FailedUrl>) {
    let fetch_outcomes: Vec<_> = stream::iter(urls.into_iter().enumerate())
        .map(|(idx, url)| async move {
            let result = timeout(
                FETCH_TIMEOUT,
                fetch::fetch_page(http, &url, fetch::FetchOptions::default(), resolver),
            )
            .await;
            let result = match result {
                Ok(inner) => inner,
                Err(_) => Err(fetch::FetchError::Timeout(format!(
                    "page fetch timed out after {}s",
                    FETCH_TIMEOUT.as_secs()
                ))),
            };
            (idx, url, result)
        })
        // Concurrency cap = 5: balances fetch parallelism (faster overall research)
        // against per-host rate limits (multiple URLs in one query may share an origin).
        .buffer_unordered(5)
        .collect()
        .await;

    let mut indexed_pages = Vec::new();
    let mut failed_urls = Vec::new();

    for (idx, url, outcome) in fetch_outcomes {
        match outcome {
            Ok(page) => indexed_pages.push((idx, page)),
            Err(e) => failed_urls.push(FailedUrl {
                url,
                reason: e.to_string(),
            }),
        }
    }

    indexed_pages.sort_by_key(|(idx, _)| *idx);
    let fetched_pages: Vec<_> = indexed_pages.into_iter().map(|(_, page)| page).collect();

    if !failed_urls.is_empty() && fetched_pages.is_empty() {
        warn!(failed = failed_urls.len(), "all page fetches failed");
    }

    (fetched_pages, failed_urls)
}

pub(crate) fn format_report(report: &ResearchReport, query: &str) -> String {
    let mut out = format!("# Research: {}\n\n", sanitize_heading(query));
    format_fetched_pages(&report.fetched_pages, &mut out);
    format_failed_urls(&report.failed_urls, &mut out);
    format_sources(&report.all_sources, &mut out);
    out
}

fn format_fetched_pages(pages: &[FetchResult], out: &mut String) {
    if pages.is_empty() {
        return;
    }
    out.push_str("---\n\n## Fetched Pages\n\n");
    for page in pages {
        let _ = writeln!(out, "### {}\n", sanitize_heading(&page.url));
        if page.used_raw_fallback {
            out.push_str(fetch::converter::RAW_FALLBACK_NOTE);
        }
        // Shift headings by 3 levels so page content (h1->h4, h2->h5, ...)
        // does not collide with the report's own heading hierarchy.
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
        let _ = writeln!(
            out,
            "- {} ({})",
            escape_md_link(&f.url),
            escape_md_inline(&f.reason)
        );
    }
    out.push('\n');
}

fn format_sources(sources: &[SearchResult], out: &mut String) {
    if sources.is_empty() {
        return;
    }
    out.push_str("## Sources\n\n");
    for source in sources {
        let _ = writeln!(
            out,
            "- [{}]({})",
            escape_md_inline(&source.title),
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
            all_sources: vec![make_source("https://a.com", "A")],
        };

        let text = format_report(&report, "test query");
        assert!(text.contains("# Research: test query"));
        assert!(text.contains("Failed URLs"));
        assert!(text.contains("https://fail.com"));
        assert!(text.contains("Sources"));
        assert!(text.contains("[A](https://a.com)"));
    }

    /// [T-7] AC-3.1: format_report does not emit "## Search Result" header
    #[test]
    fn format_report_omits_search_result_header() {
        let report = ResearchReport {
            fetched_pages: vec![],
            failed_urls: vec![],
            all_sources: vec![make_source("https://a.com", "A")],
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
    }

    /// [T-SE005] format_report truncates long pages with a byte-count note
    #[test]
    fn format_report_truncates_long_pages() {
        let total = MAX_PAGE_BYTES + 2_000;
        let long_content = "x".repeat(total);
        let report = ResearchReport {
            fetched_pages: vec![FetchResult {
                url: "https://long.com".into(),
                markdown: long_content,
                used_raw_fallback: false,
            }],
            failed_urls: vec![],
            all_sources: vec![],
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
            all_sources: vec![],
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
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "test",
            depth: 3,
            lang: Lang::En,
        };
        let report = research(&mock, &http, &req, &resolver).await.unwrap();

        assert_eq!(report.all_sources.len(), 1);

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
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "型安全 TypeScript",
            depth: 3,
            lang: Lang::Auto,
        };
        let _ = research(&mock, &http, &req, &resolver).await.unwrap();

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
        let resolver = fetch::TokioDnsResolver;

        let req = ResearchRequest {
            query: "test",
            depth: 3,
            lang: Lang::En,
        };
        let err = research(&mock, &http, &req, &resolver).await.unwrap_err();
        assert!(matches!(err, BraveError::RateLimited { .. }));
    }

    /// [T-SE011] fetch_sources sort restores input order after buffer_unordered
    #[test]
    fn fetch_sources_sort_restores_input_order() {
        let mut indexed_pages: Vec<(usize, FetchResult)> = vec![
            (
                2,
                FetchResult {
                    url: "https://c.com".into(),
                    markdown: String::new(),
                    used_raw_fallback: false,
                },
            ),
            (
                0,
                FetchResult {
                    url: "https://a.com".into(),
                    markdown: String::new(),
                    used_raw_fallback: false,
                },
            ),
            (
                1,
                FetchResult {
                    url: "https://b.com".into(),
                    markdown: String::new(),
                    used_raw_fallback: false,
                },
            ),
        ];

        indexed_pages.sort_by_key(|(idx, _)| *idx);
        let pages: Vec<_> = indexed_pages.into_iter().map(|(_, page)| page).collect();

        assert_eq!(pages[0].url, "https://a.com");
        assert_eq!(pages[1].url, "https://b.com");
        assert_eq!(pages[2].url, "https://c.com");
    }
}
