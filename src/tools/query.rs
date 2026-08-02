//! `Scout` query commands: web search, page fetch (including Slack), research.

use std::borrow::Cow;

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
            egress: self.egress.clone(),
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
        let data = to_data_value(&result, "fetch result")?;
        let mut degradation = Degradation::default();
        if result.used_raw_fallback() {
            degradation.push(
                String::from(
                    "Readability extraction failed; raw page conversion was used instead.",
                ),
                DegradedReason::ReadabilityFallback,
            );
        }
        if result.decode_uncertain() {
            degradation.push(
                String::from(
                    "Character encoding could not be determined; the body is a best-effort decode and may be garbled. Do not trust it as a faithful primary source.",
                ),
                DegradedReason::DecodeUncertain,
            );
        }
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }

    async fn fetch_slack(&self, slack_url: SlackUrl) -> Result<CommandOutput, ScoutError> {
        info!(workspace = %slack_url.workspace(), channel = %slack_url.channel(), "fetch (slack)");
        let client = self.slack().await?;
        let slack_timeout = self.config.slack_timeout;
        let outcome = timeout(slack_timeout, client.fetch_message(&slack_url))
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

        // Truncate first, then prepend the cap preamble: the preamble must
        // survive the 100KB cut, so it is added after truncation rather than
        // counted toward the limit (issue #222 Finding A). The inline byte-count
        // note from `truncate_with_note` lands at the body end and covers the
        // output-truncation case on the Markdown side, so only thread/users caps
        // go into the preamble to avoid double-reporting truncation.
        let truncated = truncate_with_note(&outcome.markdown, MAX_FETCH_OUTPUT_BYTES);
        let output_truncated = matches!(truncated, Cow::Owned(_));

        let mut degradation = Degradation::default();
        let mut preamble_notes: Vec<&str> = Vec::new();
        if outcome.thread_truncated {
            let note = "Thread truncated: reply page cap reached, some replies are omitted.";
            preamble_notes.push(note);
            degradation.push(note.to_owned(), DegradedReason::SlackThreadTruncated);
        }
        if outcome.users_capped {
            let note = "User lookups were capped, so some authors and mentions show raw IDs.";
            preamble_notes.push(note);
            degradation.push(note.to_owned(), DegradedReason::SlackUsersCapped);
        }
        if output_truncated {
            degradation.push(
                "Output truncated at the size cap; trailing content is omitted.".to_owned(),
                DegradedReason::SlackOutputTruncated,
            );
        }

        // `truncate_with_note` borrows `outcome.markdown` when the body is under
        // the cap (the common case), so move it out rather than clone; only the
        // over-cap path owns a freshly truncated copy.
        let body = match truncated {
            Cow::Owned(s) => s,
            Cow::Borrowed(_) => outcome.markdown,
        };
        let markdown = insert_preamble_notes(body, &preamble_notes);
        let data = serde_json::json!({
            "url": slack_url.raw_url(),
            "markdown": markdown,
        });
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
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
            egress: self.egress.clone(),
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
        let mut data = to_data_value(&report, "research report")?;
        if let Some(map) = data.as_object_mut() {
            map.insert("query".to_owned(), serde_json::Value::String(query));
        }
        collect_research_degradations(&report, &mut degradation);
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }
}

/// Serialize a handler's scout-owned result into the envelope `data` value,
/// mapping a `serde_json` failure to `ScoutError::internal_bug` (exit 70) so it
/// flows through the JSON error envelope via `?` instead of `.expect()`
/// panicking and bypassing it (issue #192). `what` names the value for the
/// error message. The single serialize-to-`data` point shared by `fetch` and
/// `research`.
pub(super) fn to_data_value<T: serde::Serialize>(
    value: &T,
    what: &str,
) -> Result<serde_json::Value, ScoutError> {
    serde_json::to_value(value)
        .map_err(|e| ScoutError::internal_bug(format!("failed to serialize {what}: {e}")))
}

/// Insert cap notes as a Markdown blockquote right after the Slack frontmatter
/// so they reach the agent even when the body is truncated. `format_slack_output`
/// opens with `---\n` then frontmatter then `---\n\n`, so the frontmatter
/// terminator is the FIRST `---\n\n`; reply separators (`\n\n---\n\n`) come later
/// in the body. Inserting after the first occurrence keeps the preamble ahead of
/// any reply and above the truncation point. A reply-less single message has only
/// one `---\n\n`, which is still the terminator, so this stays correct. Returns
/// the input unchanged when there are no notes (no cap fired).
pub(super) fn insert_preamble_notes(markdown: String, notes: &[&str]) -> String {
    const FRONTMATTER_END: &str = "---\n\n";
    if notes.is_empty() {
        return markdown;
    }
    let preamble = format!("> Note: {}\n\n", notes.join(" "));
    match markdown.find(FRONTMATTER_END) {
        Some(idx) => {
            let insert_at = idx + FRONTMATTER_END.len();
            let mut out = String::with_capacity(markdown.len() + preamble.len());
            out.push_str(&markdown[..insert_at]);
            out.push_str(&preamble);
            out.push_str(&markdown[insert_at..]);
            out
        }
        None => format!("{preamble}{markdown}"),
    }
}

pub(super) fn collect_research_degradations(
    report: &engine::ResearchReport,
    degradation: &mut Degradation,
) {
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
    let decode_uncertain_pages: Vec<&str> = report
        .fetched_pages
        .iter()
        .filter(|p| p.decode_uncertain())
        .map(FetchResult::url)
        .collect();
    if !decode_uncertain_pages.is_empty() {
        degradation.push(
            format!(
                "Character encoding could not be determined for: {}. The body is a best-effort decode and may be garbled.",
                decode_uncertain_pages.join(", ")
            ),
            DegradedReason::DecodeUncertain,
        );
    }
}
