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
    /// Configuration errors (`ApiKeyNotSet`, `Unauthorized`, `ParseUrl`,
    /// `InsecureBaseUrl`), the scout-side invariants `ParseJson` and
    /// `ResponseTooLarge`, and 4xx `Api` codes stay propagated — they require
    /// user action or signal a scout/upstream invariant violation.
    pub(crate) fn is_degradable(&self) -> bool {
        match self {
            Self::ApiKeyNotSet
            | Self::Unauthorized
            | Self::ParseJson(_)
            | Self::ResponseTooLarge
            | Self::ParseUrl(_)
            | Self::InsecureBaseUrl => false,
            Self::Api { code, .. } if (400..500).contains(code) => false,
            Self::RateLimited { .. } | Self::Server(_) | Self::Network(_) | Self::Api { .. } => {
                true
            }
        }
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
            // Priority 2: DATA_ERROR (4xx body, URL parse failure, or insecure base URL)
            Self::ParseUrl(_) | Self::InsecureBaseUrl => Classification::new(ErrorCode::DataError),
            Self::Api { code, .. } if (400..500).contains(code) => {
                Classification::new(ErrorCode::DataError)
            }
            // Priority 4: TIMEOUT
            Self::Network(re) if re.is_timeout() => Classification::timeout_retry(),
            // Priority 4: TEMP_FAILURE
            Self::RateLimited { .. } => Classification::transient_retry(),
            Self::Server(_) => Classification::transient_retry(),
            Self::Network(_) => Classification::transient_network(),
            Self::Api { code, .. } if (500..=599).contains(code) => {
                Classification::transient_retry()
            }
            // Priority 5: INTERNAL — schema drift is a scout-side invariant;
            // peer to `GitHubError::Decode` / `SlackError::Decode`. Oversized
            // body is an upstream invariant violation (Brave returning >1 MiB
            // on `web/search`), classified the same as schema drift because
            // it signals the API surface drifted and retry will not recover.
            Self::ParseJson(_) | Self::ResponseTooLarge => Classification::new(ErrorCode::Internal),
            // Unknown — Api codes that did not match 4xx or 5xx
            Self::Api { .. } => Classification::new(ErrorCode::Unknown),
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
            .header("User-Agent", crate::USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?;

        let response = classify_response(response, self.clock.as_ref()).await?;
        let bytes =
            read_body_capped(response, || BraveError::ResponseTooLarge, BraveError::from).await?;
        let parsed: WebSearchResponse = serde_json::from_slice(&bytes)?;

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
            || BraveError::RateLimited { retry_after: None },
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
mod http_tests {
    use super::*;
    use crate::test_support::try_spawn_mock_server;
    use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, ResponseTemplate};

    fn ok_body() -> serde_json::Value {
        serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://example.com", "title": "Example", "description": "snippet"}
                ]
            }
        })
    }

    /// [T-BC-LOG001] (issue #166 / OPS-003)
    /// Setup: wiremock returns a 1-result Brave payload.
    /// Action: `client.search("foo", None)` is invoked under `traced_test`.
    /// Expected: an INFO-level `Brave search dispatching` event fires before
    /// dispatch, and an INFO-level `Brave search complete` event fires after,
    /// carrying `result_count` and `elapsed_ms` structured fields. Operators
    /// at the default `info` log level can attribute latency without enabling
    /// `RUST_LOG=debug`.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn search_emits_info_dispatch_and_complete_events() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        client.search("foo", None).await.unwrap();

        assert!(
            logs_contain("Brave search dispatching"),
            "expected INFO dispatch event before the HTTP call"
        );
        assert!(
            logs_contain("Brave search complete"),
            "expected INFO completion event after the HTTP call"
        );
        assert!(
            logs_contain("result_count=1"),
            "completion event should carry result_count"
        );
        assert!(
            logs_contain("elapsed_ms"),
            "completion event should carry elapsed_ms for latency attribution"
        );
    }

    /// [T-001] BraveClient sends query unmodified with q parameter
    #[tokio::test]
    async fn search_sends_query_unmodified() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("q", "foo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, "https://example.com");
    }

    /// [T-002] BraveClient includes search_lang=ja when Lang::Ja maps to "ja"
    #[tokio::test]
    async fn search_includes_search_lang_ja() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(query_param("q", "foo"))
            .and(query_param("search_lang", "ja"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", Some("ja")).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    /// [T-003] BraveClient includes search_lang=en when Lang::En maps to "en"
    #[tokio::test]
    async fn search_includes_search_lang_en() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(query_param("q", "foo"))
            .and(query_param("search_lang", "en"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", Some("en")).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    /// [T-004] BraveClient omits search_lang when None is provided
    #[tokio::test]
    async fn search_omits_search_lang_for_auto() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(query_param("q", "foo"))
            .and(query_param_is_missing("search_lang"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    /// [T-005] BraveClient sends X-Subscription-Token header with api key
    #[tokio::test]
    async fn search_sends_subscription_token_header() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(header("X-Subscription-Token", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    /// [T-006] search recovers when 429 transient response is followed by 200
    #[tokio::test]
    async fn search_retries_after_429_then_succeeds() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };

        // First call: 429 with short Retry-After to keep test fast
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Subsequent calls: 200
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await.unwrap();
        assert_eq!(result.len(), 1);
    }

    /// [T-007] search returns RateLimited when 429 persists across retries
    #[tokio::test]
    async fn search_429_persistent_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(
            matches!(result, Err(BraveError::RateLimited { .. })),
            "expected RateLimited, got: {result:?}"
        );
    }

    /// [T-008] search returns Unauthorized without retry on 401
    #[tokio::test]
    async fn search_401_returns_unauthorized() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1) // exactly one call, no retries
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(
            matches!(result, Err(BraveError::Unauthorized)),
            "expected Unauthorized, got: {result:?}"
        );
    }

    /// [T-026] (unit / FR-019)
    /// Setup: wiremock always returns HTTP 403.
    /// Action: `client.search("foo", None)` is invoked.
    /// Expected: returns `BraveError::Unauthorized`; no retry (mock call count = 1)
    /// because 403/401 are auth-class failures and not retriable.
    #[tokio::test]
    async fn search_403_returns_unauthorized() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1) // exactly one call, no retries
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(
            matches!(result, Err(BraveError::Unauthorized)),
            "expected Unauthorized for 403, got: {result:?}"
        );
    }

    /// [T-023] search returns ServerError(503) after retries on persistent 503
    #[tokio::test]
    async fn search_503_persistent_returns_server_error() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(
            matches!(result, Err(BraveError::Server(503))),
            "expected ServerError(503), got: {result:?}"
        );
    }

    /// [T-BC-CAP001] (issue #165 / CHX-008)
    /// Setup: wiremock returns a 2xx whose body exceeds `MAX_API_RESPONSE_BYTES`
    /// (1 MiB), simulating an upstream Brave deployment returning unbounded
    /// JSON.
    /// Action: `client.search("foo", None)` is invoked.
    /// Expected: returns `BraveError::ResponseTooLarge`; no retry (mock call
    /// count = 1) because the variant is not retriable.
    #[tokio::test]
    async fn search_oversized_body_returns_too_large() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        // 1 MiB + 1 byte trips the cap regardless of pre-check vs chunk path.
        let body = vec![b'x'; (1024 * 1024) + 1];
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(1)
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        assert!(
            matches!(result, Err(BraveError::ResponseTooLarge)),
            "expected ResponseTooLarge, got: {result:?}"
        );
    }

    /// [T-024] search returns ParseJson error when response body is malformed JSON
    #[tokio::test]
    async fn search_malformed_json_returns_parse_error() {
        let Some(server) = try_spawn_mock_server("brave::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"web\":"))
            .mount(&server)
            .await;

        let client = BraveClient::with_base_url(Client::new(), &server.uri());
        let result = client.search("foo", None).await;
        match result {
            Err(BraveError::ParseJson(e)) => {
                let msg = e.to_string();
                assert!(!msg.is_empty(), "parse error message should not be empty");
                assert!(
                    msg.contains("EOF") || msg.contains("expected"),
                    "serde diagnostic expected (EOF/expected token), got: {msg}"
                );
            }
            other => panic!("expected ParseJson, got: {other:?}"),
        }
    }

    // T-RC001: from_env_with_returns_api_key_not_set_when_closure_errs
    /// FR-001 / FR-002: closure returning `Err(VarError::NotPresent)` must surface
    /// as `BraveError::ApiKeyNotSet` from `from_env_with`. Exercises the injectable
    /// env path that `from_env` delegates to.
    #[test]
    fn from_env_with_returns_api_key_not_set_when_closure_errs() {
        let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
            Err(env::VarError::NotPresent)
        });
        assert!(
            matches!(result, Err(BraveError::ApiKeyNotSet)),
            "expected ApiKeyNotSet, got: {result:?}"
        );
    }

    // T-RC002: from_env_with_rejects_whitespace_only_key
    /// FR-003: closure returning a whitespace-only string must be trimmed and rejected
    /// as `ApiKeyNotSet` (parity with the previous `trim().is_empty()` check in
    /// `from_env`).
    #[test]
    fn from_env_with_rejects_whitespace_only_key() {
        let result =
            BraveClient::from_env_with(
                Client::new(),
                DEFAULT_MAX_RETRIES,
                |_| Ok("   ".to_owned()),
            );
        assert!(
            matches!(result, Err(BraveError::ApiKeyNotSet)),
            "expected ApiKeyNotSet for whitespace-only key, got: {result:?}"
        );
    }

    // T-RC003: from_env_with_constructs_client_with_api_base_and_exposed_key
    /// FR-001 / FR-003: closure returning a real key must yield `Ok(client)` whose
    /// `api_key` round-trips through `Redacted::expose()` and whose `base_url` equals
    /// the constant `API_BASE`.
    #[test]
    fn from_env_with_constructs_client_with_api_base_and_exposed_key() {
        let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
            Ok("real-key".to_owned())
        });
        let client = result.expect("expected Ok(client) from valid key");
        assert_eq!(client.api_key.expose(), "real-key");
        assert_eq!(client.base_url, API_BASE);
    }

    // T-RC006: from_env_with_does_not_set_skip_https_check
    /// FR-010: production constructor path must not enable the test-only HTTPS bypass.
    /// `skip_https_check` is a `#[cfg(test)]` field; under `cargo test` it exists and
    /// must be `false` when the client comes from `from_env_with`.
    #[test]
    fn from_env_with_does_not_set_skip_https_check() {
        let client =
            BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| Ok("k".to_owned()))
                .expect("expected Ok(client) from valid key");
        assert!(
            !client.skip_https_check,
            "production constructor must not skip HTTPS check"
        );
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    /// [T-BRC001] ApiKeyNotSet classifies as UsageError with BRAVE_SEARCH_API_KEY hint.
    #[test]
    fn api_key_not_set_is_usage_error_with_key_hint() {
        let c = BraveError::ApiKeyNotSet.classify();
        assert_eq!(c.kind, ErrorCode::UsageError);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("BRAVE_SEARCH_API_KEY")),
            "expected BRAVE_SEARCH_API_KEY hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-BRC002] Unauthorized classifies as UsageError with a Brave dashboard hint.
    #[test]
    fn unauthorized_is_usage_error_with_dashboard_hint() {
        let c = BraveError::Unauthorized.classify();
        assert_eq!(c.kind, ErrorCode::UsageError);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("api-dashboard.search.brave.com")),
            "expected Brave dashboard hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-BRC003] Priority-2 DataError variants classify as DataError.
    #[test]
    fn data_error_variants_classify_as_data_error() {
        let cases: Vec<BraveError> = vec![
            BraveError::InsecureBaseUrl,
            BraveError::Api {
                code: 400,
                message: "bad".into(),
            },
            BraveError::Api {
                code: 422,
                message: "unprocessable".into(),
            },
        ];
        for case in &cases {
            assert_eq!(case.classify().kind, ErrorCode::DataError, "{case:?}");
        }
    }

    /// [T-BRC004] Server (5xx) and RateLimited classify as TempFailure.
    #[test]
    fn server_and_rate_limited_are_temp_failure() {
        let cases: Vec<BraveError> = vec![
            BraveError::Server(503),
            BraveError::RateLimited { retry_after: None },
            BraveError::Api {
                code: 502,
                message: "bad gateway".into(),
            },
        ];
        for case in &cases {
            assert_eq!(case.classify().kind, ErrorCode::TempFailure, "{case:?}");
        }
    }

    /// [T-BRC005] Schema drift variants classify as Internal per ADR-0011 priority 5.
    /// `ResponseTooLarge` and `ParseJson` both signal an upstream invariant violation
    /// that retry will not recover from.
    #[test]
    fn schema_drift_is_internal() {
        let serde_err =
            serde_json::from_str::<serde_json::Value>("{not valid").expect_err("malformed json");
        let cases: Vec<BraveError> = vec![
            BraveError::ResponseTooLarge,
            BraveError::ParseJson(serde_err),
        ];
        for case in &cases {
            assert_eq!(case.classify().kind, ErrorCode::Internal, "{case:?}");
        }
    }

    /// [T-BRC006] Api codes outside 4xx/5xx classify as Unknown (escape hatch).
    #[test]
    fn api_non_4xx_5xx_is_unknown() {
        let c = BraveError::Api {
            code: 304,
            message: "not modified".into(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::Unknown);
    }
}
