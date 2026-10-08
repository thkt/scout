//! Brave Search API client and response types.
//!
//! `BraveClient` implements `SearchClient` for search and research, returning
//! source URLs and engine snippets without LLM-generated summaries.

pub(crate) mod client;
pub(crate) mod types;
