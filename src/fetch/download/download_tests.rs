use std::io::Write;

use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
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

/// reqwest's `deflate` Content-Encoding expects zlib-wrapped data (not raw
/// DEFLATE), which is what `ZlibEncoder` produces.
fn zlib_deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn validated(url: &str) -> ValidatedUrl {
    ValidatedUrl::for_test(url)
}

fn public_resolver() -> ssrf::StaticDnsResolver {
    ssrf::StaticDnsResolver::single("8.8.8.8")
}

/// `download` against a wiremock path with every argument at its default: no
/// redirects, the standard hop cap, a resolver that answers public, direct
/// egress. A test that varies one of those calls `download` directly, so the
/// variation stays visible at the call site.
async fn download_default(
    uri: &str,
    path: &str,
) -> Result<(ValidatedUrl, String, bool), FetchError> {
    let client = no_redirect_client();
    download(
        &client,
        &validated(&format!("{uri}{path}")),
        MAX_REDIRECTS,
        &public_resolver(),
        &ssrf::EgressMode::Direct,
    )
    .await
}

/// [T-F009]
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

    let (final_url, html, _uncertain) = download_default(&server.uri(), "/page").await.unwrap();

    assert!(final_url.as_str().contains("/page"));
    assert!(html.contains("hello"));
}

/// [T-F067] download flags `decode_uncertain` for a body that cannot be decoded
/// cleanly under its label and that detection refuses (issue #241). Windows-1252
/// smart quotes mislabeled as utf-8 are the realistic single-byte case: the body
/// still comes back (best-effort lossy, exit 0), but the uncertain flag is set.
#[tokio::test]
async fn download_flags_decode_uncertain_for_undecodable_body() {
    let Some(server) = try_spawn_mock_server("fetch::download").await else {
        return;
    };
    let mut body = b"<html><body><p>It\x92s a fine day, isn\x92t it? ".to_vec();
    body.extend_from_slice(
        b"\x93Quoted\x94 text and an \x97 em dash, with plenty more prose.</p></body></html>",
    );
    Mock::given(method("GET"))
        .and(path("/mojibake"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;

    let (_final_url, html, uncertain) = download_default(&server.uri(), "/mojibake").await.unwrap();

    assert!(uncertain, "undecodable body must set decode_uncertain");
    assert!(
        !html.is_empty(),
        "a best-effort body must still be returned"
    );
}

/// [T-F069] download recovers a mislabeled multi-byte body via detection and does
/// NOT flag `decode_uncertain` (issue #241). Shift_JIS content mislabeled as utf-8
/// is the recoverable case the reliability gate trusts.
#[tokio::test]
async fn download_recovers_mislabeled_multibyte_without_uncertain() {
    let Some(server) = try_spawn_mock_server("fetch::download").await else {
        return;
    };
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(
        "<html><body><p>これはシフトジスでエンコードされた日本語の本文です。誤ったラベルが付いていても検知で復元されます。十分な長さの文章を用意します。</p></body></html>",
    );
    Mock::given(method("GET"))
        .and(path("/sjis"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_bytes(sjis.to_vec()),
        )
        .mount(&server)
        .await;

    let (_final_url, html, uncertain) = download_default(&server.uri(), "/sjis").await.unwrap();

    assert!(
        !uncertain,
        "recovered multi-byte body must not be flagged uncertain"
    );
    assert!(
        html.contains("日本語の本文"),
        "recovered text should be present, got: {html}"
    );
}

/// [T-F010]
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

    assert!(matches!(
        download_default(&server.uri(), "/404").await,
        Err(FetchError::Status(404))
    ));
    assert!(matches!(
        download_default(&server.uri(), "/500").await,
        Err(FetchError::Status(500))
    ));
}

/// [T-F011]
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

    let result = download_default(&server.uri(), "/huge").await;
    assert!(matches!(result, Err(FetchError::TooLarge)));
}

/// [T-F012]
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

    let result = download_default(&server.uri(), "/binary").await;
    assert!(
        matches!(result, Err(FetchError::UnsupportedContentType(ref ct)) if ct == "application/pdf"),
        "got: {result:?}"
    );
}

/// [T-F013]
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

    let result = download_default(&server.uri(), "/redir").await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "redirect to 127.0.0.1 should be blocked, got: {result:?}"
    );
}

/// [T-F014] a redirect to a public-looking host is blocked when DNS resolves it
/// to a private IP — the case T-F013 (literal IP in Location) cannot reach, which
/// is why this test injects a resolver and calls `download` directly.
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
        &ssrf::EgressMode::Direct,
    )
    .await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "redirect to domain resolving to private IP should be blocked, got: {result:?}"
    );
}

/// [T-F015]
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
        &ssrf::EgressMode::Direct,
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
        &ssrf::EgressMode::Direct,
    )
    .await;
    assert!(matches!(result, Err(FetchError::TooManyRedirects(0))));
    assert!(logs_contain("redirect cap exceeded"));
    assert!(logs_contain("redirect_chain_length"));
    assert!(logs_contain("max_redirects"));
    assert!(logs_contain("final_url"));
}

/// [T-F016]
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

    let result = download_default(&server.uri(), "/bad-redir").await;
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

    let (_final_url, body, _uncertain) = download_default(&server.uri(), "/gz").await.unwrap();

    assert!(
        body.contains("hello"),
        "gzip body should be transparently decompressed, got: {body:?}"
    );
}

/// [T-F059] (issue #202) Pins a second enabled codec beyond gzip.
/// `Content-Encoding: deflate` carries zlib-wrapped data; reqwest's `deflate`
/// feature must transparently decompress it rather than hand the raw bytes to
/// the charset decoder.
#[tokio::test]
async fn download_transparently_decodes_deflate_response() {
    let Some(server) = try_spawn_mock_server("fetch::download").await else {
        return;
    };
    let html = "<html><body><p>hello</p></body></html>";
    Mock::given(method("GET"))
        .and(path("/df"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("content-encoding", "deflate")
                .set_body_bytes(zlib_deflate(html.as_bytes())),
        )
        .mount(&server)
        .await;

    let (_final_url, body, _uncertain) = download_default(&server.uri(), "/df").await.unwrap();

    assert!(
        body.contains("hello"),
        "deflate body should be transparently decompressed, got: {body:?}"
    );
}

/// [T-008]
#[tokio::test]
async fn proxied_mode_blocks_a_redirect_hop_whose_location_is_a_literal_private_ip_url() {
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
        &ssrf::EgressMode::Proxied("http://proxy.example:8080".to_owned()),
    )
    .await;
    assert!(
        matches!(result, Err(FetchError::InternalHost)),
        "proxied mode must block a redirect hop to a literal private IP, got: {result:?}"
    );
}
