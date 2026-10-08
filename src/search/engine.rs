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
use crate::markdown::{escape_md_inline, md_link, sanitize_heading, truncate_with_note};
use crate::search::Lang;
use crate::yaml::{ReportBody, finish_report_body};

/// `pub(crate)` because `yaml::MAX_FIELD_BYTES` derives the per-field
/// frontmatter cap from this same page budget.
pub(crate) const MAX_PAGE_BYTES: usize = 4_500;
/// Per-source timeout; exposed for the config invariant test.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Research hits and fetched bodies. Default is an empty report; callers add degradation.
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
    /// Forward proxy egress (ADR-0023) so the pre-check does not reject domains
    /// that only the proxy can resolve.
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

/// Inject the source timeout so T-SE015 need not wait the production budget.
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
        // Bound concurrency because several results may share an origin.
        .buffer_unordered(5)
        .collect()
        .await;

    let (fetched_pages, failed_urls) = partition_by_rank(fetch_outcomes);

    if !failed_urls.is_empty() && fetched_pages.is_empty() {
        warn!(failed = failed_urls.len(), "all page fetches failed");
    }

    (fetched_pages, failed_urls)
}

/// Restore search rank after unordered completion, for successes and failures
/// alike, so timing cannot reorder report sections.
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
    // Use a thematic break that cannot forge a YAML boundary (DR-0014).
    out.push_str("***\n\n## Fetched Pages\n\n");
    for page in pages {
        let _ = writeln!(out, "### {}\n", sanitize_heading(page.url()));
        if page.used_raw_fallback() {
            out.push_str(fetch::converter::RAW_FALLBACK_NOTE);
        }
        if page.decode_uncertain() {
            out.push_str(fetch::converter::DECODE_UNCERTAIN_NOTE);
        }
        // Nest HTML headings under the page heading; preserve plain-text literals.
        let content = page.with_heading_offset(3);
        let body = truncate_with_note(&content, MAX_PAGE_BYTES);
        out.push_str(&finish_report_body(&body, ReportBody::Fetched));
        out.push_str("\n\n");
    }
}

/// Failed URLs are text, not link targets; both URL and reason require inline
/// escaping. Link-destination escaping would leave `|` untouched.
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

/// Always emit the zero-result marker (ADR-0005), distinguishing an empty
/// report from missing sections. Search has a different empty-output contract
/// (ADR-0020).
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
