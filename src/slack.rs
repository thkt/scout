use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{debug, info, warn};

use crate::clock::{Clock, SystemClock};
use crate::envelope::ErrorCode;
use crate::fetch::converter::escape_yaml;
use crate::redacted::{Redacted, validate_https};
#[cfg(test)]
use crate::retry::DEFAULT_MAX_RETRIES;
use crate::retry::{
    MAX_API_RESPONSE_BYTES, parse_retry_after, read_body_capped, retry_after_within_cap,
    retry_with_rate_limit,
};
use crate::rng::{FastrandRng, Rng};
use crate::tools::Classification;

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

struct FetchedThread {
    messages: Vec<Message>,
    is_thread: bool,
}

pub(crate) struct SlackClient {
    http: Client,
    token: Redacted,
    base_url: String,
    max_retries: u32,
    /// Wall-clock source for `parse_retry_after`. Set at construction and
    /// read on every Slack 429; defaults to `SystemClock`. Mirrors
    /// `GitHubClient`'s injection seam.
    clock: Arc<dyn Clock>,
    /// Backoff jitter source handed to `retry_with_rate_limit` per attempt.
    /// Set at construction; defaults to `FastrandRng`.
    rng: Arc<dyn Rng>,
    /// Test-only escape hatch for wiremock servers on `http://127.0.0.1`.
    /// Production constructors leave this `false` so `api_get_once` always
    /// runs `validate_https`; only `with_base_url` opts in.
    #[cfg(test)]
    skip_https_check: bool,
}

const API_BASE: &str = "https://slack.com/api";

/// Cap for `conversations.replies` page size. Slack's default is undocumented
/// and threads can grow into the thousands on incident channels; making the
/// limit explicit bounds the JSON payload that `api_get_once` buffers in
/// memory (issue #155 / CHX-005).
const SLACK_REPLIES_LIMIT: &str = "200";

/// Concurrent in-flight `users.info` requests during `prefetch_users`.
/// Slack Tier-4 allows ~50 req/min; capping at 5 keeps the burst well below
/// that even for threads with hundreds of unique participants, instead of
/// firing every request simultaneously and tripping the per-minute cap
/// (issue #155 / OPS-009 / CHX-001).
const SLACK_USERS_CONCURRENCY: usize = 5;

impl SlackClient {
    pub fn new(http: Client, token: Redacted, max_retries: u32) -> Self {
        Self {
            http,
            token,
            base_url: API_BASE.to_owned(),
            max_retries,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            #[cfg(test)]
            skip_https_check: false,
        }
    }

    pub fn from_env(http: Client, max_retries: u32) -> Result<Self, SlackError> {
        let raw = env::var("SLACK_TOKEN").map_err(|_| SlackError::TokenNotSet)?;
        let token = Redacted::new(&raw).ok_or(SlackError::TokenNotSet)?;
        Ok(Self::new(http, token, max_retries))
    }

    #[cfg(test)]
    fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            token: Redacted::new("xoxp-test").expect("static literal is non-empty"),
            base_url: base_url.to_owned(),
            max_retries: DEFAULT_MAX_RETRIES,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            skip_https_check: true,
        }
    }

    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub(crate) fn with_rng(mut self, rng: Arc<dyn Rng>) -> Self {
        self.rng = rng;
        self
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
            self.max_retries,
            is_retriable,
            |e| match e {
                SlackError::RateLimited { retry_after } => *retry_after,
                _ => None,
            },
            || SlackError::RateLimited { retry_after: None },
            self.rng.as_ref(),
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

        let retry_after = parse_retry_after(resp.headers(), self.clock.as_ref());

        if resp.status() == 429 {
            warn!(retry_after_secs = retry_after, "Slack API rate limited");
            return Err(SlackError::RateLimited { retry_after });
        }

        let bytes = read_body_capped(
            resp,
            || {
                SlackError::Decode(format!(
                    "response too large (>{MAX_API_RESPONSE_BYTES} bytes)"
                ))
            },
            |e| SlackError::Network(e.to_string()),
        )
        .await?;
        // Schema fail → Decode (terminal); transport drop already mapped to
        // Network by the closure above (issue #113).
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| SlackError::Decode(e.to_string()))?;

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

    /// Slack `users.info` per-ID fetch capped at `SLACK_USERS_CONCURRENCY`
    /// concurrent requests via `buffer_unordered`. The cap bounds the burst
    /// rate so a thread with hundreds of participants cannot fire that many
    /// simultaneous requests and trip Slack's per-minute rate limit. Matches
    /// the same idiom used in `search/engine.rs::fetch_sources`.
    async fn prefetch_users(&self, ids: &HashSet<String>) -> HashMap<String, String> {
        let id_list: Vec<String> = ids.iter().cloned().collect();
        stream::iter(id_list)
            .map(|id| async move {
                let name = self.fetch_user_name(&id).await;
                (id, name)
            })
            .buffer_unordered(SLACK_USERS_CONCURRENCY)
            .collect()
            .await
    }

    async fn fetch_thread(&self, slack_url: &SlackUrl) -> Result<FetchedThread, SlackError> {
        let ch = &slack_url.channel;
        if let Some(ref thread_ts) = slack_url.thread_ts {
            let body: MessagesBody = self
                .api_get(
                    "conversations.replies",
                    &[
                        ("channel", ch),
                        ("ts", thread_ts),
                        ("limit", SLACK_REPLIES_LIMIT),
                    ],
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
                    &[
                        ("channel", ch),
                        ("ts", &slack_url.ts),
                        ("limit", SLACK_REPLIES_LIMIT),
                    ],
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
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct DummyBody {
    ok: bool,
}

#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod constructor_tests;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod mention_tests;
#[cfg(test)]
mod resolve_messages_tests;
#[cfg(test)]
mod url_tests;
