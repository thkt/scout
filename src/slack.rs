//! Slack message URL parsing, error classification, wire-format structs, and
//! YAML output formatting. The token-bearing HTTP client lives in [`client`].

use std::collections::HashMap;
use std::fmt::Write;

use serde::Deserialize;
use tracing::{debug, warn};

use crate::classify::Classification;
use crate::envelope::ErrorCode;
use crate::yaml::{neutralize_yaml_markers, write_yaml_str};

mod client;
pub(crate) use client::SlackClient;

mod mention;
pub(in crate::slack) use mention::{collect_mention_ids_ordered, substitute_mentions};

mod url;
pub(crate) use url::{SlackUrl, parse_slack_url};

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
    // Qualified `::url` (crate root), not `url`: the local `mod url` declared
    // above shadows the `url` crate name within this module's path resolution.
    ParseUrl(#[from] ::url::ParseError),

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
mod resolve_messages_tests;
#[cfg(test)]
mod url_tests;
