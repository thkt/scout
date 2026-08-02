use std::env;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use tracing::{info, warn};

use crate::clock::{Clock, SystemClock};
use crate::envelope::ErrorCode;
use crate::redacted::{Redacted, validate_https};
#[cfg(test)]
use crate::retry::DEFAULT_MAX_RETRIES;
use crate::retry::{
    MAX_API_RESPONSE_BYTES, is_transient_network, parse_retry_after, read_body_capped,
    retry_after_within_cap, retry_with_rate_limit,
};
use crate::rng::{FastrandRng, Rng};
use crate::tools::Classification;

use super::types::{SearchResult, WebSearchResponse};

const API_BASE: &str = "https://api.search.brave.com/res/v1/web/search";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const BODY_SNIPPET_BYTES: usize = 200;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BraveError {
    #[error("BRAVE_SEARCH_API_KEY not set. Get one at https://api-dashboard.search.brave.com/")]
    ApiKeyNotSet,

    #[error("API rate limit exceeded. Please retry later.")]
    RateLimited { retry_after: Option<u64> },

    #[error("Unauthorized: check your BRAVE_SEARCH_API_KEY")]
    Unauthorized,

    #[error("Brave API server error (HTTP {0})")]
    Server(u16),

    #[error("Failed to parse Brave API response: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("Brave API response too large (>{} bytes)", MAX_API_RESPONSE_BYTES)]
    ResponseTooLarge,

    #[error("Invalid Brave API URL: {0}")]
    ParseUrl(#[from] url::ParseError),

    #[error("API error ({code}): {message}")]
    Api { code: u16, message: String },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Insecure base URL: HTTPS required")]
    InsecureBaseUrl,
}

impl BraveError {
    /// Returns `true` when the error is a transient infrastructure failure that callers
    /// may legitimately surface as a degraded result instead of propagating.
    ///
    /// Derived from [`classify`](Self::classify) so the degradable set stays a
    /// single source of truth: only `TempFailure` and `Timeout` (retryable
    /// infrastructure faults) degrade. Everything else propagates, including
    /// the `Unknown` escape hatch — a non-4xx/5xx `Api` code surfaces as an
    /// error rather than masking an unrecognized status as an empty result.
    pub(crate) fn is_degradable(&self) -> bool {
        matches!(
            self.classify().kind,
            ErrorCode::TempFailure | ErrorCode::Timeout
        )
    }

    /// Map each variant to its ADR-0011 priority-table [`Classification`].
    ///
    /// Arm order is load-bearing: `Api { code: 4xx }` precedes the bare
    /// `Api { .. }` fallback so a reorder cannot silently demote a 4xx
    /// response from DataError to Unknown.
    pub(crate) fn classify(&self) -> Classification {
        match self {
            // Priority 1: USAGE_ERROR / config
            Self::ApiKeyNotSet => Classification::new(ErrorCode::UsageError)
                .with_hint("Set BRAVE_SEARCH_API_KEY environment variable"),
            Self::Unauthorized => Classification::new(ErrorCode::UsageError).with_hint(
                "Verify BRAVE_SEARCH_API_KEY at https://api-dashboard.search.brave.com/",
            ),
            // Priority 2: DATA_ERROR (URL parse failure or insecure base URL)
            Self::ParseUrl(_) | Self::InsecureBaseUrl => Classification::new(ErrorCode::DataError),
            // Priority 4: TEMP_FAILURE
            Self::RateLimited { .. } => Classification::transient_retry(),
            Self::Server(code) => Classification::from_http_status(*code),
            // Priority 4 (TIMEOUT) and 退避: see `Classification::from_reqwest`
            Self::Network(re) => Classification::from_reqwest(re),
            // Priority 5: INTERNAL — schema drift is a scout-side invariant;
            // peer to `GitHubError::Decode` / `SlackError::Decode`. Oversized
            // body is an upstream invariant violation (Brave returning >1 MiB
            // on `web/search`), classified the same as schema drift because
            // it signals the API surface drifted and retry will not recover.
            Self::ParseJson(_) | Self::ResponseTooLarge => Classification::new(ErrorCode::Internal),
            // Every remaining status follows the ADR-0003 table.
            Self::Api { code, .. } => Classification::from_http_status(*code),
        }
    }
}

pub(crate) trait SearchClient {
    async fn search(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<Vec<SearchResult>, BraveError>;
}

#[derive(Clone)]
pub(crate) struct BraveClient {
    http: Client,
    api_key: Redacted,
    base_url: String,
    max_retries: u32,
    /// Wall-clock source for `secs_until_ratelimit_reset` / Retry-After
    /// arithmetic. Set at construction and read on every retry; defaults to
    /// `SystemClock`. Mirrors `GitHubClient`'s injection seam.
    clock: Arc<dyn Clock>,
    /// Backoff jitter source handed to `retry_with_rate_limit` per attempt.
    /// Set at construction; defaults to `FastrandRng`.
    rng: Arc<dyn Rng>,
    /// Test-only escape hatch for wiremock servers on `http://127.0.0.1`.
    /// Production constructors leave this `false` so `send_request` always
    /// runs `validate_https`; only `with_base_url` opts in.
    #[cfg(test)]
    skip_https_check: bool,
}

// Manual Debug because `clock` and `rng` are `dyn Trait` without Debug
// bounds. `api_key` is intentionally not exposed so accidental `{client:?}`
// in logs cannot leak the Brave secret.
impl fmt::Debug for BraveClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BraveClient")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl BraveClient {
    pub(crate) fn from_env(http: Client, max_retries: u32) -> Result<Self, BraveError> {
        Self::from_env_with(http, max_retries, |k| env::var(k))
    }

    /// Wraps [`Self::from_env`] with a caller-supplied env reader so unit
    /// tests can exercise the env-not-set / whitespace branches without
    /// `unsafe { std::env::set_var(...) }` (forbidden by `unsafe_code = "forbid"`).
    pub(crate) fn from_env_with<F>(
        http: Client,
        max_retries: u32,
        get_var: F,
    ) -> Result<Self, BraveError>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        let api_key = get_var("BRAVE_SEARCH_API_KEY").map_err(|_| BraveError::ApiKeyNotSet)?;
        let api_key = Redacted::new(&api_key).ok_or(BraveError::ApiKeyNotSet)?;
        Ok(Self {
            http,
            api_key,
            base_url: API_BASE.to_owned(),
            max_retries,
            clock: Arc::new(SystemClock),
            rng: Arc::new(FastrandRng),
            #[cfg(test)]
            skip_https_check: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(http: Client, base_url: &str) -> Self {
        Self {
            http,
            api_key: Redacted::new("test-key").expect("static literal is non-empty"),
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

    async fn send_request(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<WebSearchResponse, BraveError> {
        if self.should_check_https() {
            validate_https(&self.base_url, || BraveError::InsecureBaseUrl)?;
        }
        let url = build_url(&self.base_url, query, search_lang)?;
        let query_len = query.len();

        // Bracket the call with info events so operators can attribute
        // latency from the default log level. `query_len` (not `query`)
        // keeps the user term out of logs.
        info!(query_len, "Brave search dispatching");
        let started = Instant::now();

        let response = self
            .http
            .get(url)
            .header("X-Subscription-Token", self.api_key.expose())
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?;

        let response = classify_response(response, self.clock.as_ref()).await?;
        let bytes = read_body_capped(
            response,
            MAX_API_RESPONSE_BYTES,
            || BraveError::ResponseTooLarge,
            BraveError::from,
        )
        .await?;
        let parsed: WebSearchResponse = serde_json::from_slice(&bytes).inspect_err(|e| {
            warn!(query_len, error = %e, "Brave search response parse failed");
        })?;

        info!(
            query_len,
            result_count = parsed.web.as_ref().map_or(0, |w| w.results.len()),
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "Brave search complete"
        );
        Ok(parsed)
    }
}

fn build_url(
    base_url: &str,
    query: &str,
    search_lang: Option<&str>,
) -> Result<reqwest::Url, BraveError> {
    // Intentionally omit count/offset/safesearch/freshness/country/ui_lang —
    // accept Brave defaults (safesearch=moderate, count=20). Adding params is
    // additive; introduce them only when a caller surfaces a concrete need.
    let mut params = vec![("q", query)];
    if let Some(lang) = search_lang {
        params.push(("search_lang", lang));
    }
    Ok(reqwest::Url::parse_with_params(base_url, &params)?)
}

async fn classify_response(
    response: reqwest::Response,
    clock: &dyn Clock,
) -> Result<reqwest::Response, BraveError> {
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = parse_retry_after(response.headers(), clock);
        warn!(retry_after_secs = retry_after, "Brave API rate limited");
        return Err(BraveError::RateLimited { retry_after });
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        warn!(status = %status, "Brave API unauthorized");
        return Err(BraveError::Unauthorized);
    }
    if status.is_server_error() {
        warn!(status = %status, "Brave API server error");
        return Err(BraveError::Server(status.as_u16()));
    }
    if !status.is_success() {
        let snippet = read_error_body_snippet(response, status).await;
        warn!(status = %status, "Brave API error");
        return Err(BraveError::Api {
            code: status.as_u16(),
            message: format!("HTTP {status}: {snippet}"),
        });
    }
    Ok(response)
}

async fn read_error_body_snippet(
    response: reqwest::Response,
    status: reqwest::StatusCode,
) -> String {
    let mut text = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            warn!(status = %status, error = %e, "Brave API error; body unreadable");
            return "(body unreadable)".to_owned();
        }
    };
    if text.len() > BODY_SNIPPET_BYTES {
        text.truncate(text.floor_char_boundary(BODY_SNIPPET_BYTES));
    }
    text
}

impl SearchClient for BraveClient {
    async fn search(
        &self,
        query: &str,
        search_lang: Option<&str>,
    ) -> Result<Vec<SearchResult>, BraveError> {
        let response = retry_with_rate_limit(
            || self.send_request(query, search_lang),
            self.max_retries,
            is_retriable,
            |e| match e {
                BraveError::RateLimited { retry_after } => *retry_after,
                _ => None,
            },
            self.rng.as_ref(),
        )
        .await?;
        Ok(response.into_results())
    }
}

fn is_retriable(e: &BraveError) -> bool {
    match e {
        BraveError::RateLimited { retry_after } => retry_after_within_cap(*retry_after),
        BraveError::Server(_) => true,
        BraveError::Network(e) => is_transient_network(e),
        // Oversized body is an upstream invariant violation (issue #165 /
        // CHX-008), not transient — retry cannot shrink the response.
        BraveError::ResponseTooLarge => false,
        _ => false,
    }
}

#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod http_tests;
