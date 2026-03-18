use std::collections::HashMap;

use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

// --- Error ---

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("SLACK_TOKEN is not set — export a User OAuth token (xoxp-…)")]
    TokenNotSet,

    #[error("Slack API error: {error}")]
    Api { error: String },

    #[error("Slack request failed: {0}")]
    Network(String),
}

// --- URL parsing ---

#[derive(Debug, Clone)]
pub struct SlackUrl {
    pub workspace: String,
    pub channel: String,
    pub ts: String,
    /// Present when the link points to a threaded reply.
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
    let ts_raw = segments[2].strip_prefix('p')?;
    if ts_raw.len() <= 6 {
        return None;
    }
    let (secs, micros) = ts_raw.split_at(ts_raw.len() - 6);
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

// --- API response types ---

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

// --- Client ---

#[derive(Clone)]
struct SlackToken(String);

impl std::fmt::Debug for SlackToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

pub struct SlackClient {
    http: Client,
    token: SlackToken,
}

impl SlackClient {
    pub fn from_env(http: Client) -> Result<Self, SlackError> {
        let raw = std::env::var("SLACK_TOKEN").map_err(|_| SlackError::TokenNotSet)?;
        if raw.trim().is_empty() {
            return Err(SlackError::TokenNotSet);
        }
        Ok(Self {
            http,
            token: SlackToken(raw),
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

        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token.0))
            .send()
            .await
            .map_err(|e| SlackError::Network(e.to_string()))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| SlackError::Network(e.to_string()))?;

        if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            return Err(SlackError::Api { error });
        }

        serde_json::from_value(body)
            .map_err(|e| SlackError::Network(format!("response parse error: {e}")))
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

    async fn resolve_user(&self, id: &str, cache: &mut HashMap<String, String>) -> String {
        if let Some(name) = cache.get(id) {
            return name.clone();
        }
        let name = match self
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
        };
        cache.insert(id.to_string(), name.clone());
        name
    }

    /// Replace `<@UXXXX>` and `<@UXXXX|name>` with `@display_name`.
    async fn resolve_mentions(&self, text: &str, cache: &mut HashMap<String, String>) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("<@") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find('>') else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let inner = &after[..end];
            let user_id = inner.split('|').next().unwrap_or(inner);
            let name = self.resolve_user(user_id, cache).await;
            out.push('@');
            out.push_str(&name);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    pub async fn fetch_message(&self, slack_url: &SlackUrl) -> Result<String, SlackError> {
        let ch = &slack_url.channel;
        let mut users = HashMap::new();

        // Fetch message(s): reply link → whole thread, otherwise history then check for thread
        let (messages, is_thread) = if let Some(ref thread_ts) = slack_url.thread_ts {
            let body: MessagesBody = self
                .api_get(
                    "conversations.replies",
                    &[("channel", ch), ("ts", thread_ts)],
                )
                .await?;
            (body.messages, true)
        } else {
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
                (thread.messages, true)
            } else {
                (body.messages, false)
            }
        };

        if messages.is_empty() {
            return Err(SlackError::Api {
                error: "message not found".into(),
            });
        }

        let channel_name = self.resolve_channel(ch).await;
        let author = match &messages[0].user {
            Some(uid) => self.resolve_user(uid, &mut users).await,
            None => "unknown".into(),
        };

        let reply_count = if is_thread && messages.len() > 1 {
            messages.len() - 1
        } else {
            0
        };

        // Frontmatter
        let escape = crate::fetch::converter::escape_yaml;
        let mut out = String::from("---\n");
        out.push_str(&format!("workspace: \"{}\"\n", escape(&slack_url.workspace)));
        out.push_str(&format!("channel: \"{}\"\n", escape(&channel_name)));
        out.push_str(&format!("author: \"{}\"\n", escape(&author)));
        out.push_str(&format!("ts: \"{}\"\n", slack_url.ts));
        if reply_count > 0 {
            out.push_str(&format!("replies: {reply_count}\n"));
        }
        out.push_str(&format!("url: {}\n", slack_url.raw_url));
        out.push_str("---\n\n");

        // Main message
        let text = self.resolve_mentions(&messages[0].text, &mut users).await;
        out.push_str(&text);

        // Thread replies (skip parent at index 0)
        if is_thread && messages.len() > 1 {
            for msg in &messages[1..] {
                let name = match &msg.user {
                    Some(uid) => self.resolve_user(uid, &mut users).await,
                    None => "unknown".into(),
                };
                let ts = msg.ts.as_deref().unwrap_or("");
                let reply_text = self.resolve_mentions(&msg.text, &mut users).await;
                out.push_str(&format!("\n\n---\n\n{name} ({ts}):\n{reply_text}"));
            }
        }

        if !out.ends_with('\n') {
            out.push('\n');
        }

        info!(
            workspace = %slack_url.workspace,
            channel = %channel_name,
            replies = reply_count,
            "slack fetch complete"
        );
        Ok(out)
    }
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

    #[test]
    fn parse_positive() {
        assert!(parse_slack_url("https://foo.slack.com/archives/C123/p1234567890123456").is_some());
    }

    #[test]
    fn parse_negative() {
        assert!(parse_slack_url("https://example.com").is_none());
        assert!(parse_slack_url("not-a-url").is_none());
    }
}
