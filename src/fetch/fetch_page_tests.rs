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

/// Shared fixture for T-F081 / T-F082: an article-shaped page (nav + footer
/// chrome around several paragraphs) carrying one `<pre><code
/// class="language-rust">` block, mirroring `extractor::tests::BLOG_HTML`
/// (a fixture Readability is already pinned to extract cleanly, T-FX016) so
/// the added `<pre>` block does not itself drop the page into raw fallback.
fn class_language_article_html() -> String {
    "<html><head><title>Understanding Ownership</title></head><body>\
        <nav>Site navigation: Home About Blog Contact archives categories tags</nav>\
        <article>\
        <h1>Understanding Rust Ownership</h1>\
        <p>Rust's ownership system is one of its most unique features. It enables \
        memory safety without garbage collection. The ownership rules are checked \
        at compile time by the borrow checker, and every value in the language \
        obeys these rules from the moment it is created until it goes out of \
        scope.</p>\
        <p>Each value in Rust has a variable that is called its owner, and there \
        can only be one owner at a time. When the owner goes out of scope, the \
        value is dropped automatically, which is why Rust programs almost never \
        leak memory even without a garbage collector running in the \
        background.</p>\
        <p>The snippet below shows a minimal Rust program that does nothing but \
        declare a main function, and readers can compile it locally to confirm \
        the ownership rules described above hold for the simplest possible \
        case.</p>\
        <pre><code class=\"language-rust\">fn main() {}</code></pre>\
        <p>Beyond this trivial example, ownership becomes more interesting once \
        references, borrowing, and lifetimes enter the picture, and the following \
        sections build on this foundation one concept at a time so the rules stay \
        easy to follow.</p>\
        </article>\
        <footer>Site footer: copyright notice and additional links</footer>\
        </body></html>"
        .to_owned()
}

/// [T-F081] 既定経路では pre の class 由来の言語指定が失われ nav が消え raw fallback にも落ちない
///
/// Contrasts with T-F082: the default (non-raw) path runs
/// `extract_article -> Readability::parse` before conversion, and
/// `Config::default()`'s `keep_classes: false` strips every `class`
/// attribute (converter.rs T-FC017 doc comment), so the `<pre><code
/// class="language-rust">` fence must lose its `rust` info string here even
/// though T-FC017 pins that same class producing one when conversion runs
/// directly on hand-authored HTML. Readability drops `<nav>` chrome
/// (T-FX016), and the surviving paragraphs must stay well above both the
/// thin-extract and thin-body thresholds so the fetch neither falls back to
/// raw HTML nor takes the JS-rendering detour.
///
/// Routed through `spawn_forward_proxy` + `EgressMode::Proxied` rather than a
/// direct fetch of `try_spawn_mock_server`'s loopback URI (T-F074: a literal
/// loopback host is blocked by `ssrf_check` before any request is sent, in
/// every mode), mirroring T-F073's pattern for exercising a real page body
/// through `fetch_page`.
#[tokio::test]
async fn default_path_loses_pre_class_language_and_nav_without_raw_fallback() {
    let Some((proxy_url, handle)) = spawn_forward_proxy(&class_language_article_html()) else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    let client = Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();
    let (cancel, _) = watch::channel(false);
    let opts = FetchOptions {
        egress: EgressMode::Proxied(proxy_url.clone()),
        ..Default::default()
    };
    let result = fetch_page(
        &client,
        "http://example.com/article",
        opts,
        real_resolver(),
        &cancel,
    )
    .await;

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
/// Same page as T-F081 with `raw: true`: `extract_raw` skips Readability
/// entirely and carries the source HTML's `class` attribute through
/// unchanged (converter.rs T-FC017 doc comment), so the `language-rust`
/// class must still attach `rust` as the fence's info string once `fetch_page`
/// converts it.
#[tokio::test]
async fn raw_path_keeps_pre_class_language_in_the_fence() {
    let Some((proxy_url, handle)) = spawn_forward_proxy(&class_language_article_html()) else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    let client = Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();
    let (cancel, _) = watch::channel(false);
    let opts = FetchOptions {
        raw: true,
        egress: EgressMode::Proxied(proxy_url.clone()),
        ..Default::default()
    };
    let result = fetch_page(
        &client,
        "http://example.com/article",
        opts,
        real_resolver(),
        &cancel,
    )
    .await;

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
/// Contrasts with T-F081/T-F082: unlike a `class` attribute, table structure
/// (`<thead>`/`<th>`) is not stripped by Readability's class cleanup, so a
/// small `<thead>`-header table embedded in an otherwise ordinary article
/// must survive the default (non-raw) path with its header row followed
/// immediately by a dash separator row, mirroring converter.rs's own pin
/// (T-FC023) but exercised end to end through `fetch_page`.
///
/// Routed through `spawn_forward_proxy` for the same reason as T-F081.
#[tokio::test]
async fn default_path_keeps_two_by_two_theaded_table_with_separator_row() {
    let html = "<html><head><title>Population Data</title></head><body>\
        <nav>Site navigation: Home About Blog Contact archives categories tags</nav>\
        <article>\
        <h1>City Population Overview</h1>\
        <p>This article summarizes recent population figures for two \
        representative cities, drawn from public census records, so readers \
        can compare growth trends across regions without needing to consult \
        the underlying government datasets directly.</p>\
        <p>The table below lists each city alongside its most recently \
        reported population count, giving a quick reference before the \
        discussion moves on to the historical trends behind these numbers.</p>\
        <table><thead><tr><th>City</th><th>Population</th></tr></thead>\
        <tbody><tr><td>Springfield</td><td>150000</td></tr></tbody></table>\
        <p>The figures above illustrate that even modest cities can carry \
        meaningfully different population totals, and later sections of this \
        guide will expand the same comparison to a wider set of regions once \
        more data becomes available.</p>\
        </article>\
        <footer>Site footer: copyright notice and additional links</footer>\
        </body></html>";
    let Some((proxy_url, handle)) = spawn_forward_proxy(html) else {
        return; // loopback bind unavailable — cannot exercise the proxy path
    };

    let client = Client::builder()
        .redirect(Policy::none())
        .proxy(Proxy::all(&proxy_url).expect("proxy url"))
        .build()
        .unwrap();
    let (cancel, _) = watch::channel(false);
    let opts = FetchOptions {
        egress: EgressMode::Proxied(proxy_url.clone()),
        ..Default::default()
    };
    let result = fetch_page(
        &client,
        "http://example.com/article",
        opts,
        real_resolver(),
        &cancel,
    )
    .await;

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
