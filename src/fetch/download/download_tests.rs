use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use super::*;
use crate::fetch::{MAX_REDIRECTS, ssrf};
use crate::test_support::{no_redirect_client, try_spawn_mock_server};

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

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
            ResponseTemplate::new(200).set_body_string("<html><body><p>hello</p></body></html>"),
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

/// [T-F058] download_transparently_decodes_gzip_response (issue #202): a server
/// that returns `Content-Encoding: gzip` even without an `Accept-Encoding`
/// request header must be transparently decompressed, not handed to the charset
/// decoder as raw gzip bytes (which yields mojibake). Requires reqwest's `gzip`
/// feature so its default `Accepts` advertises and auto-decodes the encoding.
#[tokio::test]
async fn download_transparently_decodes_gzip_response() {
    let Some(server) = try_spawn_mock_server("fetch::download").await else {
        return;
    };
    let html = "<html><body><p>hello</p></body></html>";
    Mock::given(method("GET"))
        .and(path("/gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(gzip(html.as_bytes())),
        )
        .mount(&server)
        .await;

    let client = no_redirect_client();
    let (_final_url, body) = download(
        &client,
        &validated(&format!("{}/gz", server.uri())),
        MAX_REDIRECTS,
        &public_resolver(),
    )
    .await
    .unwrap();

    assert!(
        body.contains("hello"),
        "gzip body should be transparently decompressed, got: {body:?}"
    );
}
