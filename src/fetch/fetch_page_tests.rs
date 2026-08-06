use super::ssrf::EgressMode;
use super::*;
use crate::test_support::{
    join_server_thread, no_redirect_client, spawn_forward_proxy, try_spawn_mock_server,
};
use reqwest::Proxy;
use reqwest::redirect::Policy;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn real_resolver() -> Arc<dyn DnsResolver> {
    Arc::new(TokioDnsResolver)
}

/// [T-F017]
#[tokio::test]
async fn blocks_ssrf_to_localhost() {
    let client = no_redirect_client();
    let (cancel, _) = watch::channel(false);
    let result = fetch_page(
        &client,
        "http://127.0.0.1/secret",
        FetchOptions::default(),
        real_resolver(),
        &cancel,
    )
    .await;
    assert!(matches!(result, Err(FetchError::InternalHost)));
}

/// [T-F076] SSRF-blocked fetch redacts userinfo credentials from the warn! log
///
/// Adversarial: even when SSRF blocks the fetch, the `warn!` line emitted
/// by `ssrf_check` MUST flow through `redact_url_credentials` so no
/// password fragment ever appears in stderr / `tracing` output.
#[tokio::test]
#[tracing_test::traced_test]
async fn fetch_does_not_log_userinfo_credentials_on_blocked_url() {
    let client = no_redirect_client();
    let (cancel, _) = watch::channel(false);
    let result = fetch_page(
        &client,
        "http://user:supersecret@127.0.0.1/private",
        FetchOptions::default(),
        real_resolver(),
        &cancel,
    )
    .await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "should be blocked as InternalHost, got: {result:?}"
    );
    // Positive anchor: a future refactor that drops the warn! line
    // entirely would silently make the userinfo asserts vacuous.
    assert!(
        logs_contain("blocked fetch to internal/private host"),
        "expected the SSRF block warning to fire",
    );
    assert!(
        !logs_contain("supersecret"),
        "password fragment must not appear in logs",
    );
    assert!(
        !logs_contain("user:"),
        "userinfo must be stripped from logs",
    );
}

/// [T-F072]
///
/// ADR-0012 contract pin: the pre-flight `ssrf_check` resolver returns a public
/// IP (passing pre-flight), while the `fetch_http` client's injected
/// `SsrfResolver` resolves the host to a private IP at connect time (DNS
/// rebinding). The fetch must fail AND emit `"blocked connect to private IP"`.
/// The log assertion is non-tautological: a broken connect-time guard would
/// still yield `is_err()` via a real connect failure but emit no such warn.
/// A domain (not an IP literal) is used so reqwest consults the resolver.
#[tokio::test]
#[tracing_test::traced_test]
async fn fetch_blocks_dns_rebind_at_connect_time() {
    let client = Client::builder()
        .redirect(Policy::none())
        .dns_resolver(Arc::new(SsrfResolver::new(StaticDnsResolver::single(
            "10.0.0.1",
        ))))
        .build()
        .unwrap();
    let preflight: Arc<dyn DnsResolver> = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let (cancel, _) = watch::channel(false);
    let result = fetch_page(
        &client,
        "http://rebind.example.com/",
        FetchOptions::default(),
        preflight,
        &cancel,
    )
    .await;
    assert!(
        result.is_err(),
        "DNS rebind to private IP must be blocked at connect, got: {result:?}"
    );
    assert!(
        logs_contain("blocked connect to private IP"),
        "expected the connect-time SSRF guard to fire",
    );
}

/// [T-F018]
#[tokio::test]
async fn js_flag_attempts_rendering_on_rich_body() {
    let content = "x".repeat(200);
    let Some(server) = try_spawn_mock_server("fetch::download").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/rich"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<html><body><p>{content}</p></body></html>")),
        )
        .mount(&server)
        .await;

    let client = no_redirect_client();
    let opts = FetchOptions {
        js: true,
        ..Default::default()
    };
    let (cancel, _) = watch::channel(false);
    let result = fetch_page(
        &client,
        &format!("{}/rich", server.uri()),
        opts,
        real_resolver(),
        &cancel,
    )
    .await;

    assert!(
        result.is_err(),
        "js=true should error when browser unavailable"
    );
}

/// [T-F019]
#[cfg(not(feature = "js-rendering"))]
#[tokio::test]
async fn t010_js_flag_errors_when_feature_disabled() {
    let client = no_redirect_client();
    let opts = FetchOptions {
        js: true,
        ..Default::default()
    };
    let (cancel, _) = watch::channel(false);
    let result = fetch_page(
        &client,
        "https://example.com/page",
        opts,
        real_resolver(),
        &cancel,
    )
    .await;

    assert!(
        matches!(&result, Err(FetchError::BrowserNotFound(msg)) if msg.contains("js-rendering")),
        "expected BrowserNotFound error with feature hint, got: {result:?}"
    );
}

/// [T-F073]
#[tokio::test]
async fn with_a_proxy_configured_fetch_page_returns_the_page_body_for_a_public_domain_url_routed_through_a_local_forward_proxy_while_the_dns_resolver_is_never_consulted()
 {
    // Rich body (no <script>, >100 visible chars) so `is_js_dependent` /
    // `is_thin_extract` stay false and the CDP fallback never fires.
    let body = "<html><body><h1>Proxied Article</h1><p>proxied body content long \
        enough to clear the thin-body and thin-extract thresholds so the JS \
        rendering fallback path is never taken in this proxied fetch test.</p>\
        </body></html>";
    let Some((proxy_url, handle)) = spawn_forward_proxy(body) else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    // Mirrors what ScoutBuilder builds in Proxied mode: an explicit `Proxy::all`
    // and NO `SsrfResolver` connect-time guard (which by design blocks loopback,
    // where the local proxy listens). `Proxy::all` per
    // https://docs.rs/reqwest/0.13/reqwest/struct.Proxy.html#method.all
    let client = Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();

    // `FailingDnsResolver` errors the instant it is consulted. Proxied egress
    // must skip scout's DNS pre-check, so success here proves the resolver was
    // never called; a regression to Direct would surface as
    // `FetchError::DnsResolution`.
    let resolver: Arc<dyn DnsResolver> = Arc::new(FailingDnsResolver(
        "resolver must not be consulted in Proxied mode".to_owned(),
    ));
    let (cancel, _) = watch::channel(false);
    let opts = FetchOptions {
        egress: EgressMode::Proxied(proxy_url.clone()),
        ..Default::default()
    };
    let result = fetch_page(&client, "http://example.com/page", opts, resolver, &cancel).await;

    let page = result.expect("proxied fetch of a public URL should succeed");
    assert!(
        page.markdown().contains("proxied body content"),
        "proxied fetch should return the page body, got: {:?}",
        page.markdown()
    );

    join_server_thread(handle);
}

/// [T-F074]
#[tokio::test]
async fn with_a_proxy_configured_fetch_page_to_a_literal_loopback_url_is_blocked_before_any_request_reaches_the_proxy()
 {
    // Proxied egress skips scout's DNS pre-check, but `validate_url_sync` still
    // rejects a literal loopback host in every mode (it runs before the Proxied
    // early-return in `ssrf_check`). The proxy points at a dead port: were the
    // literal block absent, the request would reach the proxy and fail as a
    // connection error (`FetchError::Http`), so asserting `InternalHost` proves
    // the block fired before any request left scout.
    let client = no_redirect_client();
    let resolver: Arc<dyn DnsResolver> = Arc::new(TokioDnsResolver);
    let (cancel, _) = watch::channel(false);
    let opts = FetchOptions {
        egress: EgressMode::Proxied("http://127.0.0.1:9".to_owned()),
        ..Default::default()
    };
    let result = fetch_page(&client, "http://127.0.0.1/secret", opts, resolver, &cancel).await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "loopback URL must be blocked before reaching the proxy, got: {result:?}"
    );
}
