use super::ssrf::EgressMode;
use super::*;
use crate::test_support::{
    join_server_thread, no_redirect_client, spawn_forward_proxy, try_spawn_mock_server,
};
use reqwest::Proxy;
use reqwest::redirect::Policy;
use std::io;
use std::thread::JoinHandle;
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

/// Wraps `payload` in the article shell Readability is pinned to extract
/// cleanly: nav and footer chrome around four filler paragraphs, shaped after
/// `extractor::tests::BLOG_HTML`. The filler is what keeps the page above the
/// thin-extract and thin-body thresholds, so `payload` alone decides what each
/// test observes and cannot be what drops the fetch into raw fallback.
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

/// Spawns a forward proxy serving `html` and runs `fetch_page` against it.
/// `configure_opts` layers each caller's own option (e.g. `raw: true`) onto
/// the shared proxied base.
///
/// `None` carries the unavailable-loopback skip, so callers early-return the
/// way every other proxy-backed test here does.
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

/// [T-F081] 既定経路では pre の class 由来の言語指定が失われ nav が消え raw fallback にも落ちない
///
/// The default path runs Readability before conversion, and its
/// `keep_classes: false` strips every `class` attribute, so the
/// `class="language-rust"` fence loses its `rust` info string. Converting
/// hand-authored HTML directly keeps it, which is why the converter's own
/// tests see an info string the production path never produces.
///
/// The fixture's paragraphs stay well above the thin-extract and thin-body
/// thresholds so the fetch neither falls back to raw HTML nor takes the
/// JS-rendering detour.
///
/// Routed through `spawn_forward_proxy` + `EgressMode::Proxied` rather than a
/// direct fetch of the mock server's loopback URI: `ssrf_check` blocks a
/// literal loopback host before any request is sent, in every mode.
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

/// [T-F082] raw 経路では pre の class 由来の言語指定がフェンスに残る
///
/// With `raw: true`, `extract_raw` skips Readability entirely and carries the
/// source HTML's `class` attribute through unchanged, so `language-rust` still
/// attaches `rust` as the fence's info string. This is the only path on which
/// a fetched page keeps a fence language.
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

/// [T-F083] thead と th を持つ 2 行 2 列の表が既定経路で区切り行つきに残る
///
/// Table structure survives where a `class` attribute does not: Readability's
/// cleanup drops attributes, not elements, so a `<thead>` header table reaches
/// conversion intact and comes out with its dash separator row.
///
/// Routed through `spawn_forward_proxy` for the same reason as the tests
/// above.
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

/// [T-F084] 本文が薄いページを fetch_page へ通すと抽出量が閾値を下回った警告が出る
///
/// `is_thin_extract` gates the CDP fallback at `fetch.rs`'s two call sites, and
/// its own unit tests (T-F033..T-F040) build `ExtractedArticle` literals, so no
/// test reaches those sites through `fetch_page`. dom_smoothie's scoring is the
/// input, and a change there moves the chromium launch condition silently.
///
/// The asserted text is the half both branches share: the message tail differs
/// by whether `js-rendering` is on (`fetch.rs` warns "trying JS rendering
/// fallback" with the feature, "but JS rendering unavailable" without). The
/// chromium launch itself is not asserted.
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
