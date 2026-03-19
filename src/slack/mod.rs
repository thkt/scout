use std::collections::{HashMap, HashSet};

use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

use crate::redacted::Redacted;

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("SLACK_TOKEN is not set — export a User OAuth token (xoxp-…)")]
    TokenNotSet,

    #[error("Slack API error: {error}")]
    Api { error: String },

    #[error("Slack request failed: {0}")]
    Network(String),

    #[error("Slack fetch timed out: {0}")]
    Timeout(String),

    #[error("Slack response decode error: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct SlackUrl {
    pub workspace: String,
    pub channel: String,
    pub ts: String,
    pub thread_ts: Option<String>,
    pub raw_url: String,
}

/// Parse a Slack message URL into its components.
///
/// Accepts `https://{workspace}.slack.com/archives/{channel}/p{ts_raw}[?thread_ts=…]`.
pub fn parse_slack_url(url: &str) -> Option<SlackUrl> {
    let parsed = url::Url::parse(url).ok()?;
    let workspace = parsed.host_str()?.strip_suffix(".slack.com")?;
    if workspace.is_empty() {
        return None;
    }

    let segments: Vec<&str> = parsed.path_segments()?.collect();
    if segments.len() != 3 || segments[0] != "archives" {
        return None;
    }

    let channel = segments[1].to_string();
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
        workspace: workspace.to_string(),
        channel,
        ts,
        thread_ts,
        raw_url: url.to_string(),
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

struct FetchedThread {
    messages: Vec<Message>,
    is_thread: bool,
}

pub struct SlackClient {
    http: Client,
    token: Redacted,
}

impl SlackClient {
    pub fn from_env(http: Client) -> Result<Self, SlackError> {
        let raw = std::env::var("SLACK_TOKEN").map_err(|_| SlackError::TokenNotSet)?;
        if raw.trim().is_empty() {
            return Err(SlackError::TokenNotSet);
        }
        Ok(Self {
            http,
            token: Redacted::new(raw),
        })
    }

    async fn api_get<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, SlackError> {
        let mut url = url::Url::parse(&format!("https://slack.com/api/{method}"))
            .map_err(|e| SlackError::Network(e.to_string()))?;
        for (k, v) in params {
            url.query_pairs_mut().append_pair(k, v);
        }

        assert!(
            url.scheme() == "https" || cfg!(test),
            "Bearer token must only be sent over HTTPS"
        );

        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token.expose()))
            .send()
            .await
            .map_err(|e| SlackError::Network(e.to_string()))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| {
                if e.is_decode() {
                    SlackError::Decode(e.to_string())
                } else {
                    SlackError::Network(e.to_string())
                }
            })?;

        if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            return Err(SlackError::Api { error });
        }

        serde_json::from_value(body)
            .map_err(|e| SlackError::Decode(e.to_string()))
    }

    async fn resolve_channel(&self, id: &str) -> String {
        match self
            .api_get::<ChannelBody>("conversations.info", &[("channel", id)])
            .await
        {
            Ok(b) => b
                .channel
                .and_then(|c| c.name)
                .map(|n| format!("#{n}"))
                .unwrap_or_else(|| id.to_string()),
            Err(e) => {
                warn!(channel_id = %id, error = %e, "channel resolution failed, using raw ID");
                id.to_string()
            }
        }
    }

    async fn fetch_user_name(&self, id: &str) -> String {
        match self
            .api_get::<UserBody>("users.info", &[("user", id)])
            .await
        {
            Ok(b) => b
                .user
                .and_then(|u| {
                    u.profile
                        .and_then(|p| p.display_name.filter(|n| !n.is_empty()))
                        .or(u.real_name)
                })
                .unwrap_or_else(|| id.to_string()),
            Err(e) => {
                warn!(user_id = %id, error = %e, "user resolution failed, using raw ID");
                id.to_string()
            }
        }
    }

    async fn prefetch_users(&self, ids: &HashSet<String>) -> HashMap<String, String> {
        let ids: Vec<String> = ids.iter().cloned().collect();
        let futs = ids.iter().map(|id| self.fetch_user_name(id));
        let results = futures::future::join_all(futs).await;
        ids.into_iter().zip(results).collect()
    }

    async fn fetch_thread(
        &self,
        slack_url: &SlackUrl,
    ) -> Result<FetchedThread, SlackError> {
        let ch = &slack_url.channel;
        if let Some(ref thread_ts) = slack_url.thread_ts {
            let body: MessagesBody = self
                .api_get(
                    "conversations.replies",
                    &[("channel", ch), ("ts", thread_ts)],
                )
                .await?;
            return Ok(FetchedThread { messages: body.messages, is_thread: true });
        }

        let body: MessagesBody = self
            .api_get(
                "conversations.history",
                &[
                    ("channel", ch),
                    ("latest", &slack_url.ts),
                    ("inclusive", "true"),
                    ("limit", "1"),
                ],
            )
            .await?;
        let has_replies = body
            .messages
            .first()
            .is_some_and(|m| m.reply_count.unwrap_or(0) > 0);
        if has_replies {
            let thread: MessagesBody = self
                .api_get(
                    "conversations.replies",
                    &[("channel", ch), ("ts", &slack_url.ts)],
                )
                .await?;
            Ok(FetchedThread { messages: thread.messages, is_thread: true })
        } else {
            Ok(FetchedThread { messages: body.messages, is_thread: false })
        }
    }

    pub async fn fetch_message(&self, slack_url: &SlackUrl) -> Result<String, SlackError> {
        let fetched = self.fetch_thread(slack_url).await?;
        if fetched.messages.is_empty() {
            return Err(SlackError::Api {
                error: "message not found".into(),
            });
        }

        let mut user_ids = HashSet::new();
        for msg in &fetched.messages {
            if let Some(uid) = &msg.user {
                user_ids.insert(uid.clone());
            }
            collect_mention_ids(&msg.text, &mut user_ids);
        }

        let (channel_name, users) = tokio::join!(
            self.resolve_channel(&slack_url.channel),
            self.prefetch_users(&user_ids),
        );

        let mut resolved = Vec::with_capacity(fetched.messages.len());
        for msg in &fetched.messages {
            let author = match &msg.user {
                Some(uid) => users.get(uid.as_str()).cloned().unwrap_or_else(|| uid.clone()),
                None => "(no author)".into(),
            };
            let text = substitute_mentions(&msg.text, &users);
            let ts = msg.ts.clone().unwrap_or_default();
            resolved.push(ResolvedMessage { author, text, ts });
        }

        let (first, replies) = if fetched.is_thread {
            resolved.split_first().expect("messages verified non-empty")
        } else {
            (&resolved[0], &[] as &[ResolvedMessage])
        };
        let output = format_slack_output(slack_url, &channel_name, first, replies);
        info!(
            workspace = %slack_url.workspace,
            channel = %channel_name,
            replies = replies.len(),
            "slack fetch complete"
        );
        Ok(output)
    }
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
        let Some(rel_end) = text[after..].find('>') else { break };
        let abs_end = after + rel_end + 1;
        let inner = &text[after..after + rel_end];
        let user_id = inner.split('|').next().unwrap_or(inner);
        spans.push(MentionSpan { user_id, start: abs_start, end: abs_end });
        search_from = abs_end;
    }
    spans
}

fn collect_mention_ids(text: &str, ids: &mut HashSet<String>) {
    for span in parse_mentions(text) {
        ids.insert(span.user_id.to_string());
    }
}

fn substitute_mentions(text: &str, cache: &HashMap<String, String>) -> String {
    let spans = parse_mentions(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for span in &spans {
        out.push_str(&text[pos..span.start]);
        out.push('@');
        out.push_str(cache.get(span.user_id).map(|s| s.as_str()).unwrap_or(span.user_id));
        pos = span.end;
    }
    out.push_str(&text[pos..]);
    out
}

fn format_slack_output(
    slack_url: &SlackUrl,
    channel_name: &str,
    first: &ResolvedMessage,
    replies: &[ResolvedMessage],
) -> String {
    let escape = crate::fetch::converter::escape_yaml;

    let mut out = String::from("---\n");
    out.push_str(&format!("workspace: \"{}\"\n", escape(&slack_url.workspace)));
    out.push_str(&format!("channel: \"{}\"\n", escape(channel_name)));
    out.push_str(&format!("author: \"{}\"\n", escape(&first.author)));
    out.push_str(&format!("ts: \"{}\"\n", slack_url.ts));
    if !replies.is_empty() {
        out.push_str(&format!("replies: {}\n", replies.len()));
    }
    out.push_str(&format!("url: \"{}\"\n", escape(&slack_url.raw_url)));
    out.push_str("---\n\n");

    out.push_str(&first.text);

    for msg in replies {
        let ts_suffix = if msg.ts.is_empty() {
            String::new()
        } else {
            format!(" ({})", msg.ts)
        };
        out.push_str(&format!("\n\n---\n\n{}{}:\n{}", msg.author, ts_suffix, msg.text));
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_url() {
        let url = "https://myteam.slack.com/archives/C0656BJSFL7/p1773819598273499";
        let parsed = parse_slack_url(url).unwrap();
        assert_eq!(parsed.workspace, "myteam");
        assert_eq!(parsed.channel, "C0656BJSFL7");
        assert_eq!(parsed.ts, "1773819598.273499");
        assert!(parsed.thread_ts.is_none());
    }

    #[test]
    fn parse_thread_reply_url() {
        let url =
            "https://team.slack.com/archives/C123/p1234567890123456?thread_ts=1234567890.123456&cid=C123";
        let parsed = parse_slack_url(url).unwrap();
        assert_eq!(parsed.channel, "C123");
        assert_eq!(parsed.ts, "1234567890.123456");
        assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
    }

    #[test]
    fn parse_rejects_non_slack_url() {
        assert!(parse_slack_url("https://example.com/page").is_none());
    }

    #[test]
    fn parse_rejects_non_archives_path() {
        assert!(parse_slack_url("https://team.slack.com/messages/C123/p111111222222333").is_none());
    }

    #[test]
    fn parse_rejects_short_timestamp() {
        assert!(parse_slack_url("https://team.slack.com/archives/C123/p12345").is_none());
    }
}
