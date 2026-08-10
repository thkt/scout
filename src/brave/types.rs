use serde::{Deserialize, Serialize};
use tracing::warn;

/// Canonical search result type returned by `SearchClient::search`.
///
/// `description` is the search engine snippet (not an LLM-generated summary)
/// and is included to help AI agents pre-filter results before fetching.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SearchResult {
    pub(crate) url: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
}

/// Top-level Brave Web Search API response.
///
/// The Brave API returns many top-level fields; scout consumes only `web.results[]`.
/// Unknown fields are silently dropped by serde. A missing or null `web` becomes
/// `None` because the field is `Option`, and [`Self::into_results`] maps that to
/// an empty result list — with a warn, for the reason given there.
#[derive(Debug, Deserialize)]
pub(super) struct WebSearchResponse {
    pub(super) web: Option<WebSearch>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WebSearch {
    #[serde(default)]
    pub(super) results: Vec<SearchResult>,
}

impl WebSearchResponse {
    /// Both a genuine zero-result search and a response shaped differently than
    /// scout expects arrive here as an empty list, and `scout search` reports
    /// either as exit 0 with no output. The two are indistinguishable from the
    /// result alone, so the second one warns: `classify_response` has already
    /// ruled out an error status, which leaves "Brave answered 200 without a
    /// `web` object" meaning its response shape moved. A real empty search still
    /// carries `web`, so this stays quiet for it.
    pub(super) fn into_results(self) -> Vec<SearchResult> {
        match self.web {
            Some(web) => web.results,
            None => {
                warn!("Brave returned 200 with no `web` object; reporting zero results");
                Vec::new()
            }
        }
    }
}

// T-BT### = Brave Types module-internal tests (serde deserialization coverage).
// See `crate::test_support` for the id convention itself.
#[cfg(test)]
mod tests {
    use super::*;

    /// [T-BT001] WebSearchResponse parses results from minimal payload
    #[test]
    fn parses_minimal_results() {
        let body = r#"{
            "web": {
                "results": [
                    {"url": "https://example.com", "title": "Example", "description": "snippet"}
                ]
            }
        }"#;
        let parsed: WebSearchResponse = serde_json::from_str(body).unwrap();
        let results = parsed.into_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].title, "Example");
        assert_eq!(results[0].description, "snippet");
    }

    /// [T-BT002]
    #[test]
    fn tolerates_missing_description() {
        let body = r#"{
            "web": {
                "results": [
                    {"url": "https://example.com", "title": "Example"}
                ]
            }
        }"#;
        let parsed: WebSearchResponse = serde_json::from_str(body).unwrap();
        let results = parsed.into_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].description, "");
    }

    /// [T-BT003]
    #[test]
    fn empty_results_when_web_absent() {
        let body = r#"{}"#;
        let parsed: WebSearchResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.into_results().is_empty());
    }

    /// [T-BT004] WebSearchResponse tolerates unknown extra fields
    #[test]
    fn tolerates_unknown_fields() {
        let body = r#"{
            "type": "search",
            "mixed": {"main": []},
            "web": {
                "type": "search",
                "results": [
                    {"url": "https://a.com", "title": "A", "description": "d", "extra_field": "ignored"}
                ]
            }
        }"#;
        let parsed: WebSearchResponse = serde_json::from_str(body).unwrap();
        let results = parsed.into_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://a.com");
    }

    /// [T-BT005] a 200 without `web` warns before reporting zero results
    ///
    /// `classify_response` has already rejected error statuses, so this shape
    /// means Brave's response moved. Without the warn it is indistinguishable
    /// from a search that genuinely matched nothing: both exit 0 with no output.
    #[tracing_test::traced_test]
    #[test]
    fn missing_web_object_warns() {
        let parsed: WebSearchResponse = serde_json::from_str("{}").unwrap();

        assert!(parsed.into_results().is_empty());
        assert!(
            logs_contain("no `web` object"),
            "a missing web object must be visible to the operator"
        );
    }

    /// [T-BT006] a genuine empty result set stays quiet
    ///
    /// The companion to T-BT005: warning on every zero-result search would make
    /// the signal worthless.
    #[tracing_test::traced_test]
    #[test]
    fn genuinely_empty_results_do_not_warn() {
        let body = r#"{"web": {"results": []}}"#;
        let parsed: WebSearchResponse = serde_json::from_str(body).unwrap();

        assert!(parsed.into_results().is_empty());
        assert!(
            !logs_contain("no `web` object"),
            "an ordinary empty search must not warn"
        );
    }

    /// [T-BT007] a result missing `url` or `title` fails the whole parse
    ///
    /// Only `description` carries `#[serde(default)]`, so one malformed entry
    /// costs the entire response rather than itself. Pinned because it is a
    /// choice, not an accident: the alternative — dropping bad entries — would
    /// report a partial result set as if it were complete.
    #[test]
    fn result_missing_required_field_fails_the_parse() {
        for body in [
            r#"{"web": {"results": [{"title": "no url"}]}}"#,
            r#"{"web": {"results": [{"url": "https://a.test"}]}}"#,
        ] {
            assert!(
                serde_json::from_str::<WebSearchResponse>(body).is_err(),
                "a result without url or title must not parse: {body}"
            );
        }
    }
}
