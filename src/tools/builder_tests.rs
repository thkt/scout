use super::*;
use crate::clock::FixedClock;
use crate::envelope::ErrorCode;
use crate::fetch::{EgressMode, FailingDnsResolver, StaticDnsResolver};
use crate::redacted::Redacted;
use crate::search::Lang;
use crate::secret_source::SecretSource;
use crate::test_support::{join_server_thread, spawn_forward_proxy, try_spawn_mock_server};
use reqwest::Proxy;
use reqwest::redirect::Policy;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-DNS001] End-to-end check that the `Arc<dyn DnsResolver>` handed to
/// `ScoutBuilder::with_dns` reaches the `Scout.dns` slot and is consulted on
/// `Scout::fetch`'s SSRF path.
///
/// Once the injected `StaticDnsResolver(10.0.0.1)` overrides the DNS lookup
/// for `https://example.com`, `ssrf_check`'s private-IP test returns
/// `FetchError::InternalHost` immediately. The default `TokioDnsResolver`
/// answers `example.com` with a public IP, so this assert fails whenever the
/// injection is not wired.
#[tokio::test]
async fn scout_builder_with_dns_blocks_fetch_via_injected_private_ip() {
    let injected: Arc<dyn DnsResolver> = Arc::new(StaticDnsResolver::single("10.0.0.1"));
    let scout = ScoutBuilder::for_test().with_dns(injected.clone()).build();

    let result = scout
        .run(Command::Fetch(FetchParams::for_test(
            "https://example.com/page",
        )))
        .await;
    let err = result.expect_err("injected private IP must trip SSRF check");
    assert_eq!(
        err.error_kind(),
        ErrorCode::DataError,
        "SSRF InternalHost maps to DataError (sysexits EX_DATAERR)"
    );
    assert!(
        err.message().contains("internal/private"),
        "error message must surface the SSRF cause, got: {}",
        err.message()
    );
}

/// [T-DNS002] Injecting a `FailingDnsResolver` makes `Scout::fetch` return a
/// `ScoutError` originating in `FetchError::DnsResolution`, which pins that the
/// resolver's failure path propagates through the SSRF route.
#[tokio::test]
async fn scout_builder_with_dns_propagates_resolver_failure() {
    let injected: Arc<dyn DnsResolver> =
        Arc::new(FailingDnsResolver("simulated DNS failure".into()));
    let scout = ScoutBuilder::for_test().with_dns(injected).build();

    let result = scout
        .run(Command::Fetch(FetchParams::for_test(
            "https://example.com/page",
        )))
        .await;
    let err = result.expect_err("injected resolver failure must surface as error");
    assert!(
        err.message().contains("DNS resolution failed"),
        "error message must surface the DNS failure cause, got: {}",
        err.message()
    );
}

/// [T-SB004] A reset beyond the retry budget fails after one HTTP request.
#[tokio::test]
async fn github_rate_limit_beyond_budget_fails_without_retries() {
    let Some(server) = try_spawn_mock_server("tools::scout_builder_seam").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(
            ResponseTemplate::new(403)
                .append_header("x-ratelimit-remaining", "0")
                .append_header("x-ratelimit-reset", "1600")
                .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_clock(Arc::new(FixedClock(1000)))
        .with_github_endpoint(&server.uri())
        .build();

    let err = scout
        .run(Command::RepoOverview(RepoOverviewParams {
            repository: Some("owner/repo".into()),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.error_kind(), ErrorCode::TempFailure);
    assert_eq!(err.exit_code(), 75);
}

/// [T-SB005] End-to-end check that the wiremock endpoint injected via
/// `with_slack_endpoint` reaches the `SlackClient` built on the production path
/// `fetch` (slack permalink) → `fetch_slack` → `slack()`. Because `build()`
/// pre-sets `slack()`'s `OnceCell`, the injected client is used even with
/// `SLACK_TOKEN` unset, so the call reaches `conversations.history` and returns
/// the body. When the injection is not wired, `for_test`'s empty secret source
/// answers `TokenNotSet` and the test fails.
#[tokio::test]
async fn scout_builder_slack_endpoint_reaches_fetch_slack_via_seam() {
    let Some(server) = try_spawn_mock_server("tools::scout_builder_slack_seam").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"type": "message", "text": "hello from wiremock", "ts": "1773819598.273499"}
            ]
        })))
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();

    let result = scout
        .fetch(FetchParams::for_test(
            "https://team.slack.com/archives/C123/p1773819598273499",
        ))
        .await;
    let output =
        result.expect("injected slack endpoint must serve fetch_slack without SLACK_TOKEN");
    assert!(
        output.markdown().contains("hello from wiremock"),
        "fetch output must carry the wiremock message body, got: {}",
        output.markdown()
    );
}

/// [T-SB006] `with_egress(Proxied)` drives the `Scout::fetch` → `fetch_page`
/// egress plumbing end-to-end: a public-domain fetch routes through a local
/// forward proxy and returns the body, while the injected `FailingDnsResolver`
/// is never consulted (Proxied skips scout's DNS pre-check). This exercises the
/// `with_egress` seam and proves `query.rs` forwards `Scout.egress` into
/// `FetchOptions.egress`. It complements the fetch_page-level T-F073/T-F074: those
/// pin the proxied client shape, this pins the builder → query wiring that
/// produces it. A regression to `Direct` would consult the resolver and surface
/// as a `DnsResolution` error instead of the body.
#[tokio::test]
async fn scout_builder_with_egress_routes_proxied_fetch_through_proxy() {
    // Rich body (no <script>, >100 visible chars) so `is_js_dependent` /
    // `is_thin_extract` stay false and the CDP fallback never fires.
    let body = "<html><body><h1>Proxied Article</h1><p>proxied body content long \
        enough to clear the thin-body and thin-extract thresholds so the JS \
        rendering fallback path is never taken in this proxied fetch test.</p>\
        </body></html>";
    let Some((proxy_url, handle)) = spawn_forward_proxy(body) else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    // Mirror what production `build_default_clients` builds for Proxied: a
    // proxied, guard-free client (no `SsrfResolver`, which by design blocks the
    // loopback proxy). `for_test`'s default `fetch_http` is the Direct
    // guard-carrying client, so it must be replaced via `with_fetch_http`.
    let fetch_http = reqwest::Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();
    let scout = ScoutBuilder::for_test()
        .with_fetch_http(fetch_http)
        .with_egress(EgressMode::Proxied(proxy_url.clone()))
        .with_dns(Arc::new(FailingDnsResolver(
            "resolver must not be consulted in Proxied mode".into(),
        )))
        .build();

    let result = scout
        .fetch(FetchParams::for_test("http://example.com/page"))
        .await;
    let output = result.expect("proxied fetch of a public URL should succeed");
    assert!(
        output.markdown().contains("proxied body content"),
        "proxied fetch must return the page body via the proxy, got: {}",
        output.markdown()
    );

    join_server_thread(handle);
}

struct CountingTokenSource(Arc<AtomicUsize>);

impl TokenSource for CountingTokenSource {
    fn fetch(&self) -> Pin<Box<dyn Future<Output = Option<Redacted>> + Send + '_>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { None })
    }
}

/// [T-SB007] The first GitHub call resolves the token through the injected
/// source, and later calls reuse that client without resolving again.
#[tokio::test]
async fn github_client_resolves_the_injected_token_source_once() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let scout = ScoutBuilder::for_test()
        .with_token_source(Arc::new(CountingTokenSource(fetches.clone())))
        .build();
    assert_eq!(fetches.load(Ordering::SeqCst), 0, "resolution must be lazy");

    scout.github().await;
    scout.github().await;

    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

/// Records every secret name it is asked for and answers each with `value`.
struct RecordingSecretSource {
    asked: Arc<Mutex<Vec<String>>>,
    value: Option<Redacted>,
}

impl RecordingSecretSource {
    fn new(value: Option<&str>) -> (Arc<Self>, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(Self {
            asked: asked.clone(),
            value: value.and_then(Redacted::new),
        });
        (source, asked)
    }
}

impl SecretSource for RecordingSecretSource {
    fn fetch<'a>(
        &'a self,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<Redacted>> + Send + 'a>> {
        self.asked.lock().expect("asked lock").push(name.to_owned());
        let value = self.value.clone();
        Box::pin(async move { value })
    }
}

fn asked_names(asked: &Mutex<Vec<String>>) -> Vec<String> {
    asked.lock().expect("asked lock").clone()
}

/// [T-SB008] The first search builds the Brave client from the key the
/// injected source resolves for `BRAVE_SEARCH_API_KEY`, and later calls reuse
/// that client without resolving again.
#[tokio::test]
async fn brave_client_resolves_its_key_through_the_secret_source_once() {
    let (source, asked) = RecordingSecretSource::new(Some("key-from-source"));
    let scout = ScoutBuilder::for_test().with_secret_source(source).build();
    assert!(asked_names(&asked).is_empty(), "resolution must be lazy");

    scout
        .brave()
        .await
        .expect("a resolved key builds the client");
    scout.brave().await.expect("the cached client is reused");

    assert_eq!(asked_names(&asked), ["BRAVE_SEARCH_API_KEY"]);
}

/// [T-SB009] A search whose source resolves no key fails with the usage error
/// that names `BRAVE_SEARCH_API_KEY`.
#[tokio::test]
async fn search_without_a_resolved_key_is_a_usage_error() {
    let (source, asked) = RecordingSecretSource::new(None);
    let scout = ScoutBuilder::for_test().with_secret_source(source).build();

    let err = scout
        .run(Command::Search(SearchParams {
            query: Some("rust".into()),
            lang: Lang::Auto,
        }))
        .await
        .unwrap_err();

    assert_eq!(err.error_kind(), ErrorCode::UsageError);
    assert_eq!(asked_names(&asked), ["BRAVE_SEARCH_API_KEY"]);
}

/// [T-SB010] The first Slack fetch builds the client from the token the
/// injected source resolves for `SLACK_TOKEN`, and the `xoxp-` check applies
/// to it.
#[tokio::test]
async fn slack_client_resolves_its_token_through_the_secret_source() {
    let (source, asked) = RecordingSecretSource::new(Some("xoxp-from-source"));
    let scout = ScoutBuilder::for_test().with_secret_source(source).build();
    scout.slack().await.expect("a user token builds the client");
    assert_eq!(asked_names(&asked), ["SLACK_TOKEN"]);

    let (bot_source, _) = RecordingSecretSource::new(Some("xoxb-from-source"));
    let scout = ScoutBuilder::for_test()
        .with_secret_source(bot_source)
        .build();
    let err = scout.slack().await.map(|_| ()).unwrap_err();
    assert_eq!(err.error_kind(), ErrorCode::UsageError);
}

/// [T-SB011] `repo-overview` and a non-Slack `fetch` never ask for a secret, so
/// they never run the Keychain lookup.
#[tokio::test]
async fn repo_and_non_slack_fetch_never_resolve_a_secret() {
    let (source, asked) = RecordingSecretSource::new(Some("unused"));
    let scout = ScoutBuilder::for_test()
        .with_secret_source(source)
        .with_dns(Arc::new(StaticDnsResolver::single("10.0.0.1")))
        .with_github_endpoint("http://127.0.0.1:1")
        .build();

    let _ = scout
        .run(Command::Fetch(FetchParams::for_test(
            "https://example.com/page",
        )))
        .await;
    let _ = scout
        .run(Command::RepoOverview(RepoOverviewParams {
            repository: Some("owner/repo".into()),
        }))
        .await;

    assert!(
        asked_names(&asked).is_empty(),
        "asked: {:?}",
        asked_names(&asked)
    );
}

/// [T-SB012] `for_test` resolves no Brave key or Slack token, so a test that
/// does not inject an endpoint or a source never sees the developer's env var
/// or Keychain.
#[tokio::test]
async fn for_test_resolves_no_secret() {
    let scout = ScoutBuilder::for_test().build();

    let brave = scout.brave().await.map(|_| ()).unwrap_err();
    let slack = scout.slack().await.map(|_| ()).unwrap_err();

    assert_eq!(brave.error_kind(), ErrorCode::UsageError);
    assert_eq!(slack.error_kind(), ErrorCode::UsageError);
}
