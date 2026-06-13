use super::*;
use crate::test_support::{no_redirect_client, try_spawn_mock_server};
use reqwest::redirect::Policy;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn real_resolver() -> Arc<dyn DnsResolver> {
    Arc::new(TokioDnsResolver)
}

/// [T-F017] blocks_ssrf_to_localhost
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

/// [T-F052] fetch_does_not_log_userinfo_credentials_on_blocked_url
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

/// [T-003] fetch_blocks_dns_rebind_at_connect_time
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

/// [T-F018] js_flag_attempts_rendering_on_rich_body
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

/// [T-F019] t010_js_flag_errors_when_feature_disabled
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
