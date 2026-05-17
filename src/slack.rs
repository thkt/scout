use std::collections::{HashMap, HashSet};
use std::env;

use futures::future::join_all;
use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{debug, info, warn};

use crate::fetch::converter::escape_yaml;
use crate::redacted::{Redacted, validate_https};
use crate::retry::{
    is_schema_decode_fail, parse_retry_after, retry_after_within_cap, retry_with_rate_limit,
};

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

#[derive(Debug, Clone)]
pub(crate) struct SlackUrl {
    pub workspace: String,
    pub channel: String,
    pub ts: String,
    pub thread_ts: Option<String>,
    pub raw_url: String,
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

struct FetchedThread {
    messages: Vec<Message>,
    is_thread: bool,
}

pub(crate) struct SlackClient {
    http: Client,
    token: Redacted,
    base_url: String,
    /// Test-only escape hatch for wiremock servers on `http://127.0.0.1`.
    /// Production constructors leave this `false` so `api_get_once` always
    /// runs `validate_https`; only `with_base_url` opts in.
    #[cfg(test)]
    skip_https_check: bool,
}

const API_BASE: &str = "https://slack.com/api";

impl SlackClient {
    pub fn new(http: Client, token: Redacted) -> Self {
        Self {
            http,
            token,
            base_url: API_BASE.to_owned(),
            #[cfg(test)]
            skip_https_check: false,
        }
    }

    pub fn from_env(http: Client) -> Result<Self, SlackError> {
        let raw = env::var("SLACK_TOKEN").map_err(|_| SlackError::TokenNotSet)?;
        if raw.trim().is_empty() {
            return Err(SlackError::TokenNotSet);
        }
        Ok(Self::new(http, Redacted::new(&raw)))
    }

    #[cfg(test)]
    fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            token: Redacted::new("xoxp-test"),
            base_url: base_url.to_owned(),
            skip_https_check: true,
        }
    }

    /// Test-only override of the production HTTPS gate. See [`validate_https`].
    fn should_check_https(&self) -> bool {
        #[cfg(test)]
        {
            !self.skip_https_check
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    async fn api_get<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, SlackError> {
        retry_with_rate_limit(
            || self.api_get_once(method, params),
            is_retriable,
            |e| match e {
                SlackError::RateLimited { retry_after } => *retry_after,
                _ => None,
            },
            || SlackError::RateLimited { retry_after: None },
        )
        .await
    }

    async fn api_get_once<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, SlackError> {
        let mut url = url::Url::parse(&format!("{}/{method}", self.base_url))
            .map_err(|e| SlackError::Network(e.to_string()))?;
        for (k, v) in params {
            url.query_pairs_mut().append_pair(k, v);
        }

        if self.should_check_https() {
            validate_https(url.as_str(), || SlackError::InsecureUrl)?;
        }

        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token.expose()))
            .send()
            .await
            .map_err(|e| SlackError::Network(e.to_string()))?;

        let retry_after = parse_retry_after(resp.headers());

        if resp.status() == 429 {
            warn!(retry_after_secs = retry_after, "Slack API rate limited");
            return Err(SlackError::RateLimited { retry_after });
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            // Schema fail → Decode (terminal). Transport drop → Network →
            // retry loop. See `is_schema_decode_fail` (issue #113).
            if is_schema_decode_fail(&e) {
                SlackError::Decode(e.to_string())
            } else {
                SlackError::Network(e.to_string())
            }
        })?;

        if body.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            // ok:false with a missing `error` field is a Slack API contract
            // violation, not a user-fixable failure — route through Decode so
            // it classifies as Internal(70) rather than UsageError.
            let Some(error) = body.get("error").and_then(|v| v.as_str()) else {
                return Err(SlackError::Decode(
                    "Slack response had `ok: false` without an `error` field".into(),
                ));
            };
            if error == "ratelimited" {
                warn!(retry_after_secs = retry_after, "Slack API rate limited");
                return Err(SlackError::RateLimited { retry_after });
            }
            return Err(SlackError::Api {
                error: error.to_owned(),
            });
        }

        serde_json::from_value(body).map_err(|e| SlackError::Decode(e.to_string()))
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
                .unwrap_or_else(|| id.to_owned()),
            Err(e) => {
                warn!(channel_id = %id, error = %e, "channel resolution failed, using raw ID");
                id.to_owned()
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
                .unwrap_or_else(|| id.to_owned()),
            Err(e) => {
                warn!(user_id = %id, error = %e, "user resolution failed, using raw ID");
                id.to_owned()
            }
        }
    }

    /// Slack `users.info` per-ID fetch via `join_all`.
    ///
    /// No concurrency cap because Slack Tier-4 allows 50+ req/min and a single
    /// thread rarely has more than ~20 unique users. Sustained violation would
    /// require a thread with hundreds of participants. If that becomes a real
    /// case, switch to `buffer_unordered` with an explicit cap.
    async fn prefetch_users(&self, ids: &HashSet<String>) -> HashMap<String, String> {
        let ids: Vec<String> = ids.iter().cloned().collect();
        let futs = ids.iter().map(|id| self.fetch_user_name(id));
        let results = join_all(futs).await;
        ids.into_iter().zip(results).collect()
    }

    async fn fetch_thread(&self, slack_url: &SlackUrl) -> Result<FetchedThread, SlackError> {
        let ch = &slack_url.channel;
        if let Some(ref thread_ts) = slack_url.thread_ts {
            let body: MessagesBody = self
                .api_get(
                    "conversations.replies",
                    &[("channel", ch), ("ts", thread_ts)],
                )
                .await?;
            return Ok(FetchedThread {
                messages: body.messages,
                is_thread: true,
            });
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
            Ok(FetchedThread {
                messages: thread.messages,
                is_thread: true,
            })
        } else {
            Ok(FetchedThread {
                messages: body.messages,
                is_thread: false,
            })
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

        let resolved = resolve_messages(&fetched.messages, &users);

        let (first, resolved) = extract_target(resolved, &slack_url.ts, fetched.is_thread)
            .ok_or_else(|| SlackError::Api {
                error: format!("message {} not found in thread", slack_url.ts),
            })?;
        let replies: &[ResolvedMessage] = if fetched.is_thread { &resolved } else { &[] };
        let output = format_slack_output(slack_url, &channel_name, &first, replies);
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

    out.push_str(&first.text);

    for msg in replies {
        let ts_suffix = if msg.ts.is_empty() {
            String::new()
        } else {
            format!(" ({})", msg.ts)
        };
        out.push_str(&format!(
            "\n\n---\n\n{}{}:\n{}",
            msg.author, ts_suffix, msg.text
        ));
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

fn is_retriable(e: &SlackError) -> bool {
    match e {
        SlackError::RateLimited { retry_after } => retry_after_within_cap(*retry_after),
        SlackError::Network(_) | SlackError::Timeout(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    #[derive(Debug, serde::Deserialize)]
    struct DummyBody {
        ok: bool,
    }

    mod http_tests {
        use super::*;
        use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};
        use reqwest::Client;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};

        /// [T-SK001] HTTP 429 response maps to SlackError::RateLimited
        #[tokio::test]
        async fn api_get_once_429_returns_rate_limited() {
            let Some(server) = try_spawn_mock_server("slack::http").await else {
                return;
            };
            Mock::given(method("GET"))
                .and(path("/test.method"))
                .respond_with(ResponseTemplate::new(429))
                .mount(&server)
                .await;

            let client = SlackClient::with_base_url(Client::new(), &server.uri());
            let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
            assert!(matches!(
                result,
                Err(SlackError::RateLimited { retry_after: None })
            ));
        }

        /// [T-SK002] HTTP 429 with Retry-After header preserves header value
        #[tokio::test]
        async fn api_get_once_429_with_retry_after_header() {
            let Some(server) = try_spawn_mock_server("slack::http").await else {
                return;
            };
            Mock::given(method("GET"))
                .and(path("/test.method"))
                .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "30"))
                .mount(&server)
                .await;

            let client = SlackClient::with_base_url(Client::new(), &server.uri());
            let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
            assert!(matches!(
                result,
                Err(SlackError::RateLimited {
                    retry_after: Some(30)
                })
            ));
        }

        /// [T-SK003] Body-level ratelimited error maps to SlackError::RateLimited
        #[tokio::test]
        async fn api_get_once_body_ratelimited_returns_rate_limited() {
            let Some(server) = try_spawn_mock_server("slack::http").await else {
                return;
            };
            Mock::given(method("GET"))
                .and(path("/test.method"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"ok": false, "error": "ratelimited"})),
                )
                .mount(&server)
                .await;

            let client = SlackClient::with_base_url(Client::new(), &server.uri());
            let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
            assert!(matches!(result, Err(SlackError::RateLimited { .. })));
        }

        /// [T-SK004] Non-ratelimited Slack API error maps to SlackError::Api
        #[tokio::test]
        async fn api_get_once_api_error_returns_api_variant() {
            let Some(server) = try_spawn_mock_server("slack::http").await else {
                return;
            };
            Mock::given(method("GET"))
                .and(path("/test.method"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(
                        serde_json::json!({"ok": false, "error": "channel_not_found"}),
                    ),
                )
                .mount(&server)
                .await;

            let client = SlackClient::with_base_url(Client::new(), &server.uri());
            let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
            assert!(matches!(
                result,
                Err(SlackError::Api { error }) if error == "channel_not_found"
            ));
        }

        /// [T-SK031] ok:false without an `error` field surfaces as SlackError::Decode
        /// (issue #114 condition 5). The previous code substituted the literal
        /// "unknown" string and mapped to UsageError; a missing `error` is a
        /// Slack API contract violation, not a user-fixable failure.
        #[tokio::test]
        async fn api_get_once_ok_false_without_error_field_returns_decode() {
            let Some(server) = try_spawn_mock_server("slack::http").await else {
                return;
            };
            Mock::given(method("GET"))
                .and(path("/test.method"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": false})),
                )
                .mount(&server)
                .await;

            let client = SlackClient::with_base_url(Client::new(), &server.uri());
            let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
            assert!(
                matches!(result, Err(SlackError::Decode(_))),
                "expected SlackError::Decode for ok:false without error field, got: {result:?}"
            );
        }

        /// [T-SK030] Mid-stream body drop on 2xx routes through SlackError::Network
        /// (transient, retry path) rather than SlackError::Decode (terminal). reqwest
        /// 0.13 reports the drop as `is_decode() == true`; `is_transient_decode`
        /// distinguishes it from a schema fail via the io::Error source chain (issue #113).
        #[tokio::test]
        async fn api_get_once_2xx_mid_stream_drop_returns_network() {
            let Some((url, _counter, handle)) = spawn_mid_stream_drop_server(1) else {
                return;
            };
            let client = SlackClient::with_base_url(Client::new(), &url);
            let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
            assert!(
                matches!(result, Err(SlackError::Network(_))),
                "expected SlackError::Network for mid-stream drop, got: {result:?}"
            );
            let _ = handle.join();
        }
    }

    /// [T-SK005] parse_slack_url accepts a standard archive permalink
    #[test]
    fn parse_standard_url() {
        let url = "https://myteam.slack.com/archives/C0656BJSFL7/p1773819598273499";
        let parsed = parse_slack_url(url).unwrap();
        assert_eq!(parsed.workspace, "myteam");
        assert_eq!(parsed.channel, "C0656BJSFL7");
        assert_eq!(parsed.ts, "1773819598.273499");
        assert!(parsed.thread_ts.is_none());
    }

    /// [T-SK006] parse_slack_url extracts thread_ts from parent permalink
    #[test]
    fn parse_parent_permalink_with_thread_ts() {
        let url = "https://team.slack.com/archives/C123/p1234567890123456?thread_ts=1234567890.123456&cid=C123";
        let parsed = parse_slack_url(url).unwrap();
        assert_eq!(parsed.channel, "C123");
        assert_eq!(parsed.ts, "1234567890.123456");
        assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
    }

    /// [T-SK007] Reply permalink carries distinct ts and thread_ts
    #[test]
    fn parse_reply_permalink_has_different_ts_and_thread_ts() {
        let url = "https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456&cid=C123";
        let parsed = parse_slack_url(url).unwrap();
        assert_eq!(parsed.ts, "1111111111.222222");
        assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
    }

    /// [T-SK008] format_slack_output uses targeted reply as primary message
    #[test]
    fn format_output_uses_reply_as_primary_when_targeted() {
        let slack_url = SlackUrl {
            workspace: "team".into(),
            channel: "C123".into(),
            ts: "1111111111.222222".into(),
            thread_ts: Some("1234567890.123456".into()),
            raw_url:
                "https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456"
                    .into(),
        };
        let reply = ResolvedMessage {
            author: "reply-author".into(),
            text: "reply body".into(),
            ts: "1111111111.222222".into(),
        };
        let parent = ResolvedMessage {
            author: "parent-author".into(),
            text: "parent body".into(),
            ts: "1234567890.123456".into(),
        };
        let output = format_slack_output(&slack_url, "#general", &reply, &[parent]);
        let expected = "\
---
workspace: \"team\"
channel: \"#general\"
author: \"reply-author\"
ts: \"1111111111.222222\"
context_messages: 1
url: \"https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456\"
---

reply body

---

parent-author (1234567890.123456):
parent body
";
        assert_eq!(output, expected);
    }

    fn msg(ts: &str, author: &str) -> ResolvedMessage {
        ResolvedMessage {
            author: author.into(),
            text: format!("text by {author}"),
            ts: ts.into(),
        }
    }

    /// [T-SK009] extract_target picks the reply matching target ts from a thread
    #[test]
    fn extract_target_picks_reply_from_thread() {
        let messages = vec![
            msg("1000.000000", "parent"),
            msg("1001.000000", "reply-1"),
            msg("1002.000000", "reply-2"),
        ];
        let (first, rest) = extract_target(messages, "1001.000000", true).unwrap();
        assert_eq!(first.ts, "1001.000000");
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].ts, "1000.000000");
        assert_eq!(rest[1].ts, "1002.000000");
    }

    /// [T-SK010] extract_target returns None when target ts not present
    #[test]
    fn extract_target_returns_none_when_ts_missing() {
        let messages = vec![msg("1000.000000", "parent"), msg("1001.000000", "reply-1")];
        assert!(extract_target(messages, "9999.999999", true).is_none());
    }

    /// [T-SK011] extract_target ignores ts for non-thread messages and picks first
    #[test]
    fn extract_target_ignores_ts_for_non_thread() {
        let messages = vec![msg("1000.000000", "author")];
        let (first, rest) = extract_target(messages, "9999.999999", false).unwrap();
        assert_eq!(first.ts, "1000.000000");
        assert!(rest.is_empty());
    }

    /// [T-SK012] parse_slack_url rejects URLs outside *.slack.com
    #[test]
    fn parse_rejects_non_slack_url() {
        assert!(parse_slack_url("https://example.com/page").is_none());
    }

    /// [T-SK013] parse_slack_url rejects paths outside /archives
    #[test]
    fn parse_rejects_non_archives_path() {
        assert!(parse_slack_url("https://team.slack.com/messages/C123/p111111222222333").is_none());
    }

    /// [T-SK014] parse_slack_url rejects timestamps shorter than micro-precision
    #[test]
    fn parse_rejects_short_timestamp() {
        assert!(parse_slack_url("https://team.slack.com/archives/C123/p12345").is_none());
    }

    mod mention_tests {
        use super::*;

        /// [T-SK015] parse_mentions on plain text returns no spans
        #[test]
        fn t001_no_mentions_returns_empty() {
            let spans = parse_mentions("hello world");
            assert!(spans.is_empty());
        }

        /// [T-SK016] parse_mentions captures one span with byte offsets for a single mention
        #[test]
        fn t002_single_mention_returns_one_span() {
            let text = "hi <@U123> bye";
            let spans = parse_mentions(text);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].user_id, "U123");
            assert_eq!(spans[0].start, 3);
            assert_eq!(spans[0].end, 10);
            assert_eq!(&text[spans[0].start..spans[0].end], "<@U123>");
        }

        /// [T-SK017] parse_mentions extracts user id only from pipe-labeled mention
        #[test]
        fn t003_pipe_label_extracts_user_id_only() {
            let text = "cc <@U123|alice>";
            let spans = parse_mentions(text);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].user_id, "U123");
        }

        /// [T-SK018] parse_mentions captures consecutive adjacent mentions
        #[test]
        fn t004_multiple_adjacent_mentions() {
            let text = "<@U001><@U002><@U003>";
            let spans = parse_mentions(text);
            assert_eq!(spans.len(), 3);
            assert_eq!(spans[0].user_id, "U001");
            assert_eq!(spans[1].user_id, "U002");
            assert_eq!(spans[2].user_id, "U003");
            assert_eq!(spans[0].end, spans[1].start);
            assert_eq!(spans[1].end, spans[2].start);
        }

        /// [T-SK019] parse_mentions stops at unclosed mention token
        #[test]
        fn t005_unclosed_mention_breaks_early() {
            let text = "<@U001> then <@U002";
            let spans = parse_mentions(text);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].user_id, "U001");
        }

        /// [T-SK020] parse_mentions yields correct byte offsets across multibyte characters
        #[test]
        fn t006_multibyte_characters_correct_offsets() {
            // CJK characters are 3 bytes each in UTF-8
            let text = "\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}<@UCJK>end";
            let spans = parse_mentions(text);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].user_id, "UCJK");
            // 5 CJK chars x 3 bytes = 15, so <@UCJK> starts at byte 15
            assert_eq!(spans[0].start, 15);
            assert_eq!(&text[spans[0].start..spans[0].end], "<@UCJK>");

            // Emoji (4-byte) surrounding a mention
            let emoji_text = "\u{1F600}<@UEMJ>\u{1F600}";
            let spans2 = parse_mentions(emoji_text);
            assert_eq!(spans2.len(), 1);
            assert_eq!(spans2[0].user_id, "UEMJ");
            assert_eq!(spans2[0].start, 4);
            assert_eq!(&emoji_text[spans2[0].start..spans2[0].end], "<@UEMJ>");
        }

        /// [T-SK021] substitute_mentions replaces known user id with display name
        #[test]
        fn t007_known_user_replaced_with_display_name() {
            let cache: HashMap<String, String> =
                [("U100".into(), "Alice".into())].into_iter().collect();
            let result = substitute_mentions("hello <@U100> world", &cache);
            assert_eq!(result, "hello @Alice world");
        }

        /// [T-SK022] substitute_mentions falls back to @UID when user unknown
        #[test]
        fn t008_unknown_user_kept_as_at_uid() {
            let cache: HashMap<String, String> = HashMap::new();
            let result = substitute_mentions("hello <@UXXX> world", &cache);
            assert_eq!(result, "hello @UXXX world");
        }

        /// [T-SK023] substitute_mentions returns text unchanged when no mentions present
        #[test]
        fn t009_no_mentions_returns_text_unchanged() {
            let cache: HashMap<String, String> =
                [("U100".into(), "Alice".into())].into_iter().collect();
            let text = "no mentions here";
            let result = substitute_mentions(text, &cache);
            assert_eq!(result, text);
        }

        /// [T-SK024] substitute_mentions replaces pipe-labeled mention with display name
        #[test]
        fn t009b_pipe_label_substituted_with_display_name() {
            let cache: HashMap<String, String> =
                [("U123".into(), "Alice".into())].into_iter().collect();
            let result = substitute_mentions("cc <@U123|alice_handle>", &cache);
            assert_eq!(result, "cc @Alice");
        }
    }

    mod constructor_tests {
        use super::*;
        use crate::test_support::try_spawn_mock_server;
        use reqwest::Client;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};

        /// [T-SK025] SlackClient::with_base_url constructs a client that reaches a wiremock server
        #[tokio::test]
        async fn t010_with_base_url_constructs_usable_client() {
            let Some(server) = try_spawn_mock_server("slack::http").await else {
                return;
            };
            Mock::given(method("GET"))
                .and(path("/auth.test"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
                )
                .mount(&server)
                .await;

            let client = SlackClient::with_base_url(Client::new(), &server.uri());

            let result: Result<DummyBody, _> = client.api_get_once("auth.test", &[]).await;
            assert!(result.is_ok());
        }
    }

    mod resolve_messages_tests {
        use super::*;
        use tracing_test::traced_test;

        fn make_msg(user: Option<&str>, text: &str, ts: Option<&str>) -> Message {
            Message {
                user: user.map(String::from),
                text: text.into(),
                ts: ts.map(String::from),
                reply_count: None,
            }
        }

        #[traced_test]
        /// [T-SK026] resolve_messages logs debug and falls back to "(no author)" when user is None
        #[test]
        fn t012_user_none_emits_debug_and_falls_back_to_no_author() {
            let messages = vec![make_msg(None, "hello", Some("1000.000"))];
            let users = HashMap::new();

            let resolved = resolve_messages(&messages, &users);

            assert_eq!(resolved[0].author, "(no author)");
            assert!(logs_contain("msg.user is None"));
            assert!(logs_contain("DEBUG"));
        }

        #[traced_test]
        /// [T-SK027] resolve_messages logs warn and falls back to empty ts when missing
        #[test]
        fn t013_ts_none_emits_warn_and_falls_back_to_empty() {
            let messages = vec![make_msg(Some("U1"), "hi", None)];
            let users = HashMap::from([("U1".into(), "Alice".into())]);

            let resolved = resolve_messages(&messages, &users);

            assert_eq!(resolved[0].ts, "");
            assert!(logs_contain("msg.ts is None"));
            assert!(logs_contain("WARN"));
        }

        #[traced_test]
        /// [T-SK028] resolve_messages resolves both author and mention via user map
        #[test]
        fn t014_mention_resolved_and_user_mapped() {
            let messages = vec![make_msg(Some("U1"), "cc <@U2>", Some("1000.000"))];
            let users = HashMap::from([("U1".into(), "Alice".into()), ("U2".into(), "Bob".into())]);

            let resolved = resolve_messages(&messages, &users);

            assert_eq!(resolved.len(), 1);
            assert_eq!(resolved[0].author, "Alice");
            assert_eq!(resolved[0].text, "cc @Bob");
            assert_eq!(resolved[0].ts, "1000.000");
        }

        /// [T-SK029] resolve_messages keeps unknown user id as the author label
        #[test]
        fn t015_unknown_user_id_kept_as_author() {
            let messages = vec![make_msg(Some("UXXX"), "text", Some("1000.000"))];
            let users = HashMap::new();

            let resolved = resolve_messages(&messages, &users);

            assert_eq!(resolved[0].author, "UXXX");
        }
    }
}
