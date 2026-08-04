//! Slack Web API client: token-bearing HTTP, retry/rate-limit handling, and
//! thread fetch + message resolution orchestration. Wire-format response
//! structs are defined directly above the method that deserializes into them.

use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::body_limit::{MAX_API_RESPONSE_BYTES, read_body_capped};
use crate::clock::{Clock, SystemClock};
use crate::envelope::ErrorCode;
use crate::redacted::{Redacted, validate_https};
#[cfg(test)]
use crate::retry::DEFAULT_MAX_RETRIES;
use crate::retry::{parse_retry_after, retry_after_within_cap, retry_with_rate_limit};
use crate::rng::{FastrandRng, Rng};

use super::{
    Message, ResolvedMessage, SlackError, SlackUrl, collect_mention_ids_ordered, extract_target,
    format_slack_output, resolve_messages,
};

struct FetchedThread {
    messages: Vec<Message>,
    is_thread: bool,
    /// True when `fetch_replies` stopped at `SLACK_MAX_REPLY_PAGES` with more
    /// pages still available, so the thread is missing replies past the cap.
    truncated: bool,
}

/// Result of resolving a Slack permalink into rendered Markdown, carrying the
/// cap-hit signals `fetch_slack` needs to wire into the ADR-0003 degradation
/// channel. A bare `String` return hid these, so caps were invisible to callers
/// (issue #222).
pub(crate) struct SlackFetchOutcome {
    pub markdown: String,
    /// `conversations.replies` hit the page cap; replies past it are omitted.
    pub thread_truncated: bool,
    /// Distinct user IDs exceeded `SLACK_MAX_USER_LOOKUPS`; the excess render
    /// as raw `<@UID>` instead of resolved names.
    pub users_capped: bool,
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

/// Slack User OAuth token strings begin with `xoxp-`; bot (`xoxb-`), app-level
/// (`xapp-`), workflow (`xwfp-`), and config tokens do not. scout requires a
/// user token so the channels, threads, and users it resolves match the human's
/// own workspace visibility (a bot token only sees channels its app was added
/// to). `from_env_with` rejects any other prefix up front instead of letting it
/// fail later with an opaque API error (issue #261). Verified against
/// <https://api.slack.com/concepts/token-types#user> (2026-06).
const USER_TOKEN_PREFIX: &str = "xoxp-";

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

/// Upper bound on `conversations.replies` pages fetched per thread. At
/// `SLACK_REPLIES_LIMIT` (200) messages per page this covers threads up to
/// ~10k replies; the cap stops an unbounded paging loop from re-introducing
/// the rate-limit exhaustion that claim 3 bounds (issue #188 claim 2).
const SLACK_MAX_REPLY_PAGES: usize = 50;

/// Upper bound on distinct user IDs resolved via `users.info` per message.
/// A single message can mention an unbounded number of users; without a cap a
/// mass-mention burst can exhaust Slack's Tier-4 per-minute budget. IDs beyond
/// the cap are not looked up and degrade to their raw `<@UID>` form (issue
/// #188 claim 3).
const SLACK_MAX_USER_LOOKUPS: usize = 50;

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

    pub(crate) fn from_env(http: Client, max_retries: u32) -> Result<Self, SlackError> {
        Self::from_env_with(http, max_retries, |k| env::var(k))
    }

    /// Wraps [`Self::from_env`] with a caller-supplied env reader so unit
    /// tests can exercise the token-not-set / whitespace branches without
    /// `unsafe { std::env::set_var(...) }` (forbidden by `unsafe_code = "forbid"`).
    /// Mirrors [`crate::brave::client::BraveClient::from_env_with`] (ADR-0007).
    pub(crate) fn from_env_with<F>(
        http: Client,
        max_retries: u32,
        get_var: F,
    ) -> Result<Self, SlackError>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        let raw = get_var("SLACK_TOKEN").map_err(|_| SlackError::TokenNotSet)?;
        let token = Redacted::new(&raw).ok_or(SlackError::TokenNotSet)?;
        if !token.expose().starts_with(USER_TOKEN_PREFIX) {
            return Err(SlackError::TokenWrongType);
        }
        Ok(Self::new(http, token, max_retries))
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(http: Client, base_url: &str) -> Self {
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
            self.rng.as_ref(),
        )
        .await
    }

    async fn api_get_once<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, SlackError> {
        let mut url = url::Url::parse(&format!("{}/{method}", self.base_url))?;
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
            .await?;

        let retry_after = parse_retry_after(resp.headers(), self.clock.as_ref());

        if resp.status() == 429 {
            warn!(retry_after_secs = retry_after, "Slack API rate limited");
            return Err(SlackError::RateLimited { retry_after });
        }

        // Slack reports its own failures as `error` strings inside a 200 body, so
        // any other non-2xx came from something between scout and Slack. Its body
        // is not an API envelope, and letting it reach the JSON parse turned a
        // gateway hiccup into Decode -> Internal(70), which never retries.
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            warn!(status, "Slack API returned a non-success status");
            return Err(SlackError::Server(status));
        }

        let bytes = read_body_capped(
            resp,
            MAX_API_RESPONSE_BYTES,
            || {
                SlackError::Decode(format!(
                    "response too large (>{MAX_API_RESPONSE_BYTES} bytes)"
                ))
            },
            SlackError::from,
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
}

#[derive(Deserialize)]
struct ChannelBody {
    channel: Option<ChannelInfo>,
}

#[derive(Deserialize)]
struct ChannelInfo {
    name: Option<String>,
}

impl SlackClient {
    async fn resolve_channel(&self, id: &str) -> String {
        match self
            .api_get::<ChannelBody>("conversations.info", &[("channel", id)])
            .await
        {
            Ok(b) => b
                .channel
                .and_then(|c| c.name)
                .map(|n| format!("#{n}"))
                .unwrap_or_else(|| {
                    warn!(channel_id = %id, "channel name missing in response, using raw ID");
                    id.to_owned()
                }),
            Err(e) => {
                warn!(channel_id = %id, error = %e, "channel resolution failed, using raw ID");
                id.to_owned()
            }
        }
    }
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

impl SlackClient {
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
                .unwrap_or_else(|| {
                    warn!(user_id = %id, "user name missing in response, using raw ID");
                    id.to_owned()
                }),
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

impl SlackClient {
    /// Fetch every reply in a thread, following `response_metadata.next_cursor`
    /// across pages up to `SLACK_MAX_REPLY_PAGES`. Without this loop a target
    /// message past the first `SLACK_REPLIES_LIMIT` page is silently dropped and
    /// surfaces as "not found" (issue #188 claim 2).
    /// Returns the fetched replies paired with a `truncated` flag: `true` when
    /// the loop stopped at `SLACK_MAX_REPLY_PAGES` with another page still
    /// advertised, `false` when it ran out of pages naturally.
    async fn fetch_replies(
        &self,
        channel: &str,
        ts: &str,
    ) -> Result<(Vec<Message>, bool), SlackError> {
        let mut messages = Vec::new();
        // conversations.replies is observed to repeat the thread parent as
        // messages[0] on each page; the official reference is silent, so dedup
        // by ts defensively. Safe because ts is unique per message in a channel.
        let mut seen: HashSet<String> = HashSet::new();
        let mut cursor: Option<String> = None;
        for _ in 0..SLACK_MAX_REPLY_PAGES {
            // Scope `params` so its borrow of `cursor` ends before the
            // reassignment below.
            let body: MessagesBody = {
                let mut params = vec![
                    ("channel", channel),
                    ("ts", ts),
                    ("limit", SLACK_REPLIES_LIMIT),
                ];
                if let Some(c) = cursor.as_deref() {
                    params.push(("cursor", c));
                }
                self.api_get("conversations.replies", &params).await?
            };
            let next = body.next_cursor().map(str::to_owned);
            for msg in body.messages {
                // A message with no ts cannot be deduped; keep it as-is.
                match &msg.ts {
                    Some(t) if !seen.insert(t.clone()) => continue,
                    _ => messages.push(msg),
                }
            }
            match next {
                Some(c) => cursor = Some(c),
                None => return Ok((messages, false)),
            }
        }
        warn!(
            channel = %channel,
            ts = %ts,
            max_pages = SLACK_MAX_REPLY_PAGES,
            "conversations.replies hit the page cap, thread truncated"
        );
        Ok((messages, true))
    }

    async fn fetch_thread(&self, slack_url: &SlackUrl) -> Result<FetchedThread, SlackError> {
        let ch = &slack_url.channel;
        if let Some(ref thread_ts) = slack_url.thread_ts {
            let (messages, truncated) = self.fetch_replies(ch, thread_ts).await?;
            return Ok(FetchedThread {
                messages,
                is_thread: true,
                truncated,
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
            let (messages, truncated) = self.fetch_replies(ch, &slack_url.ts).await?;
            Ok(FetchedThread {
                messages,
                is_thread: true,
                truncated,
            })
        } else {
            Ok(FetchedThread {
                messages: body.messages,
                is_thread: false,
                truncated: false,
            })
        }
    }

    pub async fn fetch_message(
        &self,
        slack_url: &SlackUrl,
    ) -> Result<SlackFetchOutcome, SlackError> {
        let fetched = self.fetch_thread(slack_url).await?;
        if fetched.messages.is_empty() {
            return Err(SlackError::Api {
                error: "message not found".into(),
            });
        }

        // Authors render on every message, so when distinct IDs exceed the
        // lookup cap they take priority over mentions: an unresolved author
        // degrades visible output more than an unresolved mention. The keep set
        // is fixed to first-occurrence (thread chronological) order so which IDs
        // resolve is reproducible across runs (issue #221). Two passes over the
        // messages — all authors first, then mentions — share one `seen` set, so
        // a dual-role ID is kept as an author and consumes one slot, not two.
        let mut authors: Vec<String> = Vec::new();
        let mut mentions: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for msg in &fetched.messages {
            if let Some(uid) = &msg.user
                && seen.insert(uid.clone())
            {
                authors.push(uid.clone());
            }
        }
        for msg in &fetched.messages {
            collect_mention_ids_ordered(&msg.text, &mut seen, &mut mentions);
        }

        let distinct_total = authors.len() + mentions.len();
        let users_capped = distinct_total > SLACK_MAX_USER_LOOKUPS;
        if users_capped {
            warn!(
                distinct_users = distinct_total,
                cap = SLACK_MAX_USER_LOOKUPS,
                "too many distinct user IDs; capping users.info lookups, excess IDs render as raw IDs (authors kept first)"
            );
        }
        // `take` is a no-op under the cap, so the capped and uncapped cases
        // collapse to one chained collect: authors first, then mention top-up.
        let user_ids: HashSet<String> = authors
            .into_iter()
            .chain(mentions)
            .take(SLACK_MAX_USER_LOOKUPS)
            .collect();

        let (channel_name, users) = tokio::join!(
            self.resolve_channel(&slack_url.channel),
            self.prefetch_users(&user_ids),
        );

        let resolved = resolve_messages(&fetched.messages, &users);

        let (first, resolved) =
            extract_target(resolved, &slack_url.ts).ok_or_else(|| SlackError::Api {
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
        Ok(SlackFetchOutcome {
            markdown: output,
            thread_truncated: fetched.truncated,
            users_capped,
        })
    }
}

/// Retry eligibility, derived from [`SlackError::classify`] so retryability
/// stays a single source of truth (mirrors `BraveError::is_degradable`).
/// `RateLimited` keeps its own arm: the cap check needs the raw `retry_after`
/// value, which `classify()` does not carry through.
fn is_retriable(e: &SlackError) -> bool {
    match e {
        SlackError::RateLimited { retry_after } => retry_after_within_cap(*retry_after),
        _ => matches!(
            e.classify().kind,
            ErrorCode::TempFailure | ErrorCode::Timeout
        ),
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct DummyBody {
    ok: bool,
}

#[cfg(test)]
mod constructor_tests;
#[cfg(test)]
mod http_tests;
