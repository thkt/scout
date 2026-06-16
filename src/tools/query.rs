//! `Scout` query commands: web search, page fetch (including Slack), research.

use tokio::time::timeout;
use tracing::{info, warn};

use crate::brave::client::SearchClient as _;
use crate::envelope::{CommandOutput, Degradation, DegradedReason};
use crate::fetch::converter::FetchResult;
use crate::fetch::{FetchError, FetchOptions, RedactedLogUrl, fetch_page};
use crate::markdown::truncate_with_note;
use crate::search::engine;
use crate::slack::{SlackError, SlackUrl, parse_slack_url};

use super::params::{FetchParams, ResearchParams, SearchParams};
use super::{MAX_FETCH_OUTPUT_BYTES, Scout, ScoutError, format_fetch_output, resolve_stdin_arg};

impl Scout {
    pub(super) async fn search(&self, params: SearchParams) -> Result<CommandOutput, ScoutError> {
        let query = resolve_stdin_arg(params.query, "query", "<QUERY>").await?;

        info!(query = %query, "search");

        let brave = self.brave()?;
        let search_lang = params.lang.to_brave_param();
        let sources = brave.search(&query, search_lang).await?;

        info!(sources = sources.len(), "search complete");

        // Default output: one URL per line, no markdown decoration.
        // OUTCOME.md: AI agents receive raw source URLs without intermediate summary.
        let markdown = sources
            .iter()
            .map(|s| s.url.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let data = serde_json::json!({
            "query": query,
            "sources": sources,
        });
        Ok(CommandOutput::ok(markdown, data))
    }

    pub(super) async fn fetch(&self, params: FetchParams) -> Result<CommandOutput, ScoutError> {
        let url = resolve_stdin_arg(params.url, "url", "<URL>").await?;

        if let Some(slack_url) = parse_slack_url(&url) {
            return self.fetch_slack(slack_url).await;
        }

        info!(url = %RedactedLogUrl(&url), js = params.js, raw = params.raw, "fetch");

        let opts = FetchOptions {
            js: params.js,
            raw: params.raw,
        };
        let fetch_timeout = self.config.fetch_timeout;
        let result = timeout(
            fetch_timeout,
            fetch_page(&self.fetch_http, &url, opts, self.dns.clone(), &self.cancel),
        )
        .await
        .unwrap_or_else(|_| {
            warn!(
                url = %RedactedLogUrl(&url),
                timeout_secs = fetch_timeout.as_secs(),
                "fetch timed out"
            );
            Err(FetchError::Timeout(format!(
                "fetch timed out after {}s",
                fetch_timeout.as_secs()
            )))
        })?;

        if result.used_raw_fallback() {
            warn!(url = %RedactedLogUrl(&url), "readability extraction failed, using raw fallback");
        }

        info!(url = %RedactedLogUrl(&url), "fetch complete");
        let markdown = format_fetch_output(&result);
        let data = serde_json::to_value(&result).expect("FetchResult is Serialize");
        let mut degradation = Degradation::default();
        if result.used_raw_fallback() {
            degradation.push(
                String::from(
                    "Readability extraction failed; raw page conversion was used instead.",
                ),
                DegradedReason::ReadabilityFallback,
            );
        }
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }

    async fn fetch_slack(&self, slack_url: SlackUrl) -> Result<CommandOutput, ScoutError> {
        info!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), "fetch (slack)");
        let client = self.slack().await?;
        let slack_timeout = self.config.slack_timeout;
        let output = timeout(slack_timeout, client.fetch_message(&slack_url))
            .await
            .unwrap_or_else(|_| {
                warn!(
                    workspace = %slack_url.workspace(),
                    channel = %slack_url.channel(),
                    timeout_secs = slack_timeout.as_secs(),
                    "slack fetch timed out"
                );
                Err(SlackError::Timeout(format!(
                    "slack fetch timed out after {}s",
                    slack_timeout.as_secs()
                )))
            })
            .inspect_err(|e| {
                warn!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), error = %e, "slack fetch failed");
            })?;
        info!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), "fetch (slack) complete");
        let markdown = truncate_with_note(&output, MAX_FETCH_OUTPUT_BYTES).into_owned();
        let data = serde_json::json!({
            "url": slack_url.raw_url(),
            "markdown": markdown,
        });
        Ok(CommandOutput::ok(markdown, data))
    }

    pub(super) async fn research(
        &self,
        params: ResearchParams,
    ) -> Result<CommandOutput, ScoutError> {
        let query = resolve_stdin_arg(params.query, "query", "<QUERY>").await?;

        info!(query = %query, depth = params.depth, "research");

        let brave = self.brave()?;
        let req = engine::ResearchRequest {
            query: &query,
            depth: params.depth,
            lang: params.lang,
        };

        let mut degradation = Degradation::default();

        let research_timeout = self.config.research_timeout;
        let report = match timeout(
            research_timeout,
            engine::research(
                brave,
                &self.fetch_http,
                &req,
                self.dns.clone(),
                &self.cancel,
            ),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) if e.is_degradable() => {
                warn!(error = %e, "Brave search failed; returning degraded report");
                degradation.push(
                    format!("Brave search failed: {e}"),
                    DegradedReason::BraveSearchFailed,
                );
                engine::ResearchReport {
                    fetched_pages: vec![],
                    failed_urls: vec![],
                    sources: vec![],
                }
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                warn!(
                    query = %query,
                    depth = params.depth,
                    timeout_secs = research_timeout.as_secs(),
                    "research timed out"
                );
                return Err(ScoutError::timeout(format!(
                    "research timed out after {}s",
                    research_timeout.as_secs()
                )));
            }
        };

        info!(
            pages = report.fetched_pages.len(),
            failed = report.failed_urls.len(),
            sources = report.sources.len(),
            "research complete"
        );

        let markdown = engine::format_report(&report, &query);
        let mut data = serde_json::to_value(&report).expect("ResearchReport is Serialize");
        if let Some(map) = data.as_object_mut() {
            map.insert("query".to_owned(), serde_json::Value::String(query));
        }
        collect_research_degradations(&report, &mut degradation);
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }
}

fn collect_research_degradations(report: &engine::ResearchReport, degradation: &mut Degradation) {
    for f in &report.failed_urls {
        degradation.push(
            format!("Failed to fetch {}: {}", f.url, f.reason),
            DegradedReason::UrlFetchFailed,
        );
    }
    let raw_fallback_pages: Vec<&str> = report
        .fetched_pages
        .iter()
        .filter(|p| p.used_raw_fallback())
        .map(FetchResult::url)
        .collect();
    if !raw_fallback_pages.is_empty() {
        degradation.push(
            format!(
                "Readability extraction failed for: {}",
                raw_fallback_pages.join(", ")
            ),
            DegradedReason::ReadabilityFallback,
        );
    }
}
