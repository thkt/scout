use super::query::to_data_value;
use super::test_helpers::*;
use super::*;
use crate::search::Lang;
use crate::test_support::try_spawn_mock_server;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-009] search returns plain URL list with no markdown decoration
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

/// [T-009-json] search --json output schema (data.query, data.sources, no data.answer)
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

/// [T-015] search command issues exactly one Brave call (no engine::research fanout)
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

/// [T-009-empty] search with zero results returns empty stdout and exit 0
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

/// [T-TS002] research returns report with Brave sources and no obsolete Search Result header
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
        "AC-3.1: report must not contain the obsolete Search Result header"
    );
    assert!(
        !result
            .markdown()
            .contains("vertexaisearch.cloud.google.com"),
        "AC-3.2: Sources must not contain Google redirect URLs"
    );
}

/// [T-10] AC-4.2: --json research data schema (query, sources, fetched_pages, failed_urls)
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
        "data.answer must be absent (AC-4.1: no LLM-generated answer)"
    );
    assert!(
        data.get("all_sources").is_none(),
        "data.all_sources is the legacy key — must be renamed to sources"
    );
}

/// [T-028] (unit / FR-019)
/// Setup: wiremock always returns HTTP 503 (still fails after retry).
/// Action: `Scout::research(...)` is invoked.
/// Expected: returns `Ok(CommandOutput)` (no hard-fail);
/// `degraded_reasons` contains `BraveSearchFailed`; `data.sources` is empty.
/// RC-03 fix: cascade no longer propagates `BraveError`; failure is absorbed
/// into the degraded report envelope.
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

/// [T-029] (unit / FR-019)
/// Setup: wiremock always returns HTTP 401.
/// Action: `Scout::research(...)` is invoked.
/// Expected: returns `Err(ScoutError)` (not a degraded `Ok`), because
/// `BraveError::Unauthorized` is a configuration error and must surface to
/// the user instead of being silently absorbed into the degraded envelope.
/// Companion to T-028 which covers the transient (503) degradable path.
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

/// [T-11] AC-4.3: zero results yield empty arrays, not null
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

/// [T-TS003] fetch_output_shifts_headings
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

/// [T-TS004] fetch_output_shifts_headings_with_raw_fallback
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

/// [T-TS005] fetch_output_truncates_long_content
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

/// A type whose `Serialize` impl always errors. Needed because the values scout
/// actually serializes never fail (`f64::NAN` serializes to `null`, it does not
/// error), so the error arm of `to_data_value` requires a forced failure.
struct FailingSerialize;

impl serde::Serialize for FailingSerialize {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom("forced serialize failure"))
    }
}

/// [T-TDV001] to_data_value returns the serialized JSON value on success.
#[test]
fn to_data_value_serializes_owned_value() {
    let value = to_data_value(&serde_json::json!({"k": "v"}), "test value").unwrap();
    assert_eq!(value, serde_json::json!({"k": "v"}));
}

/// [T-TDV002] to_data_value maps a serialize failure to an Internal (exit 70)
/// ScoutError naming the value, so a handler serde failure surfaces through the
/// JSON error envelope via `?` instead of `.expect()` panicking (issue #192).
#[test]
fn to_data_value_maps_serialize_failure_to_internal_bug() {
    let err = to_data_value(&FailingSerialize, "fetch result").unwrap_err();
    assert_eq!(err.error_kind(), crate::envelope::ErrorCode::Internal);
    assert_eq!(err.exit_code(), 70, "expected EX_SOFTWARE (70)");
    assert!(
        err.message().contains("failed to serialize fetch result"),
        "message should name the value, got: {}",
        err.message()
    );
}
