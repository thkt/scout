use serde::{Deserialize, Serialize};

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
/// an empty result list.
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
    pub(super) fn into_results(self) -> Vec<SearchResult> {
        self.web.map(|w| w.results).unwrap_or_default()
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
}
