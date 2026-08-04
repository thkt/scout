//! Slack message resolution (mention substitution, author/ts lookup) and the
//! YAML-frontmatter + body rendering of a resolved permalink.

use std::collections::HashMap;
use std::fmt::Write;

use serde::Deserialize;
use tracing::{debug, warn};

use super::{SlackUrl, substitute_mentions};
use crate::yaml::{neutralize_yaml_markers, write_yaml_str};

#[derive(Deserialize)]
pub(in crate::slack) struct Message {
    pub(in crate::slack) user: Option<String>,
    #[serde(default)]
    pub(in crate::slack) text: String,
    pub(in crate::slack) ts: Option<String>,
    pub(in crate::slack) reply_count: Option<u32>,
}

pub(in crate::slack) struct ResolvedMessage {
    pub(in crate::slack) author: String,
    pub(in crate::slack) text: String,
    pub(in crate::slack) ts: String,
}

pub(in crate::slack) fn resolve_messages(
    messages: &[Message],
    users: &HashMap<String, String>,
) -> Vec<ResolvedMessage> {
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
pub(in crate::slack) fn extract_target(
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
pub(in crate::slack) fn format_slack_output(
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
mod format_tests;
#[cfg(test)]
mod resolve_messages_tests;
