//! Slack message URL parsing, error classification, wire-format structs, and
//! YAML output formatting. The token-bearing HTTP client lives in [`client`].

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use tracing::{debug, warn};

use crate::envelope::ErrorCode;
use crate::fetch::converter::{escape_yaml, neutralize_yaml_markers};
use crate::tools::Classification;

mod client;
pub(crate) use client::SlackClient;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SlackError {
    #[error("SLACK_TOKEN is not set — export a User OAuth token (xoxp-…)")]
    TokenNotSet,

    #[error("Slack API error: {error}")]
    Api { error: String },

    #[error("Slack API rate limit exceeded")]
    RateLimited { retry_after: Option<u64> },

    #[error("Slack request failed: {0}")]
    Network(String),

    #[error("Slack fetch timed out: {0}")]
    Timeout(String),

    #[error("Slack response decode error: {0}")]
    Decode(String),

    #[error("Insecure URL: HTTPS required for token-bearing request")]
    InsecureUrl,
}

impl SlackError {
    /// Map each variant to its ADR-0011 priority-table [`Classification`].
    ///
    /// Slack surfaces failures as `error` strings inside `Api` instead of HTTP
    /// status codes, so the string-table arm replaces the HTTP-status arm used
    /// by other backends. The table's `internal_error` / `service_unavailable`
    /// / `fatal_error` entries are load-bearing — without them those codes
    /// would fall through to UsageError instead of the retryable TempFailure.
    pub(crate) fn classify(&self) -> Classification {
        match self {
            // Priority 1: USAGE_ERROR
            Self::TokenNotSet => Classification::new(ErrorCode::UsageError)
                .with_hint("Export a User OAuth token to SLACK_TOKEN (xoxp-…)"),
            // Priority 2: DATA_ERROR (insecure URL — peer to BraveError::InsecureBaseUrl)
            Self::InsecureUrl => Classification::new(ErrorCode::DataError),
            Self::Api { error } => match error.as_str() {
                // Priority 3: NOT_FOUND. Underscore forms are Slack-native error
                // codes; "message not found" (space) is scout's own string from
                // `fetch_message` when the resolved messages list is empty.
                "channel_not_found" | "message_not_found" | "thread_not_found"
                | "message not found" => Classification::new(ErrorCode::NotFound),
                // Priority 4: TEMP_FAILURE
                "internal_error" | "service_unavailable" | "fatal_error" => {
                    Classification::transient_retry()
                }
                // Priority 1: USAGE_ERROR (invalid_auth, missing_scope, etc.)
                _ => Classification::new(ErrorCode::UsageError),
            },
            // Priority 4: TEMP_FAILURE
            Self::RateLimited { .. } => Classification::transient_retry(),
            Self::Network(_) => Classification::transient_network(),
            // Priority 4: TIMEOUT
            Self::Timeout(_) => Classification::timeout_retry(),
            // Priority 5: INTERNAL — scout-side bug (unexpected schema)
            Self::Decode(_) => Classification::new(ErrorCode::Internal),
        }
    }
}

/// Parsed Slack message URL. Fields are private so the only construction path
/// is [`parse_slack_url`]; this guarantees `workspace`/`channel`/`ts` carry the
/// shape that path established (non-empty workspace, `<secs>.<micros>` ts).
#[derive(Debug, Clone)]
pub(crate) struct SlackUrl {
    workspace: String,
    channel: String,
    ts: String,
    thread_ts: Option<String>,
    raw_url: String,
}

impl SlackUrl {
    pub(crate) fn workspace(&self) -> &str {
        &self.workspace
    }

    pub(crate) fn channel(&self) -> &str {
        &self.channel
    }

    pub(crate) fn raw_url(&self) -> &str {
        &self.raw_url
    }
}

/// Parse a Slack message URL into its components.
///
/// Accepts `https://{workspace}.slack.com/archives/{channel}/p{ts_raw}[?thread_ts=…]`.
pub(crate) fn parse_slack_url(url: &str) -> Option<SlackUrl> {
    let parsed = url::Url::parse(url).ok()?;
    let workspace = parsed.host_str()?.strip_suffix(".slack.com")?;
    if workspace.is_empty() {
        return None;
    }

    let segments: Vec<&str> = parsed.path_segments()?.collect();
    if segments.len() != 3 || segments[0] != "archives" {
        return None;
    }

    let channel = segments[1].to_owned();
    // Slack timestamps: p{epoch_secs}{6-digit micros} → "{epoch_secs}.{micros}"
    const TS_MICROS_DIGITS: usize = 6;
    let ts_raw = segments[2].strip_prefix('p')?;
    if ts_raw.len() <= TS_MICROS_DIGITS {
        return None;
    }
    let (secs, micros) = ts_raw.split_at(ts_raw.len() - TS_MICROS_DIGITS);
    let ts = format!("{secs}.{micros}");

    let thread_ts = parsed
        .query_pairs()
        .find(|(k, _)| k == "thread_ts")
        .map(|(_, v)| v.into_owned());

    Some(SlackUrl {
        workspace: workspace.to_owned(),
        channel,
        ts,
        thread_ts,
        raw_url: url.to_owned(),
    })
}

#[derive(Deserialize)]
struct MessagesBody {
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    user: Option<String>,
    #[serde(default)]
    text: String,
    ts: Option<String>,
    reply_count: Option<u32>,
}

#[derive(Deserialize)]
struct ChannelBody {
    channel: Option<ChannelInfo>,
}

#[derive(Deserialize)]
struct ChannelInfo {
    name: Option<String>,
}

#[derive(Deserialize)]
struct UserBody {
    user: Option<UserDetail>,
}

#[derive(Deserialize)]
struct UserDetail {
    real_name: Option<String>,
    profile: Option<Profile>,
}

#[derive(Deserialize)]
struct Profile {
    display_name: Option<String>,
}

struct ResolvedMessage {
    author: String,
    text: String,
    ts: String,
}

/// A `<@UID>` or `<@UID|label>` mention span within a text.
struct MentionSpan<'a> {
    user_id: &'a str,
    /// Byte range covering the entire `<@…>` token.
    start: usize,
    end: usize,
}

fn parse_mentions(text: &str) -> Vec<MentionSpan<'_>> {
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find("<@") {
        let abs_start = search_from + rel;
        let after = abs_start + 2;
        let Some(rel_end) = text[after..].find('>') else {
            break;
        };
        let abs_end = after + rel_end + 1;
        let inner = &text[after..after + rel_end];
        let user_id = inner.split('|').next().unwrap_or(inner);
        spans.push(MentionSpan {
            user_id,
            start: abs_start,
            end: abs_end,
        });
        search_from = abs_end;
    }
    spans
}

fn collect_mention_ids(text: &str, ids: &mut HashSet<String>) {
    for span in parse_mentions(text) {
        ids.insert(span.user_id.to_owned());
    }
}

fn resolve_messages(messages: &[Message], users: &HashMap<String, String>) -> Vec<ResolvedMessage> {
    let mut resolved = Vec::with_capacity(messages.len());
    for msg in messages {
        let author = match &msg.user {
            Some(uid) => users
                .get(uid.as_str())
                .cloned()
                .unwrap_or_else(|| uid.clone()),
            None => {
                debug!("msg.user is None, falling back to \"(no author)\"");
                "(no author)".into()
            }
        };
        let text = substitute_mentions(&msg.text, users);
        let ts = match &msg.ts {
            Some(t) => t.clone(),
            None => {
                warn!("msg.ts is None, falling back to empty string");
                String::new()
            }
        };
        resolved.push(ResolvedMessage { author, text, ts });
    }
    resolved
}

fn substitute_mentions(text: &str, cache: &HashMap<String, String>) -> String {
    let spans = parse_mentions(text);
    if spans.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for span in &spans {
        out.push_str(&text[pos..span.start]);
        out.push('@');
        out.push_str(
            cache
                .get(span.user_id)
                .map(String::as_str)
                .unwrap_or(span.user_id),
        );
        pos = span.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// Extract the message matching `target_ts` from `messages`, returning it and
/// the remaining messages in their original order.
fn extract_target(
    mut messages: Vec<ResolvedMessage>,
    target_ts: &str,
    is_thread: bool,
) -> Option<(ResolvedMessage, Vec<ResolvedMessage>)> {
    let idx = if is_thread {
        messages.iter().position(|m| m.ts == target_ts)?
    } else {
        0
    };
    let first = messages.remove(idx);
    Some((first, messages))
}

fn format_slack_output(
    slack_url: &SlackUrl,
    channel_name: &str,
    first: &ResolvedMessage,
    replies: &[ResolvedMessage],
) -> String {
    let escape = escape_yaml;

    let mut out = String::from("---\n");
    out.push_str(&format!(
        "workspace: \"{}\"\n",
        escape(&slack_url.workspace)
    ));
    out.push_str(&format!("channel: \"{}\"\n", escape(channel_name)));
    out.push_str(&format!("author: \"{}\"\n", escape(&first.author)));
    out.push_str(&format!("ts: \"{}\"\n", escape(&slack_url.ts)));
    if !replies.is_empty() {
        out.push_str(&format!("context_messages: {}\n", replies.len()));
    }
    out.push_str(&format!("url: \"{}\"\n", escape(&slack_url.raw_url)));
    out.push_str("---\n\n");

    out.push_str(&neutralize_yaml_markers(&first.text));

    for msg in replies {
        let ts_suffix = if msg.ts.is_empty() {
            String::new()
        } else {
            format!(" ({})", msg.ts)
        };
        out.push_str(&format!(
            "\n\n---\n\n{}{}:\n{}",
            neutralize_yaml_markers(&msg.author),
            ts_suffix,
            neutralize_yaml_markers(&msg.text)
        ));
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod mention_tests;
#[cfg(test)]
mod resolve_messages_tests;
#[cfg(test)]
mod url_tests;
