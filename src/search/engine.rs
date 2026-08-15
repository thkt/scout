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
use crate::fetch::converter::FetchResult;
use crate::fetch::{DnsResolver, EgressMode};
use crate::markdown::{escape_md_inline, md_link, sanitize_heading, shift_headings};
use crate::search::Lang;
use crate::yaml::truncate_and_reneutralize;

const MAX_PAGE_BYTES: usize = 4_500;
/// Per-source cap inside one research run. `pub(crate)` for the same config
/// invariant test that reads `brave::client::REQUEST_TIMEOUT`.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Aggregated output of a research session: search hits + their fetched bodies.
///
/// `Default` is the empty report — a real state, and the one `research` returns
/// when a degradable Brave failure leaves nothing to report. It also lets a test
/// name only the field it is about instead of spelling out the other two as
/// `vec![]`.
#[derive(Debug, Default, serde::Serialize)]
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

pub(crate) struct ResearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) depth: u8,
    pub(crate) lang: Lang,
    /// Carried for the same reason `fetch` carries it (ADR-0023): under a proxy
    /// the local DNS pre-check validates addresses scout never connects to, and
    /// rejects hosts only the proxy can resolve. A `Direct` default here fails
    /// every research source behind a proxy that `fetch` handles fine.
    pub(crate) egress: EgressMode,
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

    let (fetched_pages, failed_urls) = fetch_sources(
        http,
        &sources,
        req.depth as usize,
        &req.egress,
        resolver,
        cancel,
        FETCH_TIMEOUT,
    )
    .await;

    Ok(ResearchReport {
        fetched_pages,
        failed_urls,
        sources,
    })
}

/// `research` passes `FETCH_TIMEOUT` as `source_timeout`; taking the per-source
/// budget as an argument is what lets `T-SE015` reach the timeout arm below
/// without waiting those 15s.
async fn fetch_sources(
    http: &Client,
    sources: &[SearchResult],
    depth: usize,
    egress: &EgressMode,
    resolver: Arc<dyn DnsResolver>,
    cancel: &watch::Sender<bool>,
    source_timeout: Duration,
) -> (Vec<FetchResult>, Vec<FailedUrl>) {
    let fetch_outcomes: Vec<_> = stream::iter(sources.iter().take(depth).enumerate())
        .map(|(idx, source)| {
            let resolver = Arc::clone(&resolver);
            let opts = fetch::FetchOptions {
                egress: egress.clone(),
                ..Default::default()
            };
            async move {
                let url = source.url.as_str();
                let result = timeout(
                    source_timeout,
                    fetch::fetch_page(http, url, opts, resolver, cancel),
                )
                .await;
                let result = match result {
                    Ok(inner) => inner,
                    Err(_) => Err(fetch::FetchError::Timeout(format!(
                        "no response within {}s",
                        source_timeout.as_secs()
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

    let (fetched_pages, failed_urls) = partition_by_rank(fetch_outcomes);

    if !failed_urls.is_empty() && fetched_pages.is_empty() {
        warn!(failed = failed_urls.len(), "all page fetches failed");
    }

    (fetched_pages, failed_urls)
}

/// Split fetch outcomes into the report's two sections, restoring search
/// ranking order in both.
///
/// `buffer_unordered` yields in completion order, so the index captured at
/// dispatch is the only remaining record of where a URL ranked. Both lists are
/// sorted, not just the successes: leaving the failures in completion order
/// makes two runs over the same sources print them differently.
fn partition_by_rank(
    outcomes: Vec<(usize, &str, Result<FetchResult, fetch::FetchError>)>,
) -> (Vec<FetchResult>, Vec<FailedUrl>) {
    let mut indexed_pages = Vec::new();
    let mut indexed_failures = Vec::new();

    for (idx, url, outcome) in outcomes {
        match outcome {
            Ok(page) => indexed_pages.push((idx, page)),
            Err(e) => {
                warn!(url = %url, error = %e, "page fetch failed");
                indexed_failures.push((
                    idx,
                    FailedUrl {
                        url: url.to_owned(),
                        reason: e.to_string(),
                    },
                ));
            }
        }
    }

    indexed_pages.sort_by_key(|(idx, _)| *idx);
    indexed_failures.sort_by_key(|(idx, _)| *idx);

    (
        indexed_pages.into_iter().map(|(_, page)| page).collect(),
        indexed_failures.into_iter().map(|(_, f)| f).collect(),
    )
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
    // `***` renders the same thematic break without being a YAML document
    // marker, so scout's own divider cannot forge one (ADR-0014).
    out.push_str("***\n\n## Fetched Pages\n\n");
    for page in pages {
        let _ = writeln!(out, "### {}\n", sanitize_heading(page.url()));
        if page.used_raw_fallback() {
            out.push_str(fetch::converter::RAW_FALLBACK_NOTE);
        }
        if page.decode_uncertain() {
            out.push_str(fetch::converter::DECODE_UNCERTAIN_NOTE);
        }
        // h1->h4, h2->h5, ...: unshifted, a page's own headings would collide
        // with the report's hierarchy.
        let content = shift_headings(page.markdown(), 3);
        out.push_str(&truncate_and_reneutralize(&content, MAX_PAGE_BYTES));
        out.push_str("\n\n");
    }
}

/// Unlike `## Sources`, these URLs are rendered as text rather than through
/// [`md_link`]: the fetch behind each one already failed, so offering it as
/// something to follow points the reader at the failure again.
///
/// Both halves take the same escape. `escape_md_link` does not fit the URL
/// here: it leaves `|` alone because a link target has no column to break, and
/// nothing on this line sits inside a link.
fn format_failed_urls(failed: &[FailedUrl], out: &mut String) {
    if failed.is_empty() {
        return;
    }
    out.push_str("## Failed URLs\n\n");
    for f in failed {
        let _ = writeln!(
            out,
            "- {} ({})",
            escape_md_inline(&f.url),
            escape_md_inline(&f.reason)
        );
    }
    out.push('\n');
}

/// Unlike the two sections above, this one is emitted even when empty: a
/// report that found nothing has to be distinguishable from one whose sections
/// went missing, so ADR-0005 marks the zero-result case explicitly rather than
/// dropping the heading. `search` takes the opposite contract (ADR-0020 pins it
/// to true empty output) because its consumers read it line by line.
fn format_sources(sources: &[SearchResult], out: &mut String) {
    out.push_str("## Sources\n\n");
    if sources.is_empty() {
        out.push_str("(no results)\n");
        return;
    }
    for source in sources {
        let _ = writeln!(out, "- {}", md_link(&source.title, &source.url));
    }
}

#[cfg(test)]
mod tests;
