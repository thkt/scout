use futures::future::join_all;
use futures::stream::{self, StreamExt};
use reqwest::Client;
use tracing::warn;

use crate::fetch;
use crate::fetch::converter::FetchResult;
use crate::gemini::client::{GeminiError, SearchClient};
use crate::gemini::types::{GroundedResult, Source};
use crate::search::bilingual::expand_bilingual;
use crate::tools::Lang;

#[derive(Debug)]
pub struct ResearchReport {
    pub search_results: Vec<GroundedResult>,
    pub fetched_pages: Vec<FetchResult>,
    pub failed_urls: Vec<FailedUrl>,
    pub all_sources: Vec<Source>,
}

#[derive(Debug)]
pub struct FailedUrl {
    pub url: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    #[error("{0}")]
    Gemini(#[from] GeminiError),
}

pub async fn research(
    gemini: &impl SearchClient,
    http: &Client,
    query: &str,
    depth: u8,
    lang: Lang,
) -> Result<ResearchReport, ResearchError> {
    let queries = match lang {
        Lang::Auto => expand_bilingual(query),
        _ => vec![lang.apply_to_query(query)],
    };

    let search_futures = queries.iter().map(|q| gemini.search(q));
    let search_outcomes = join_all(search_futures).await;

    let (successes, failures): (Vec<_>, Vec<_>) =
        search_outcomes.into_iter().partition(Result::is_ok);

    if successes.is_empty() {
        let first_err = failures
            .into_iter()
            .find_map(Result::err)
            .unwrap_or(GeminiError::RateLimited);
        return Err(first_err.into());
    }

    for e in failures.iter().filter_map(|r| r.as_ref().err()) {
        warn!(error = %e, "partial search failure (continuing with other results)");
    }

    let search_results: Vec<_> = successes.into_iter().map(Result::unwrap).collect();

    let all_sources = collect_unique_sources(&search_results);
    let urls: Vec<String> = all_sources
        .iter()
        .take(depth as usize)
        .map(|s| s.url.clone())
        .collect();

    let fetch_outcomes: Vec<_> = stream::iter(urls)
        .map(|url| async {
            let result = fetch::fetch_page(http, &url, false, true).await;
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

    Ok(ResearchReport {
        search_results,
        fetched_pages,
        failed_urls,
        all_sources,
    })
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
    let mut output = format!("# Research: {query}\n\n");

    for (i, result) in report.search_results.iter().enumerate() {
        if report.search_results.len() > 1 {
            output.push_str(&format!("## Search Result {}\n\n", i + 1));
        }
        output.push_str(&result.answer);
        output.push_str("\n\n");
    }

    if !report.fetched_pages.is_empty() {
        output.push_str("---\n\n## Fetched Pages\n\n");
        for page in &report.fetched_pages {
            output.push_str(&format!("### {}\n\n", page.url));
            let content = if page.markdown.len() > 3000 {
                let end = page.markdown.floor_char_boundary(3000);
                format!("{}...\n\n(truncated)", &page.markdown[..end])
            } else {
                page.markdown.clone()
            };
            output.push_str(&content);
            output.push_str("\n\n");
        }
    }

    if !report.failed_urls.is_empty() {
        output.push_str("## Failed URLs\n\n");
        for failed in &report.failed_urls {
            output.push_str(&format!("- {} ({})\n", failed.url, failed.reason));
        }
        output.push('\n');
    }

    if !report.all_sources.is_empty() {
        output.push_str("## Sources\n\n");
        for source in &report.all_sources {
            output.push_str(&format!("- [{}]({})\n", source.title, source.url));
        }
    }

    output
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
            answer: "test answer".into(),
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
        let results = vec![make_grounded(vec![
            ("", "Empty"),
            ("https://a.com", "A"),
        ])];

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

    #[tokio::test]
    async fn research_with_mock_returns_report() {
        let mock = MockSearch::with_results(vec![make_grounded(vec![
            ("https://a.com", "A"),
        ])]);
        let http = Client::new();

        let report = research(&mock, &http, "test", 3, Lang::En).await.unwrap();

        assert_eq!(report.search_results.len(), 1);
        assert_eq!(report.all_sources.len(), 1);

        let queries = mock.captured_queries();
        assert_eq!(queries.len(), 1);
        assert!(queries[0].contains("test"));
    }

    #[tokio::test]
    async fn research_partial_search_failure_still_returns() {
        let mock = MockSearch::success_then_failure(
            make_grounded(vec![("https://a.com", "A")]),
            GeminiError::RateLimited,
        );
        let http = Client::new();

        let report = research(&mock, &http, "テスト query", 3, Lang::Auto)
            .await
            .unwrap();

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

        let err = research(&mock, &http, "test", 3, Lang::En).await.unwrap_err();
        assert!(err.to_string().contains("rate limit"));
    }
}
