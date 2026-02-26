use std::fmt::Write;
use std::time::Duration;

use futures::future::join_all;
use futures::stream::{self, StreamExt};
use reqwest::Client;
use tracing::warn;

use crate::fetch;
use crate::fetch::DnsResolver;
use crate::fetch::converter::FetchResult;
use crate::gemini::client::{GeminiError, SearchClient};
use crate::gemini::types::{GroundedResult, Source};
use crate::markdown::{escape_md_link, sanitize_heading};
use crate::search::Lang;
use crate::search::bilingual::expand_bilingual;

const MAX_PAGE_CHARS: usize = 3000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Aggregated output of a multi-source research session.
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

/// Parameters for a research session (query, depth, language).
pub(crate) struct ResearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) depth: u8,
    pub(crate) lang: Lang,
}

pub async fn research(
    gemini: &impl SearchClient,
    http: &Client,
    req: &ResearchRequest<'_>,
    resolver: &impl DnsResolver,
) -> Result<ResearchReport, GeminiError> {
    let queries = match req.lang {
        Lang::Auto => expand_bilingual(req.query),
        _ => vec![req.lang.apply_to_query(req.query)],
    };

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

async fn run_searches(
    gemini: &impl SearchClient,
    queries: &[String],
) -> Result<Vec<GroundedResult>, GeminiError> {
    let search_futures = queries.iter().map(|q| gemini.search(q));
    let search_outcomes = join_all(search_futures).await;

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

    Ok(successes.into_iter().filter_map(Result::ok).collect())
}

async fn fetch_sources(
    http: &Client,
    urls: Vec<String>,
    resolver: &impl DnsResolver,
) -> (Vec<FetchResult>, Vec<FailedUrl>) {
    let fetch_outcomes: Vec<_> = stream::iter(urls)
        .map(|url| async {
            let result = tokio::time::timeout(
                FETCH_TIMEOUT,
                fetch::fetch_page(http, &url, false, true, resolver),
            )
            .await;
            let result = match result {
                Ok(inner) => inner,
                Err(_) => Err(fetch::FetchError::Timeout(format!(
                    "page fetch timed out after {}s",
                    FETCH_TIMEOUT.as_secs()
                ))),
            };
            (url, result)
        })
        .buffer_unordered(5)
        .collect()
        .await;

    let mut fetched_pages = Vec::new();
    let mut failed_urls = Vec::new();

    for (url, outcome) in fetch_outcomes {
        match outcome {
            Ok(page) => fetched_pages.push(page),
            Err(e) => failed_urls.push(FailedUrl {
                url,
                reason: e.to_string(),
            }),
        }
    }

    if !failed_urls.is_empty() && fetched_pages.is_empty() {
        warn!(failed = failed_urls.len(), "all page fetches failed");
    }

    (fetched_pages, failed_urls)
}

fn collect_unique_sources(results: &[GroundedResult]) -> Vec<Source> {
    let mut seen = std::collections::HashSet::new();
    let mut sources = Vec::new();

    for result in results {
        for source in &result.sources {
            if !source.url.is_empty() && seen.insert(source.url.clone()) {
                sources.push(source.clone());
            }
        }
    }

    sources
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
            out.push_str("> Note: Readability extraction failed. Showing raw page conversion.\n\n");
        }
        if page.markdown.len() > MAX_PAGE_CHARS {
            let end = page.markdown.floor_char_boundary(MAX_PAGE_CHARS);
            out.push_str(&page.markdown[..end]);
            out.push_str("...\n\n(truncated)");
        } else {
            out.push_str(&page.markdown);
        }
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
        assert_eq!(sources[0].url, "https://a.com");
        assert_eq!(sources[1].url, "https://b.com");
        assert_eq!(sources[2].url, "https://c.com");
    }

    #[test]
    fn collect_sources_skips_empty_urls() {
        let results = vec![make_grounded(vec![("", "Empty"), ("https://a.com", "A")])];

        let sources = collect_unique_sources(&results);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].url, "https://a.com");
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
                markdown: "# Example Page\n\nSome content here.".into(),
                used_raw_fallback: false,
            }],
            failed_urls: vec![],
            all_sources: vec![],
        };

        let text = format_report(&report, "test");
        assert!(text.contains("Fetched Pages"));
        assert!(text.contains("### https://example.com"));
        assert!(text.contains("Some content here."));
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
        assert!(text.contains("(truncated)"));
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
            lang: Lang::En,
        };
        let err = research(&mock, &http, &req, &resolver).await.unwrap_err();
        assert!(err.to_string().contains("rate limit"));
    }
}
