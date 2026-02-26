use tracing::warn;

use super::types::{GenerateContentResponse, GroundedResult, Source};

pub fn extract_grounded_result(response: &GenerateContentResponse) -> GroundedResult {
    let candidate = response.candidates.as_ref().and_then(|c| c.first());

    let answer = candidate
        .and_then(|c| c.content.as_ref())
        .and_then(|content| content.parts.first())
        .map(|part| part.text.clone())
        .filter(|text| !text.is_empty());

    if answer.is_none() {
        warn!("Gemini returned empty answer (safety filter or empty response)");
    }

    let metadata = candidate.and_then(|c| c.grounding_metadata.as_ref());

    let sources = metadata
        .and_then(|m| m.grounding_chunks.as_ref())
        .map(|chunks| {
            chunks
                .iter()
                .filter_map(|chunk| {
                    let web = chunk.web.as_ref()?;
                    let url = web.uri.as_ref().filter(|u| !u.is_empty())?.clone();
                    Some(Source {
                        url,
                        title: web.title.clone().unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    GroundedResult { answer, sources }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::types::*;

    fn make_response(answer: &str, chunks: Vec<GroundingChunk>) -> GenerateContentResponse {
        GenerateContentResponse {
            candidates: Some(vec![Candidate {
                content: Some(Content {
                    parts: vec![Part {
                        text: answer.to_string(),
                    }],
                    role: Some("model".to_string()),
                }),
                grounding_metadata: Some(GroundingMetadata {
                    grounding_chunks: Some(chunks),
                }),
            }]),
            error: None,
        }
    }

    #[test]
    fn extracts_answer_and_sources() {
        let response = make_response(
            "React 19 introduces new features.",
            vec![GroundingChunk {
                web: Some(WebChunk {
                    uri: Some("https://react.dev/blog".into()),
                    title: Some("React Blog".into()),
                }),
            }],
        );

        let result = extract_grounded_result(&response);

        assert_eq!(
            result.answer.as_deref(),
            Some("React 19 introduces new features.")
        );
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].url, "https://react.dev/blog");
        assert_eq!(result.sources[0].title, "React Blog");
    }

    #[test]
    fn handles_multiple_sources() {
        let response = make_response(
            "Answer",
            vec![
                GroundingChunk {
                    web: Some(WebChunk {
                        uri: Some("https://a.com".into()),
                        title: Some("Site A".into()),
                    }),
                },
                GroundingChunk {
                    web: Some(WebChunk {
                        uri: Some("https://b.com".into()),
                        title: Some("Site B".into()),
                    }),
                },
            ],
        );

        let result = extract_grounded_result(&response);

        assert_eq!(result.sources.len(), 2);
    }

    #[test]
    fn handles_empty_response() {
        let response = GenerateContentResponse {
            candidates: None,
            error: None,
        };

        let result = extract_grounded_result(&response);

        assert!(result.answer.is_none());
        assert!(result.sources.is_empty());
    }

    #[test]
    fn handles_missing_metadata() {
        let response = GenerateContentResponse {
            candidates: Some(vec![Candidate {
                content: Some(Content {
                    parts: vec![Part {
                        text: "No grounding".into(),
                    }],
                    role: Some("model".into()),
                }),
                grounding_metadata: None,
            }]),
            error: None,
        };

        let result = extract_grounded_result(&response);

        assert_eq!(result.answer.as_deref(), Some("No grounding"));
        assert!(result.sources.is_empty());
    }

    #[test]
    fn skips_chunks_without_web_or_empty_uri() {
        let response = make_response(
            "Answer",
            vec![
                GroundingChunk { web: None },
                GroundingChunk {
                    web: Some(WebChunk {
                        uri: None,
                        title: Some("No URI".into()),
                    }),
                },
                GroundingChunk {
                    web: Some(WebChunk {
                        uri: Some("".into()),
                        title: Some("Empty URI".into()),
                    }),
                },
                GroundingChunk {
                    web: Some(WebChunk {
                        uri: Some("https://valid.com".into()),
                        title: Some("Valid".into()),
                    }),
                },
            ],
        );

        let result = extract_grounded_result(&response);

        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].url, "https://valid.com");
    }
}
