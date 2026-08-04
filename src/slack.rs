//! Facade over the Slack backend. This file itself holds [`SlackError`] and
//! its classification; everything else it names comes from a submodule:
//! permalink parsing from [`url`], message resolution and YAML output from
//! [`format`], mention handling from [`mention`], and the token-bearing HTTP
//! client from [`client`].
//!
//! Wire-format structs sit next to their deserialize call sites in [`client`],
//! except `Message`, which [`format`] owns because both modules read it.

use crate::classify::Classification;
use crate::envelope::ErrorCode;

mod client;
pub(crate) use client::SlackClient;

mod format;
pub(in crate::slack) use format::{
    Message, ResolvedMessage, extract_target, format_slack_output, resolve_messages,
};

mod mention;
pub(in crate::slack) use mention::{
    collect_mention_ids_ordered, resolved_display_name, substitute_mentions,
};

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

    /// The payload states what did not respond and within what budget, never
    /// the timeout itself; see `FetchError::Timeout` (src/fetch.rs) for the
    /// doubling that rule prevents.
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

#[cfg(test)]
mod classify_tests;
