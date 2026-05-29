use super::*;

mod charset_tests {
    use super::*;

    /// [T-F001] extracts_charset_from_content_type
    #[test]
    fn extracts_charset_from_content_type() {
        assert_eq!(
            extract_charset("text/html; charset=utf-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            extract_charset("text/html; charset=Shift_JIS").as_deref(),
            Some("shift_jis")
        );
        assert_eq!(
            extract_charset("text/html; charset=\"EUC-KR\"").as_deref(),
            Some("euc-kr")
        );
    }

    /// [T-F002] returns_none_when_no_charset
    #[test]
    fn returns_none_when_no_charset() {
        assert!(extract_charset("text/html").is_none());
        assert!(extract_charset("text/plain; boundary=something").is_none());
    }

    /// [T-F003] decode_body_handles_utf8
    #[test]
    fn decode_body_handles_utf8() {
        let bytes = "こんにちは".as_bytes();
        assert_eq!(decode_body(bytes, Some("utf-8")), "こんにちは");
        assert_eq!(decode_body(bytes, None), "こんにちは");
    }

    /// [T-F004] decode_body_handles_shift_jis
    #[test]
    fn decode_body_handles_shift_jis() {
        let encoding = encoding_rs::SHIFT_JIS;
        let (bytes, _, _) = encoding.encode("テスト");
        assert_eq!(decode_body(&bytes, Some("shift_jis")), "テスト");
    }

    /// [T-F005] decode_body_handles_euc_jp
    #[test]
    fn decode_body_handles_euc_jp() {
        let encoding = encoding_rs::EUC_JP;
        let (bytes, _, _) = encoding.encode("日本語");
        assert_eq!(decode_body(&bytes, Some("euc-jp")), "日本語");
    }

    /// [T-F006] decode_body_falls_back_to_utf8_for_unknown
    #[test]
    fn decode_body_falls_back_to_utf8_for_unknown() {
        let bytes = "hello".as_bytes();
        assert_eq!(decode_body(bytes, Some("unknown-encoding")), "hello");
    }
}

mod content_type_tests {
    use super::*;

    /// [T-F007] accepts_textual_content_types
    #[test]
    fn accepts_textual_content_types() {
        for ct in [
            "text/html; charset=utf-8",
            "text/plain",
            "application/xhtml+xml",
            "application/xml",
            "; charset=utf-8", // edge: empty mime before semicolon → permissive
        ] {
            assert!(check_content_type(ct).is_ok(), "should accept: {ct}");
        }
    }

    /// [T-F008] rejects_non_textual_content_types
    #[test]
    fn rejects_non_textual_content_types() {
        for ct in ["application/pdf", "image/png", "application/json"] {
            assert!(
                matches!(
                    check_content_type(ct),
                    Err(FetchError::UnsupportedContentType(ref m)) if m == ct
                ),
                "should reject: {ct}"
            );
        }
    }
}

mod download_tests {
    use super::*;
    use crate::test_support::{no_redirect_client, try_spawn_mock_server};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn validated(url: &str) -> ValidatedUrl {
        ValidatedUrl::for_test(url)
    }

    fn public_resolver() -> ssrf::StaticDnsResolver {
        ssrf::StaticDnsResolver::single("8.8.8.8")
    }

    /// [T-F009] download_success_returns_html
    #[tokio::test]
    async fn download_success_returns_html() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body><p>hello</p></body></html>"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let (final_url, html) = download(
            &client,
            &validated(&format!("{}/page", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await
        .unwrap();

        assert!(final_url.as_str().contains("/page"));
        assert!(html.contains("hello"));
    }

    /// [T-F010] download_non_success_returns_status_error
    #[tokio::test]
    async fn download_non_success_returns_status_error() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/404"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/500"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = no_redirect_client();
        assert!(matches!(
            download(
                &client,
                &validated(&format!("{}/404", server.uri())),
                MAX_REDIRECTS,
                &public_resolver()
            )
            .await,
            Err(FetchError::Status(404))
        ));
        assert!(matches!(
            download(
                &client,
                &validated(&format!("{}/500", server.uri())),
                MAX_REDIRECTS,
                &public_resolver()
            )
            .await,
            Err(FetchError::Status(500))
        ));
    }

    /// [T-F011] download_too_large_body_rejected
    #[tokio::test]
    async fn download_too_large_body_rejected() {
        let oversized = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/huge"))
            .respond_with(ResponseTemplate::new(200).set_body_string(oversized))
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/huge", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(matches!(result, Err(FetchError::TooLarge)));
    }

    /// [T-F012] download_rejects_non_html_content_type
    #[tokio::test]
    async fn download_rejects_non_html_content_type() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/binary"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(b"fake pdf".to_vec()),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/binary", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::UnsupportedContentType(ref ct)) if ct == "application/pdf"),
            "got: {result:?}"
        );
    }

    /// [T-F013] redirect_to_private_ip_blocked
    #[tokio::test]
    async fn redirect_to_private_ip_blocked() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://127.0.0.1/secret"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "redirect to 127.0.0.1 should be blocked, got: {result:?}"
        );
    }

    /// [T-F014] redirect_to_dns_private_ip_blocked
    #[tokio::test]
    async fn redirect_to_dns_private_ip_blocked() {
        let private_resolver = ssrf::StaticDnsResolver::single("10.0.0.1");

        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://evil.com/internal"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            MAX_REDIRECTS,
            &private_resolver,
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::InternalHost)),
            "redirect to domain resolving to private IP should be blocked, got: {result:?}"
        );
    }

    /// [T-F015] too_many_redirects_returns_error
    #[tokio::test]
    async fn too_many_redirects_returns_error() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://example.com/next"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            0, // max_redirects = 0
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::TooManyRedirects(0))),
            "should error on too many redirects, got: {result:?}"
        );
    }

    /// [T-F056] redirect_cap_exceeded_emits_calibration_warn — `redirect cap
    /// exceeded` warn must carry structured fields (`redirect_chain_length`,
    /// `max_redirects`, `final_url`) so caller logs can sample retry-success
    /// rate for the DataError vs TempFailure flip decision (issue #145).
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn redirect_cap_exceeded_emits_calibration_warn() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://example.com/next"),
            )
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/redir", server.uri())),
            0, // max_redirects = 0
            &public_resolver(),
        )
        .await;
        assert!(matches!(result, Err(FetchError::TooManyRedirects(0))));
        assert!(logs_contain("redirect cap exceeded"));
        assert!(logs_contain("redirect_chain_length"));
        assert!(logs_contain("max_redirects"));
        assert!(logs_contain("final_url"));
    }

    /// [T-F016] redirect_missing_location_header_returns_error
    #[tokio::test]
    async fn redirect_missing_location_header_returns_error() {
        let Some(server) = try_spawn_mock_server("fetch::download").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/bad-redir"))
            .respond_with(ResponseTemplate::new(302))
            .mount(&server)
            .await;

        let client = no_redirect_client();
        let result = download(
            &client,
            &validated(&format!("{}/bad-redir", server.uri())),
            MAX_REDIRECTS,
            &public_resolver(),
        )
        .await;
        assert!(
            matches!(result, Err(FetchError::RedirectMissingLocation)),
            "missing Location header should error, got: {result:?}"
        );
    }
}

mod fetch_page_tests {
    use super::*;
    use crate::test_support::{no_redirect_client, try_spawn_mock_server};
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
}

mod js_dependent_tests {
    use super::*;

    /// [T-F020] all_spa_frameworks_detected
    #[test]
    fn all_spa_frameworks_detected() {
        for id in SPA_ROOT_IDS {
            let html = format!(
                r#"<html><head><script src="app.js"></script></head>
                <body><div {id}></div></body></html>"#
            );
            assert!(is_js_dependent(&html), "should detect SPA with {id}");
        }
    }

    /// [T-F021] normal_html_not_detected
    #[test]
    fn normal_html_not_detected() {
        let html = r#"<html><body><article>
        <h1>Title</h1><p>Long paragraph with enough content to exceed
        the threshold of one hundred characters easily.</p>
        </article></body></html>"#;
        assert!(!is_js_dependent(html));
    }

    /// [T-F022] script_without_spa_pattern_but_empty_body
    #[test]
    fn script_without_spa_pattern_but_empty_body() {
        let html = r#"<html><head><script src="bundle.js"></script></head>
        <body><div class="app"></div></body></html>"#;
        assert!(is_js_dependent(html));
    }

    /// [T-F023] spa_pattern_without_script_but_empty_body
    #[test]
    fn spa_pattern_without_script_but_empty_body() {
        let html = r#"<html><body><div id="root"></div></body></html>"#;
        assert!(is_js_dependent(html));
    }

    /// [T-F024] rich_body_with_scripts_not_detected
    #[test]
    fn rich_body_with_scripts_not_detected() {
        let content = "x".repeat(200);
        let html = format!(
            r#"<html><head><script src="app.js"></script></head>
            <body><div id="root"><p>{content}</p></div></body></html>"#
        );
        assert!(!is_js_dependent(&html));
    }

    /// [T-F025] thin_body_without_script_or_spa_pattern_not_detected
    #[test]
    fn thin_body_without_script_or_spa_pattern_not_detected() {
        let html = "<html><body><p>short</p></body></html>";
        assert!(!is_js_dependent(html));
    }

    /// [T-F026] no_body_tag_falls_back_to_full_html
    #[test]
    fn no_body_tag_falls_back_to_full_html() {
        let html = r#"<div id="root"></div><script src="app.js"></script>"#;
        assert!(is_js_dependent(html));
    }
}

mod thin_body_tests {
    use super::*;

    /// [T-F027] style_content_excluded_from_visible_text
    #[test]
    fn style_content_excluded_from_visible_text() {
        let html = "<html><body><style>.big{font-size:9999px;color:red;margin:0 auto;padding:10px 20px 30px 40px}</style><p>hi</p></body></html>";
        assert!(has_thin_body(html));
    }

    /// [T-F028] uppercase_script_tag_excluded
    #[test]
    fn uppercase_script_tag_excluded() {
        let html = "<html><body><SCRIPT>var x = 'lots of javascript code that should be ignored by the parser';</SCRIPT><p>hi</p></body></html>";
        assert!(has_thin_body(html));
    }

    /// [T-F029] uppercase_body_tag_found
    #[test]
    fn uppercase_body_tag_found() {
        let content = "x".repeat(200);
        let html = format!("<html><BODY><p>{content}</p></BODY></html>");
        assert!(!has_thin_body(&html));
    }

    /// [T-F030] exactly_at_threshold_is_not_thin (body)
    #[test]
    fn exactly_at_threshold_is_not_thin() {
        let content = "x".repeat(BODY_TEXT_THRESHOLD);
        let html = format!("<html><body><p>{content}</p></body></html>");
        assert!(!has_thin_body(&html));
    }

    /// [T-F031] just_below_threshold_is_thin (body)
    #[test]
    fn just_below_threshold_is_thin() {
        let content = "x".repeat(BODY_TEXT_THRESHOLD - 1);
        let html = format!("<html><body><p>{content}</p></body></html>");
        assert!(has_thin_body(&html));
    }

    /// [T-F032] whitespace_only_body_is_thin
    #[test]
    fn whitespace_only_body_is_thin() {
        let html = "<html><body>   \n\t  \n   </body></html>";
        assert!(has_thin_body(html));
    }
}

mod thin_extract_tests {
    use super::*;
    use extractor::ExtractedArticle;

    fn article(content_html: &str, used_raw_fallback: bool) -> ExtractedArticle {
        ExtractedArticle {
            title: None,
            byline: None,
            published_time: None,
            content_html: content_html.to_owned(),
            used_raw_fallback,
        }
    }

    /// [T-F033] raw_fallback_with_short_content_is_thin
    #[test]
    fn raw_fallback_with_short_content_is_thin() {
        assert!(is_thin_extract(&article("<p>short</p>", true)));
    }

    /// [T-F034] raw_fallback_with_rich_content_still_thin
    #[test]
    fn raw_fallback_with_rich_content_still_thin() {
        let content = format!("<p>{}</p>", "x".repeat(100));
        assert!(is_thin_extract(&article(&content, true)));
    }

    /// [T-F035] short_content_is_thin
    #[test]
    fn short_content_is_thin() {
        assert!(is_thin_extract(&article("<p>hi</p>", false)));
    }

    /// [T-F036] sufficient_content_is_not_thin
    #[test]
    fn sufficient_content_is_not_thin() {
        let content = format!("<p>{}</p>", "x".repeat(100));
        assert!(!is_thin_extract(&article(&content, false)));
    }

    /// [T-F037] exactly_at_threshold_is_not_thin (extract)
    #[test]
    fn exactly_at_threshold_is_not_thin() {
        let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD));
        assert!(!is_thin_extract(&article(&content, false)));
    }

    /// [T-F038] just_below_threshold_is_thin (extract)
    #[test]
    fn just_below_threshold_is_thin() {
        let content = format!("<p>{}</p>", "x".repeat(EXTRACT_TEXT_THRESHOLD - 1));
        assert!(is_thin_extract(&article(&content, false)));
    }

    /// [T-F039] html_tags_excluded_from_count
    #[test]
    fn html_tags_excluded_from_count() {
        let content = r#"<div class="very-long-class-name"><span>ab</span></div>"#;
        assert!(is_thin_extract(&article(content, false)));
    }

    /// [T-F040] whitespace_excluded_from_count
    #[test]
    fn whitespace_excluded_from_count() {
        let content = format!("<p>{}</p>", " x ".repeat(30));
        assert!(is_thin_extract(&article(&content, false)));
    }
}

mod browser_binary_tests {
    use super::*;
    use std::env;

    /// [T-F041] t001_returns_error_when_chrome_not_found
    #[test]
    fn t001_returns_error_when_chrome_not_found() {
        let result = resolve_browser_binary_from(&[], &[]);
        assert!(
            matches!(result, Err(BrowserError::NotFound)),
            "expected NotFound, got: {result:?}"
        );
    }

    /// [T-F042] finds_binary_at_known_path
    #[test]
    fn finds_binary_at_known_path() {
        let existing = env::current_exe().unwrap();
        let result = resolve_browser_binary_from(&[], &[existing.as_path()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), existing);
    }
}

#[cfg(feature = "js-rendering")]
mod cdp_launch_tests {
    use super::*;

    /// [T-F043] t009_launch_args_contain_security_flags
    #[test]
    fn t009_launch_args_contain_security_flags() {
        let args = build_launch_args();
        for flag in [
            "--disable-webrtc",
            "--disable-background-networking",
            "--disable-features=DnsOverHttps",
            "--disable-domain-reliability",
            "--no-pings",
        ] {
            assert!(args.contains(&flag), "missing security flag: {flag}");
        }
    }
}

mod browser_request_tests {
    use super::ssrf::StaticDnsResolver;
    use super::*;

    fn private_dns() -> StaticDnsResolver {
        StaticDnsResolver::single("10.0.0.1")
    }

    fn public_dns() -> StaticDnsResolver {
        StaticDnsResolver::single("93.184.216.34")
    }

    /// [T-F044] t004_blocks_dns_resolving_to_private_ip
    #[tokio::test]
    async fn t004_blocks_dns_resolving_to_private_ip() {
        let resolver = private_dns();
        assert!(
            !check_browser_request("https://evil.example/secret", &resolver).await,
            "must block when DNS resolves to private IP"
        );
    }

    /// [T-F045] t004_blocks_internal_ip_literal
    #[tokio::test]
    async fn t004_blocks_internal_ip_literal() {
        let resolver = public_dns();
        assert!(
            !check_browser_request("http://127.0.0.1/secret", &resolver).await,
            "must block loopback IP"
        );
    }

    /// [T-F046] t004_allows_public_url
    #[tokio::test]
    async fn t004_allows_public_url() {
        let resolver = public_dns();
        assert!(
            check_browser_request("https://example.com/page", &resolver).await,
            "must allow public URL"
        );
    }

    /// [T-F047] t004_allows_non_network_urls
    #[tokio::test]
    async fn t004_allows_non_network_urls() {
        let resolver = public_dns();
        for url in [
            "data:text/html,<p>test</p>",
            "about:blank",
            "chrome://settings",
            "blob:https://example.com/uuid",
        ] {
            assert!(
                check_browser_request(url, &resolver).await,
                "must allow non-network URL: {url}"
            );
        }
    }

    /// [T-F048] t004_blocks_unknown_schemes
    #[tokio::test]
    async fn t004_blocks_unknown_schemes() {
        let resolver = public_dns();
        for url in ["file:///etc/passwd", "ftp://internal/data", "gopher://x"] {
            assert!(
                !check_browser_request(url, &resolver).await,
                "must block unknown scheme: {url}"
            );
        }
    }

    /// [T-F049] t004_blocks_websocket_to_internal
    #[tokio::test]
    async fn t004_blocks_websocket_to_internal() {
        let resolver = public_dns();
        assert!(
            !check_browser_request("ws://127.0.0.1:8080/ws", &resolver).await,
            "must block ws:// to loopback"
        );
        assert!(
            !check_browser_request("wss://localhost/ws", &resolver).await,
            "must block wss:// to localhost"
        );
    }

    /// [T-F050] t004_blocks_websocket_dns_to_private
    #[tokio::test]
    async fn t004_blocks_websocket_dns_to_private() {
        let resolver = private_dns();
        assert!(
            !check_browser_request("ws://evil.example/ws", &resolver).await,
            "must block ws:// when DNS resolves to private IP"
        );
    }
}

#[cfg(feature = "js-rendering")]
mod ws_url_parse_tests {
    use super::*;

    /// [T-F052] parse_ws_url_extracts_first_matching_line
    #[tokio::test]
    async fn parse_ws_url_extracts_first_matching_line() {
        let stderr = b"[chromium] starting up\n\
                       DevTools listening on ws://127.0.0.1:54321/devtools/browser/abc-123\n\
                       DevTools listening on ws://127.0.0.1:54321/devtools/browser/def-456\n";
        let url = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect("first match should win");
        assert_eq!(url, "ws://127.0.0.1:54321/devtools/browser/abc-123");
    }

    /// [T-F053] parse_ws_url_skips_unrelated_lines_until_match
    #[tokio::test]
    async fn parse_ws_url_skips_unrelated_lines_until_match() {
        let stderr = b"[8765:0x110000000] preference manifest unparseable\n\
                       [warn] hardware acceleration unavailable\n\
                       random listening on something else\n\
                       DevTools listening on ws://localhost:1234/devtools/browser/xyz\n";
        let url = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect("should match after unrelated prefix");
        assert_eq!(url, "ws://localhost:1234/devtools/browser/xyz");
    }

    /// [T-F054] parse_ws_url_eof_before_match_errors
    #[tokio::test]
    async fn parse_ws_url_eof_before_match_errors() {
        let stderr = b"chromium crashed before opening port\n";
        let err = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect_err("EOF without match must surface as error");
        let msg = err.to_string();
        assert!(
            msg.contains("chromium exited before announcing DevTools URL"),
            "expected EOF message, got: {msg}"
        );
    }

    /// [T-F055] parse_ws_url_rejects_non_browser_devtools_url
    ///
    /// chromium also prints `DevTools listening on ws://.../page/<id>` for
    /// per-page debuggers — we must only accept the browser-level URL.
    #[tokio::test]
    async fn parse_ws_url_rejects_non_browser_devtools_url() {
        let stderr = b"DevTools listening on ws://127.0.0.1:9999/devtools/page/something\n";
        let err = parse_ws_url_from_lines(BufReader::new(&stderr[..]))
            .await
            .expect_err("page-level URL must not match");
        assert!(
            err.to_string()
                .contains("chromium exited before announcing DevTools URL")
        );
    }
}

#[cfg(feature = "js-rendering")]
mod cdp_integration_tests {
    use super::*;

    fn chrome_available() -> bool {
        resolve_browser_binary().is_ok()
    }

    /// [T-F051] t005_cdp_renders_public_url
    #[tokio::test]
    async fn t005_cdp_renders_public_url() {
        if !chrome_available() {
            eprintln!("SKIP: Chrome not found");
            return;
        }
        let (cancel, _) = watch::channel(false);
        let html = fetch_with_cdp(
            &ValidatedUrl::for_test("https://example.com"),
            Arc::new(TokioDnsResolver),
            &cancel,
        )
        .await
        .expect("fetch_with_cdp should succeed for public URL");
        assert!(
            html.contains("Example Domain") || html.contains("example"),
            "rendered HTML should contain page content, got {} bytes",
            html.len()
        );
    }
}

mod classify_tests {
    use super::*;

    /// [T-FEC001] BrowserNotFound classifies as UsageError.
    #[test]
    fn browser_not_found_is_usage_error() {
        let c = FetchError::BrowserNotFound("not installed".into()).classify();
        assert_eq!(c.kind, ErrorCode::UsageError);
    }

    /// [T-FEC002] Status(401) and Status(403) classify as UsageError
    /// (priority 1 over the priority-2 4xx fallback).
    #[test]
    fn status_401_403_is_usage_error_not_data_error() {
        for code in [401u16, 403] {
            let c = FetchError::Status(code).classify();
            assert_eq!(
                c.kind,
                ErrorCode::UsageError,
                "code {code} must precede 4xx arm"
            );
        }
    }

    /// [T-FEC003] Status(404) classifies as NotFound
    /// (priority 3 over the priority-2 4xx fallback).
    #[test]
    fn status_404_is_not_found_not_data_error() {
        let c = FetchError::Status(404).classify();
        assert_eq!(c.kind, ErrorCode::NotFound);
        assert!(
            c.next_step.as_deref().is_some_and(|h| h.contains("URL")),
            "expected URL hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-FEC004] Status(408) and Status(429) classify as TempFailure
    /// (priority 4 over the priority-2 4xx fallback).
    #[test]
    fn status_408_429_is_temp_failure_not_data_error() {
        for code in [408u16, 429] {
            let c = FetchError::Status(code).classify();
            assert_eq!(c.kind, ErrorCode::TempFailure, "code {code}");
        }
    }

    /// [T-FEC005] Other 4xx Status codes classify as DataError.
    #[test]
    fn status_other_4xx_is_data_error() {
        for code in [400u16, 410, 422, 499] {
            let c = FetchError::Status(code).classify();
            assert_eq!(c.kind, ErrorCode::DataError, "code {code}");
        }
    }

    /// [T-FEC006] 5xx Status codes classify as TempFailure.
    #[test]
    fn status_5xx_is_temp_failure() {
        for code in [500u16, 502, 503, 599] {
            let c = FetchError::Status(code).classify();
            assert_eq!(c.kind, ErrorCode::TempFailure, "code {code}");
        }
    }

    /// [T-FEC007] Priority-2 DataError variants (non-Status) classify as DataError.
    #[test]
    fn data_error_variants_classify_as_data_error() {
        let cases: Vec<FetchError> = vec![
            FetchError::InvalidScheme,
            FetchError::InternalHost,
            FetchError::UnsupportedContentType("image/png".into()),
            FetchError::RedirectMissingLocation,
            FetchError::TooLarge,
            FetchError::TooManyRedirects(10),
        ];
        for case in &cases {
            assert_eq!(
                case.classify().kind,
                ErrorCode::DataError,
                "{case:?} must classify as DataError"
            );
        }
    }

    /// [T-FEC008] Timeout classifies as Timeout (exit 124 split from TempFailure).
    #[test]
    fn timeout_is_timeout_kind() {
        let c = FetchError::Timeout("timed out".into()).classify();
        assert_eq!(c.kind, ErrorCode::Timeout);
    }

    /// [T-FEC009] DnsResolution classifies as TempFailure with a DNS hint.
    #[test]
    fn dns_resolution_is_temp_failure_with_dns_hint() {
        let c = FetchError::DnsResolution("dns failed".into()).classify();
        assert_eq!(c.kind, ErrorCode::TempFailure);
        assert!(
            c.next_step.as_deref().is_some_and(|h| h.contains("DNS")),
            "expected DNS hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-FEC010] BrowserFailed classifies as IoError (priority 5 sibling).
    #[test]
    fn browser_failed_is_io_error() {
        let c = FetchError::BrowserFailed("CDP error".into()).classify();
        assert_eq!(c.kind, ErrorCode::IoError);
    }
}
