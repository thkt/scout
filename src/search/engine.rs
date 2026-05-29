use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use reqwest::Client;
use tokio::sync::watch;
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
    pub(crate) sources: Vec<SearchResult>,
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
    resolver: Arc<dyn DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> Result<ResearchReport, BraveError> {
    let search_lang = req.lang.to_brave_param();
    let sources = brave.search(req.query, search_lang).await?;

    let (fetched_pages, failed_urls) =
        fetch_sources(http, &sources, req.depth as usize, resolver, cancel).await;

    Ok(ResearchReport {
        fetched_pages,
        failed_urls,
        sources,
    })
}

async fn fetch_sources(
    http: &Client,
    sources: &[SearchResult],
    depth: usize,
    resolver: Arc<dyn DnsResolver>,
    cancel: &watch::Sender<bool>,
) -> (Vec<FetchResult>, Vec<FailedUrl>) {
    let fetch_outcomes: Vec<_> = stream::iter(sources.iter().take(depth).enumerate())
        .map(|(idx, source)| {
            let resolver = Arc::clone(&resolver);
            async move {
                let url = source.url.as_str();
                let result = timeout(
                    FETCH_TIMEOUT,
                    fetch::fetch_page(http, url, fetch::FetchOptions::default(), resolver, cancel),
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
            }
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
            Err(e) => {
                warn!(url = %url, error = %e, "page fetch failed");
                failed_urls.push(FailedUrl {
                    url: url.to_owned(),
                    reason: e.to_string(),
                });
            }
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
    format_sources(&report.sources, &mut out);
    out
}

fn format_fetched_pages(pages: &[FetchResult], out: &mut String) {
    if pages.is_empty() {
        return;
    }
    out.push_str("---\n\n## Fetched Pages\n\n");
    for page in pages {
        let _ = writeln!(out, "### {}\n", sanitize_heading(page.url()));
        if page.used_raw_fallback() {
            out.push_str(fetch::converter::RAW_FALLBACK_NOTE);
        }
        // Shift headings by 3 levels so page content (h1->h4, h2->h5, ...)
        // does not collide with the report's own heading hierarchy.
        let content = shift_headings(page.markdown(), 3);
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
mod tests;
