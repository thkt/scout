use serde::ser::Error as _;

use super::query::to_data_value;
use super::test_helpers::*;
use super::*;
use crate::envelope::ErrorCode;
use crate::search::Lang;
use crate::test_support::try_spawn_mock_server;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-TS024]
#[tokio::test]
async fn search_returns_plain_url_list() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://rust-lang.org", "title": "Rust", "description": "snippet"},
                    {"url": "https://doc.rust-lang.org", "title": "Docs", "description": "more"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("What is Rust?".into()),
        lang: Lang::Auto,
    };

    let result = s.search(params).await.unwrap();
    assert_eq!(
        result.markdown(),
        "https://rust-lang.org\nhttps://doc.rust-lang.org",
        "stdout should be one URL per line, no markdown decoration"
    );
}

/// [T-TS025] search --json output schema (data.query, data.sources, no data.answer)
#[tokio::test]
async fn search_json_schema_omits_answer() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://a.com", "title": "A", "description": "d"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("foo".into()),
        lang: Lang::Auto,
    };

    let result = s.search(params).await.unwrap();
    let data = result.data();
    assert!(data.get("answer").is_none(), "answer field must be absent");
    assert_eq!(data["query"], "foo");
    assert!(data["sources"].is_array());
    assert_eq!(data["sources"][0]["url"], "https://a.com");
    assert_eq!(data["sources"][0]["title"], "A");
    assert_eq!(data["sources"][0]["description"], "d");
}

/// [T-TS026] search command issues exactly one Brave call (no engine::research fanout)
/// Engine path adds fetch + report; search must remain a single Brave round-trip.
#[tokio::test]
async fn search_does_not_traverse_engine_path() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {"results": [{"url": "https://a.com", "title": "A", "description": ""}]}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("foo".into()),
        lang: Lang::Auto,
    };
    s.search(params).await.unwrap();
}

/// [T-TS027] search with zero results returns empty stdout and exit 0
#[tokio::test]
async fn search_zero_results_returns_empty() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {"results": []}
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = SearchParams {
        query: Some("foo".into()),
        lang: Lang::Auto,
    };
    let result = s.search(params).await.unwrap();
    assert_eq!(result.markdown(), "", "empty stdout for zero results");
    assert_eq!(result.data()["sources"].as_array().unwrap().len(), 0);
}

/// [T-TS002] research returns a report with Brave sources and no Search Result header
#[tokio::test]
async fn research_success_returns_report() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    // Brave search response. The URL is unreachable, so fetch will fail and land in
    // failed_urls, but the Sources section still proves the Brave URL flowed through.
    Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {
                    "results": [
                        {"url": "https://rust-lang.test/", "title": "Rust Language", "description": "snippet"}
                    ]
                }
            })))
            .mount(&server)
            .await;

    let s = scout_with_brave(&server.uri());
    let params = ResearchParams {
        query: Some("What is Rust?".into()),
        depth: 1,
        lang: Lang::Auto,
    };

    let result = s.research(params).await.unwrap();
    assert!(
        result.markdown().contains("rust-lang.test"),
        "report should reference Brave source URL, got: {result:?}"
    );
    assert!(
        !result.markdown().contains("## Search Result"),
        "report must not contain a Search Result header"
    );
    assert!(
        !result
            .markdown()
            .contains("vertexaisearch.cloud.google.com"),
        "Sources must not contain Google redirect URLs"
    );
}

/// [T-TS028] --json research data schema (query, sources, fetched_pages, failed_urls)
#[tokio::test]
async fn research_json_schema_includes_required_keys() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {
                "results": [
                    {"url": "https://a.test/", "title": "A", "description": "snippet"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = ResearchParams {
        query: Some("foo".into()),
        depth: 1,
        lang: Lang::Auto,
    };
    let result = s.research(params).await.unwrap();
    let data = result.data();

    assert_eq!(data["query"], "foo", "data.query must echo the request");
    assert!(data["sources"].is_array(), "data.sources must be an array");
    assert_eq!(data["sources"][0]["url"], "https://a.test/");
    assert_eq!(data["sources"][0]["title"], "A");
    assert_eq!(data["sources"][0]["description"], "snippet");
    assert!(
        data["fetched_pages"].is_array(),
        "data.fetched_pages must be an array (possibly empty)"
    );
    assert!(
        data["failed_urls"].is_array(),
        "data.failed_urls must be an array (possibly empty)"
    );
    assert!(
        data.get("answer").is_none(),
        "data.answer must be absent: scout emits no LLM-generated answer"
    );
    assert!(
        data.get("all_sources").is_none(),
        "data.all_sources is the legacy key — must be renamed to sources"
    );
}

/// [T-TS029]
/// Setup: wiremock always returns HTTP 503 (still fails after retry).
/// Action: `Scout::research(...)` is invoked.
/// Expected: returns `Ok(CommandOutput)` (no hard-fail);
/// `degraded_reasons` contains `BraveSearchFailed`; `data.sources` is empty.
/// The cascade does not propagate `BraveError`; failure is absorbed into the
/// degraded report envelope.
#[tokio::test]
async fn research_brave_failure_returns_degraded_report() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let result = s
        .research(ResearchParams {
            query: Some("foo".into()),
            depth: 1,
            lang: Lang::Auto,
        })
        .await
        .expect("research should yield Ok(degraded) on Brave failure, not propagate error");

    assert!(
        result
            .degraded_reasons()
            .contains(&DegradedReason::BraveSearchFailed),
        "degraded_reasons must contain BraveSearchFailed; got: {:?}",
        result.degraded_reasons()
    );
    let data = result.data();
    assert_eq!(
        data["sources"].as_array().unwrap().len(),
        0,
        "data.sources must be empty when Brave failed"
    );
}

/// [T-TS030]
/// Setup: wiremock always returns HTTP 401.
/// Action: `Scout::research(...)` is invoked.
/// Expected: returns `Err(ScoutError)` (not a degraded `Ok`), because
/// `BraveError::Unauthorized` is a configuration error and must surface to
/// the user instead of being silently absorbed into the degraded envelope.
/// Companion to T-TS029 which covers the transient (503) degradable path.
#[tokio::test]
async fn research_unauthorized_propagates_as_error() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let result = s
        .research(ResearchParams {
            query: Some("foo".into()),
            depth: 1,
            lang: Lang::Auto,
        })
        .await;

    assert!(
        result.is_err(),
        "Unauthorized must propagate as Err, not be degraded; got: {result:?}"
    );
}

/// [T-TS031] zero results yield empty arrays, not null
#[tokio::test]
async fn research_json_zero_results_returns_empty_arrays() {
    let Some(server) = try_spawn_mock_server("tools::integration").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "web": {"results": []}
        })))
        .mount(&server)
        .await;

    let s = scout_with_brave(&server.uri());
    let params = ResearchParams {
        query: Some("foo".into()),
        depth: 1,
        lang: Lang::Auto,
    };
    let result = s.research(params).await.unwrap();
    let data = result.data();

    assert_eq!(
        data["sources"].as_array().unwrap().len(),
        0,
        "data.sources must be an empty array (not null)"
    );
    assert_eq!(
        data["fetched_pages"].as_array().unwrap().len(),
        0,
        "data.fetched_pages must be an empty array"
    );
    assert_eq!(
        data["failed_urls"].as_array().unwrap().len(),
        0,
        "data.failed_urls must be an empty array"
    );
}

/// [T-F070] collect_research_degradations pushes DecodeUncertain for an uncertain
/// page and omits it for a clean one (research-path machine-readable signal)
#[test]
fn collect_research_degradations_pushes_decode_uncertain() {
    use super::query::collect_research_degradations;
    use crate::envelope::{Degradation, DegradedReason};
    use crate::search::engine::ResearchReport;

    let report = ResearchReport {
        fetched_pages: vec![
            FetchResult::for_test("https://clean.example".into(), "Readable.".into(), false),
            FetchResult::for_test(
                "https://garbled.example".into(),
                "Best-effort.".into(),
                false,
            )
            .with_decode_uncertain(true),
        ],
        failed_urls: vec![],
        sources: vec![],
    };

    let mut degradation = Degradation::default();
    collect_research_degradations(&report, &mut degradation);
    let (notes, reasons) = degradation.into_parts();

    assert_eq!(
        reasons,
        vec![DegradedReason::DecodeUncertain],
        "only the uncertain page yields a DecodeUncertain reason, got: {reasons:?}"
    );
    assert!(
        notes[0].contains("https://garbled.example"),
        "note must name the uncertain URL, got: {notes:?}"
    );
    assert!(
        !notes[0].contains("https://clean.example"),
        "the clean page must not appear in the note, got: {notes:?}"
    );
}

/// [T-TS003]
#[test]
fn fetch_output_shifts_headings() {
    let result = FetchResult::for_test(
        "https://example.com".into(),
        "# Title\n## Section\nContent".into(),
        false,
    );
    let output = format_fetch_output(&result);
    assert!(output.contains("### Title"), "h1 should shift to h3");
    assert!(output.contains("#### Section"), "h2 should shift to h4");
}

/// [T-TS004]
#[test]
fn fetch_output_shifts_headings_with_raw_fallback() {
    let result = FetchResult::for_test(
        "https://example.com".into(),
        "# Raw Title\nBody".into(),
        true,
    );
    let output = format_fetch_output(&result);
    assert!(
        output.starts_with(RAW_FALLBACK_NOTE.trim_end()),
        "should prepend fallback note"
    );
    assert!(output.contains("### Raw Title"), "h1 should shift to h3");
}

/// [T-TS034] A body scout could not confidently decode says so in the Markdown
/// itself. Without `--json` the envelope's `degraded_reasons` never reach the
/// caller (src/lib.rs writes `into_markdown()` alone), so the note in the body
/// is the only place a default-mode reader learns the text may be garbled.
/// `research` already labels such a page in `format_fetched_pages`.
#[test]
fn fetch_output_marks_an_uncertain_decode() {
    let result = FetchResult::for_test("https://example.com".into(), "# Title\nBody".into(), false)
        .with_decode_uncertain(true);
    let output = format_fetch_output(&result);
    assert!(
        output.starts_with(DECODE_UNCERTAIN_NOTE.trim_end()),
        "an uncertain decode must be stated in the body, got: {output}"
    );
}

/// [T-TS035] Both notes fire together in the order `research` uses: what
/// produced the text (raw fallback) before what the text may suffer from
/// (garbled decode).
#[test]
fn fetch_output_orders_the_fallback_note_before_the_decode_note() {
    let result = FetchResult::for_test("https://example.com".into(), "# Title\nBody".into(), true)
        .with_decode_uncertain(true);
    let output = format_fetch_output(&result);
    assert_eq!(
        output.find(RAW_FALLBACK_NOTE.trim_end()),
        Some(0),
        "the fallback note must open the body, got: {output}"
    );
    assert!(
        output.find(DECODE_UNCERTAIN_NOTE.trim_end()) > output.find(RAW_FALLBACK_NOTE.trim_end()),
        "the decode note must follow the fallback note, got: {output}"
    );
}

/// [T-TS005]
#[test]
fn fetch_output_truncates_long_content() {
    let result = FetchResult::for_test(
        "https://example.com".into(),
        format!("# Title\n{}", "x".repeat(150_000)),
        false,
    );
    let output = format_fetch_output(&result);
    assert!(
        output.len() < 150_000,
        "output should be truncated, got {} bytes",
        output.len()
    );
    assert!(
        output.contains("(truncated: showing"),
        "should include truncation message"
    );
    assert!(
        output.contains("### Title"),
        "headings should still be shifted"
    );
}

/// [T-FC087] Fetch output truncated inside a closed fence leaves no live marker
///
/// `neutralize_yaml_markers_outside_fences` (src/yaml.rs) runs once, during
/// fetch conversion, over the whole body: a marker inside a fence that closes
/// before the body ends is left verbatim (fence-protected sample output, not
/// a forged document boundary). This fixture stands in for that already-
/// neutralized text directly, the way `FetchResult::for_test` always does,
/// with its own closing ``` placed far past `MAX_FETCH_OUTPUT_BYTES` so the
/// cut lands inside the fence body instead of at or after its close.
///
/// Once `format_fetch_output` truncates there, the fence that was genuinely
/// closed at neutralization time is left open in the truncated output, and
/// the `---` line inside it — raw only because its fence looked closed — is
/// exposed as a live, unprotected column-0 marker.
#[test]
fn fetch_output_truncated_inside_a_closed_fence_leaves_no_live_marker() {
    let filler = "x".repeat(80) + "\n";
    let markdown = format!(
        "# Title\n```\n---\nevil: true\n{}```\n",
        filler.repeat(1_300)
    );
    let result = FetchResult::for_test("https://example.com".into(), markdown, false);

    let output = format_fetch_output(&result);

    assert!(
        output.contains("(truncated: showing"),
        "output must actually be truncated for this scenario to be \
         meaningful, got:\n{output}"
    );
    assert!(
        !output.lines().any(|l| l == "---"),
        "a marker that survived verbatim only because its fence looked \
         closed must be re-neutralized once truncation removes that fence's \
         own closing delimiter, got output:\n{output}"
    );
}

/// A type whose `Serialize` impl always errors. Needed because the values scout
/// actually serializes never fail (`f64::NAN` serializes to `null`, it does not
/// error), so the error arm of `to_data_value` requires a forced failure.
struct FailingSerialize;

impl serde::Serialize for FailingSerialize {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(S::Error::custom("forced serialize failure"))
    }
}

/// [T-TDV001]
#[test]
fn to_data_value_serializes_owned_value() {
    let value = to_data_value(&serde_json::json!({"k": "v"}), "test value").unwrap();
    assert_eq!(value, serde_json::json!({"k": "v"}));
}

/// [T-TDV002] to_data_value maps a serialize failure to an Internal (exit 70)
/// ScoutError naming the value, so a handler serde failure surfaces through the
/// JSON error envelope via `?` instead of `.expect()` panicking.
#[test]
fn to_data_value_maps_serialize_failure_to_internal_bug() {
    let err = to_data_value(&FailingSerialize, "fetch result").unwrap_err();
    assert_eq!(err.error_kind(), ErrorCode::Internal);
    assert_eq!(err.exit_code(), 70, "expected EX_SOFTWARE (70)");
    assert!(
        err.message().contains("failed to serialize fetch result"),
        "message should name the value, got: {}",
        err.message()
    );
}

/// [T-FETCH-OK] fetch handler returns Ok for a reachable page, exercising the
/// success path end-to-end: page download, markdown render, and `data`
/// serialize (query.rs `to_data_value` delegation). The guard-free `fetch_http`
/// paired with a public-IP `with_dns` is the seam `ScoutBuilder::with_fetch_http`
/// documents; production keeps the connect-time guard.
#[tokio::test]
async fn fetch_returns_ok_for_reachable_page() {
    let Some(server) = try_spawn_mock_server("query::fetch_ok").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Scout Test</title></head><body><article><h1>Scout Test</h1>\
             <p>This is a sufficiently long article body so Readability extracts it cleanly \
             rather than falling back to raw conversion. Lorem ipsum dolor sit amet, \
             consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et \
             dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation.</p>\
             </article></body></html>",
        ))
        .mount(&server)
        .await;

    let addr = *server.address();
    let scout = scout_reaching(addr);

    let params = super::params::FetchParams::for_test(&format!(
        "http://scout-test.example:{}/page",
        addr.port()
    ));
    let output = scout.fetch(params).await.expect("fetch should succeed");

    let data = output.data();
    assert!(
        data["url"]
            .as_str()
            .is_some_and(|u| u.contains("scout-test.example")),
        "data.url should echo the fetched host, got: {data}"
    );
    assert!(
        data["markdown"]
            .as_str()
            .is_some_and(|m| m.contains("Scout Test")),
        "data.markdown should contain the page heading, got: {data}"
    );
    assert!(
        !output.markdown().is_empty(),
        "rendered markdown should be non-empty"
    );
}

/// [T-F071] fetch end-to-end flags `DegradedReason::DecodeUncertain` (exit 0) when
/// the page is an undecodable windows-1252 body mislabeled `charset=utf-8`.
/// Same guard-free `fetch_http` + public-IP `with_dns` seam as T-F017 keeps SSRF
/// intact; the body reuses the smart-quote bytes pinned by T-F067.
#[tokio::test]
async fn fetch_flags_decode_uncertain_for_undecodable_body() {
    let Some(server) = try_spawn_mock_server("query::fetch_decode_uncertain").await else {
        return;
    };
    let mut body = b"<html><body><p>It\x92s a fine day, isn\x92t it? ".to_vec();
    body.extend_from_slice(
        b"\x93Quoted\x94 text and an \x97 em dash, with plenty more prose.</p></body></html>",
    );
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;

    let addr = *server.address();
    let scout = scout_reaching(addr);

    let params = super::params::FetchParams::for_test(&format!(
        "http://scout-test.example:{}/page",
        addr.port()
    ));
    let output = scout.fetch(params).await.expect("fetch should succeed");

    assert!(
        output
            .degraded_reasons()
            .contains(&DegradedReason::DecodeUncertain),
        "undecodable body must surface DecodeUncertain at exit 0; got: {:?}",
        output.degraded_reasons()
    );
}

/// [T-SK057] `insert_preamble_notes` prepends the note at the very top when the
/// input carries no `---\n\n` frontmatter terminator. `format_slack_output`
/// always emits that terminator, so this path is unreachable in production, but
/// the fallback prepends rather than silently dropping the cap notes if that
/// shape ever changes.
#[test]
fn insert_preamble_notes_prepends_when_frontmatter_absent() {
    let out = super::query::insert_preamble_notes(
        "a body with no frontmatter".to_owned(),
        &["a cap note"],
    );
    assert!(
        out.starts_with("> Note: a cap note\n\n"),
        "absent frontmatter must fall back to a top prepend, got: {out}"
    );
    assert!(
        out.contains("a body with no frontmatter"),
        "the original body must be preserved after the prepended note, got: {out}"
    );
}
