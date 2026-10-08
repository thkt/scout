use super::ssrf::EgressMode;
use super::*;
use crate::test_support::{join_server_thread, no_redirect_client, spawn_forward_proxy};
use reqwest::Proxy;
use reqwest::redirect::Policy;
use std::io;
use std::thread::JoinHandle;

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

/// [T-F076] SSRF refusal must still redact credentials from the warning.
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

/// [T-F072] A public pre-flight address followed by a private connect-time
/// address must trigger the DNS-rebind guard (ADR-0012). A domain forces
/// resolver use; the warning distinguishes rejection from connection failure.
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

    // Match production proxied egress: the proxy dials destinations, so the
    // connect-time resolver must not reject the loopback proxy itself.
    let client = Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();

    // Success with a failing resolver proves Proxied mode skips the DNS pre-check.
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

/// Readability-friendly article shell. Filler clears the thin-body/extract
/// thresholds so payload tests do not launch CDP or take raw fallback.
fn article_page(title: &str, payload: &str) -> String {
    format!(
        "<html><head><title>{title}</title></head><body>\
        <nav>Site navigation: Home About Blog Contact archives categories tags</nav>\
        <article>\
        <h1>{title}</h1>\
        <p>This article walks through the topic in enough depth that the page \
        carries real prose rather than a stub, which is what Readability scores \
        when it decides whether the body is worth extracting at all.</p>\
        <p>The second paragraph continues that discussion so the extracted body \
        stays comfortably above the thin-extract threshold, and the fetch does \
        not take the raw-HTML fallback or the JS-rendering detour.</p>\
        <p>The fragment below is the part under test; everything around it is \
        chrome and filler chosen so that it cannot be the reason extraction \
        succeeds or fails.</p>\
        {payload}\
        <p>A closing paragraph follows the fragment so it sits inside the body \
        rather than at its edge, matching how a real page surrounds the markup \
        a reader came for.</p>\
        </article>\
        <footer>Site footer: copyright notice and additional links</footer>\
        </body></html>"
    )
}

/// Fetch through a local forward proxy; direct loopback URLs are SSRF-blocked.
/// `None` preserves the shared unavailable-bind skip policy.
async fn fetch_article_via_proxy(
    html: &str,
    configure_opts: impl FnOnce(FetchOptions) -> FetchOptions,
) -> Option<(Result<FetchResult, FetchError>, JoinHandle<io::Result<()>>)> {
    let (proxy_url, handle) = spawn_forward_proxy(html)?;
    let client = Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();
    let (cancel, _) = watch::channel(false);
    let opts = configure_opts(FetchOptions {
        egress: EgressMode::Proxied(proxy_url),
        ..Default::default()
    });
    let result = fetch_page(
        &client,
        "http://example.com/article",
        opts,
        real_resolver(),
        &cancel,
    )
    .await;
    Some((result, handle))
}

/// [T-F081] Normal extraction strips language classes before conversion,
/// unlike hand-authored converter fixtures.
#[tokio::test]
async fn default_path_loses_pre_class_language_and_nav_without_raw_fallback() {
    let Some((result, handle)) = fetch_article_via_proxy(
        &article_page(
            "Understanding Rust Ownership",
            "<pre><code class=\"language-rust\">fn main() {}</code></pre>",
        ),
        |opts| opts,
    )
    .await
    else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    let page = result.expect("a rich article page must fetch successfully");
    assert!(
        !page.used_raw_fallback(),
        "a rich article with plenty of paragraph text must not fall back to raw HTML: {:?}",
        page.markdown()
    );
    assert!(
        !page.markdown().contains("```rust"),
        "the default path strips the class attribute before conversion, so no \
        `rust` fence info string should survive: {:?}",
        page.markdown()
    );
    assert!(
        !page.markdown().contains("Site navigation"),
        "Readability must drop the <nav> chrome on the default path: {:?}",
        page.markdown()
    );

    join_server_thread(handle);
}

/// [T-F082] Raw extraction preserves language classes and fence info strings.
#[tokio::test]
async fn raw_path_keeps_pre_class_language_in_the_fence() {
    let Some((result, handle)) = fetch_article_via_proxy(
        &article_page(
            "Understanding Rust Ownership",
            "<pre><code class=\"language-rust\">fn main() {}</code></pre>",
        ),
        |opts| FetchOptions { raw: true, ..opts },
    )
    .await
    else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    let page = result.expect("a rich article page must fetch successfully in raw mode");
    assert!(
        page.markdown().contains("```rust"),
        "the raw path carries the class attribute through unchanged, so a \
        `rust` fence info string must survive: {:?}",
        page.markdown()
    );

    join_server_thread(handle);
}

/// [T-F083] Readability cleanup must preserve table structure for conversion.
#[tokio::test]
async fn default_path_keeps_two_by_two_theaded_table_with_separator_row() {
    let html = article_page(
        "City Population Overview",
        "<table><thead><tr><th>City</th><th>Population</th></tr></thead>\
        <tbody><tr><td>Springfield</td><td>150000</td></tr></tbody></table>",
    );
    let Some((result, handle)) = fetch_article_via_proxy(&html, |opts| opts).await else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    let page = result.expect("a rich article page with a table must fetch successfully");
    assert!(
        !page.used_raw_fallback(),
        "a rich article with plenty of paragraph text must not fall back to raw HTML: {:?}",
        page.markdown()
    );
    let markdown = page.markdown();
    let lines: Vec<&str> = markdown.lines().collect();
    let header_idx = lines
        .iter()
        .position(|line| {
            line.starts_with('|') && line.contains("City") && line.contains("Population")
        })
        .unwrap_or_else(|| panic!("header row must survive the default path: {markdown:?}"));
    let separator_line = lines
        .get(header_idx + 1)
        .unwrap_or_else(|| panic!("a line must immediately follow the header row: {markdown:?}"));
    assert!(
        !separator_line.is_empty()
            && separator_line.contains('-')
            && separator_line
                .chars()
                .all(|c| c == '|' || c == '-' || c == ' '),
        "the line right after the header row must be a dash separator row: {markdown:?}"
    );
    assert!(
        markdown.contains("Springfield") && markdown.contains("150000"),
        "the data row must survive the default path: {markdown:?}"
    );

    join_server_thread(handle);
}

/// [T-F074]
#[tokio::test]
async fn with_a_proxy_configured_fetch_page_to_a_literal_loopback_url_is_blocked_before_any_request_reaches_the_proxy()
 {
    // A dead proxy distinguishes literal-host rejection from transport failure:
    // without the SSRF check this would return Http, not InternalHost.
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

/// [T-F084] Thin-extract detection must hold through real Readability scoring.
/// Without js-rendering this asserts the warning without launching Chrome;
/// with the feature, concurrent profile directories would interfere with the
/// CDP cleanup test. This does not verify browser fallback dynamically.
#[cfg(not(feature = "js-rendering"))]
#[tokio::test]
#[tracing_test::traced_test]
async fn a_page_whose_extracted_body_is_below_the_threshold_warns_that_extraction_was_thin() {
    // Below EXTRACT_TEXT_THRESHOLD once extracted. No `<script>` tag or SPA
    // root id, so `is_js_dependent` stays false regardless of body length and
    // the fetch reaches `is_thin_extract` instead of the JS-dependent branch.
    let thin = "<html><head><title>T</title></head><body><article><p>x</p></article></body></html>";
    let Some((result, handle)) = fetch_article_via_proxy(thin, |opts| opts).await else {
        return; // loopback bind unavailable
    };

    assert!(
        result.is_ok(),
        "a thin page must still return a body, not an error: {result:?}"
    );
    assert!(
        logs_contain("extraction yielded too little content"),
        "fetch_page must warn when is_thin_extract gates the CDP fallback"
    );
    join_server_thread(handle);
}
