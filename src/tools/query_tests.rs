use crate::fetch::converter::plain_text_result;
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

/// [T-TS029] Transient Brave failure degrades research to a successful empty
/// report carrying BraveSearchFailed instead of propagating a hard error.
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

/// [T-TS030] Authentication failure must remain a hard configuration error,
/// unlike the degradable transient failure in T-TS029.
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

/// [T-TS034] Non-JSON callers see only Markdown, so decoding uncertainty
/// must appear in the body as well as the envelope.
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

/// [T-TS035] Simultaneous notes must match research ordering: raw fallback
/// before decoding uncertainty.
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

/// [T-FC087] The fixture represents already-neutralized text whose YAML marker
/// was protected by a closed fence. The output cap removes that close; the
/// newly dangling fence must not expose a live column-zero marker.
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

/// Force a serialization error: ordinary scout values do not reach this arm
/// (even NaN serializes as null).
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

/// [T-TDV002] Serialization failure must return Internal (exit 70), not panic.
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

/// [T-FETCH-OK] Exercise fetch success through download, conversion and JSON
/// serialization. The loopback client plus public pre-flight resolver is the
/// documented test seam; production retains the connect-time guard.
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

/// [T-F071] Undecodable mislabeled bytes must reach fetch as DecodeUncertain
/// at exit 0. The loopback client/public pre-flight resolver is a test seam;
/// the fixture reuses T-F067's smart-quote bytes.
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

/// [T-SK057] Without the frontmatter terminator, preamble notes must still be
/// prepended. Current Slack formatting always supplies that terminator.
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

/// [T-TS039] Plain-text headings and trailing blank lines survive the output boundary.
#[test]
fn fetch_output_preserves_plain_text_literals() {
    let result = plain_text_result(
        "Title\n=====\n# comment\nbody\n\n",
        "https://example.com".into(),
        false,
    );
    assert_eq!(
        format_fetch_output(&result),
        "---\n---\n\nTitle\n=====\n# comment\nbody\n\n"
    );
}
