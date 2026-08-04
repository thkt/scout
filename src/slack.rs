//! Slack message URL parsing, error classification, wire-format structs, and
//! YAML output formatting. The token-bearing HTTP client lives in [`client`].

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use serde::Deserialize;
use tracing::{debug, warn};

use crate::classify::Classification;
use crate::envelope::ErrorCode;
use crate::yaml::{neutralize_yaml_markers, write_yaml_str};

mod client;
pub(crate) use client::SlackClient;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SlackError {
    #[error("SLACK_TOKEN is not set — export a User OAuth token (xoxp-…)")]
    TokenNotSet,

    /// `SLACK_TOKEN` is set but is not a User OAuth token — it does not begin
    /// with the `xoxp-` prefix. A bot token (`xoxb-…`) or arbitrary string would
    /// otherwise pass construction and fail later with an opaque API error
    /// (issue #261). The contract the `TokenNotSet` hint promises is enforced
    /// at construction by [`client::SlackClient::from_env_with`].
    #[error("SLACK_TOKEN must be a User OAuth token (xoxp-…)")]
    TokenWrongType,

    #[error("Slack API error: {error}")]
    Api { error: String },

    #[error("Slack API rate limit exceeded")]
    RateLimited { retry_after: Option<u64> },

    /// A non-2xx status that is not 429. The body is whatever the responder
    /// produced — a gateway's HTML error page, say — so it is never a Slack
    /// API envelope and must not reach the JSON parse.
    ///
    /// ADR-0003 requires an API-specific reclassification to say so here: this
    /// variant does NOT follow the shared HTTP-status table. Slack reports its
    /// own failures as `ok: false` inside a 200 body, so any non-2xx came from
    /// something between scout and Slack. Reading such a status as Slack's
    /// answer would report a gateway's 404 as a missing resource; every status
    /// is treated as a transient intermediary fault instead.
    #[error("Slack API returned HTTP {0}")]
    Server(u16),

    #[error("Slack request failed: {0}")]
    Network(#[source] reqwest::Error),

    /// URL construction failure inside `client::api_get_once`. Unlike
    /// `BraveError::ParseUrl` (DataError — Brave's `base_url` is caller-supplied),
    /// Slack's `base_url` is a `const` (`client::API_BASE`), so this arm is
    /// unreachable in production; a hit here is a scout-side bug, not a
    /// user-facing data problem.
    #[error("Invalid Slack API URL: {0}")]
    ParseUrl(#[from] url::ParseError),

    #[error("Slack fetch timed out: {0}")]
    Timeout(String),

    #[error("Slack response decode error: {0}")]
    Decode(String),

    #[error("Insecure URL: HTTPS required for token-bearing request")]
    InsecureUrl,
}

/// Hand-written (not `#[from]`) so the conversion strips the request URL:
/// reqwest's `Display` appends `for url (…)` including the query string.
/// Classification flags (`is_timeout()` etc.) survive `without_url`.
impl From<reqwest::Error> for SlackError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.without_url())
    }
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
            Self::TokenNotSet | Self::TokenWrongType => Classification::new(ErrorCode::UsageError)
                .with_hint("Export a User OAuth token to SLACK_TOKEN (xoxp-…)"),
            // Priority 2: DATA_ERROR (insecure URL — peer to BraveError::InsecureBaseUrl)
            Self::InsecureUrl => Classification::new(ErrorCode::DataError),
            Self::Api { error } => match error.as_str() {
                // Priority 3: NOT_FOUND. Underscore forms are Slack-native error
                // codes; the space forms are scout's own strings from
                // `fetch_message`: bare "message not found" (resolved list empty)
                // and "message {ts} not found in thread" (target absent or in a
                // truncated page). The latter interpolates `{ts}`, so it can't be
                // exact-matched — the `starts_with`/`contains` guard catches the
                // whole "message … not found …" family (issue #224). Slack-native
                // codes are snake_case and never start with "message " (space),
                // so they fall through to their own arms below.
                "channel_not_found" | "message_not_found" | "thread_not_found" => {
                    Classification::new(ErrorCode::NotFound)
                }
                s if s.starts_with("message ") && s.contains("not found") => {
                    Classification::new(ErrorCode::NotFound)
                }
                // Priority 4: TEMP_FAILURE
                "internal_error" | "service_unavailable" | "fatal_error" => {
                    Classification::transient_retry()
                }
                // Priority 1: USAGE_ERROR (invalid_auth, missing_scope, etc.)
                _ => Classification::new(ErrorCode::UsageError),
            },
            // Priority 4: TEMP_FAILURE
            Self::RateLimited { .. } | Self::Server(_) => Classification::transient_retry(),
            // Priority 4 (TIMEOUT) and 退避: see `Classification::from_reqwest`
            Self::Network(re) => Classification::from_reqwest(re),
            // Priority 4: TIMEOUT
            Self::Timeout(_) => Classification::timeout_retry(),
            // Priority 5: INTERNAL — scout-side bug (unexpected schema / URL build failure)
            Self::Decode(_) | Self::ParseUrl(_) => Classification::new(ErrorCode::Internal),
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
    #[serde(default)]
    has_more: bool,
    response_metadata: Option<ResponseMetadata>,
}

impl MessagesBody {
    /// The non-empty `next_cursor` to fetch the following page, if Slack
    /// signalled more results.
    fn next_cursor(&self) -> Option<&str> {
        if !self.has_more {
            return None;
        }
        self.response_metadata
            .as_ref()
            .and_then(|m| m.next_cursor.as_deref())
            .filter(|c| !c.is_empty())
    }
}

#[derive(Deserialize)]
struct ResponseMetadata {
    next_cursor: Option<String>,
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
    /// Human-readable label Slack embedded as `<@UID|label>`, `None` when absent
    /// or empty. Used as a best-effort render fallback when the user id is
    /// unresolved; it is the send-time display name and may be stale.
    label: Option<&'a str>,
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
        let (user_id, label) = match inner.split_once('|') {
            Some((id, label)) => (id, Some(label).filter(|l| !l.is_empty())),
            None => (inner, None),
        };
        spans.push(MentionSpan {
            user_id,
            label,
            start: abs_start,
            end: abs_end,
        });
        search_from = abs_end;
    }
    spans
}

/// Append mention IDs from `text` to `out` in first-occurrence order, skipping
/// any already in `seen`. Sharing `seen` across calls (and with an author pass)
/// dedupes a dual-role ID so it consumes a single lookup slot.
fn collect_mention_ids_ordered(text: &str, seen: &mut HashSet<String>, out: &mut Vec<String>) {
    for span in parse_mentions(text) {
        if seen.insert(span.user_id.to_owned()) {
            out.push(span.user_id.to_owned());
        }
    }
}

fn resolve_messages(messages: &[Message], users: &HashMap<String, String>) -> Vec<ResolvedMessage> {
    let mut resolved = Vec::with_capacity(messages.len());
    for msg in messages {
        let author = match &msg.user {
            // An empty value is a failed resolution, not a name — the same rule
            // `substitute_mentions` applies to this map. Without the filter the
            // frontmatter carries `author: ""` and no log says the lookup missed.
            Some(uid) => users
                .get(uid.as_str())
                .filter(|name| !name.is_empty())
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
                .filter(|name| !name.is_empty())
                .or(span.label)
                .unwrap_or(span.user_id),
        );
        pos = span.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// Extract the message matching `target_ts` from `messages`, returning it and
/// the remaining messages in their original order.
///
/// The match is by ts for a channel fetch too, not just within a thread:
/// `conversations.history` is probed with `latest` as an *upper* bound, so a ts
/// that no longer exists answers with the preceding message instead of an empty
/// list. Returning `None` lets the caller report a miss, rather than rendering a
/// neighbour's author and body under the ts the caller asked for.
fn extract_target(
    mut messages: Vec<ResolvedMessage>,
    target_ts: &str,
) -> Option<(ResolvedMessage, Vec<ResolvedMessage>)> {
    let idx = messages.iter().position(|m| m.ts == target_ts)?;
    let first = messages.remove(idx);
    Some((first, messages))
}

/// Render a resolved Slack permalink as YAML-frontmatter + body, the stable
/// output schema agent consumers parse.
///
/// Frontmatter keys are emitted in a fixed order: `workspace`, `channel`,
/// `author`, `ts`, then `context_messages` only when `replies` is non-empty
/// (omitted, not zero, so a parser feature-detects threads via key presence),
/// then `url`. Every frontmatter value flows through `escape_yaml`, and every
/// body segment through `neutralize_yaml_markers`, so a message whose text
/// contains a line `---` or `key: value` cannot break out of the body and forge
/// frontmatter the consumer would trust (output-injection defense, ADR-0014).
/// Reply blocks are separated by a `---` line and prefix the author (and `ts`
/// when present) before the neutralized text.
fn format_slack_output(
    slack_url: &SlackUrl,
    channel_name: &str,
    first: &ResolvedMessage,
    replies: &[ResolvedMessage],
) -> String {
    let mut out = String::from("---\n");
    write_yaml_str(&mut out, "workspace", &slack_url.workspace);
    write_yaml_str(&mut out, "channel", channel_name);
    write_yaml_str(&mut out, "author", &first.author);
    write_yaml_str(&mut out, "ts", &slack_url.ts);
    if !replies.is_empty() {
        // Numeric, so it is written unquoted — the one key here that is not a
        // string scalar, rather than a fifth look-alike.
        let _ = writeln!(out, "context_messages: {}", replies.len());
    }
    write_yaml_str(&mut out, "url", &slack_url.raw_url);
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
