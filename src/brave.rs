//! Brave Search API client and response types.
//!
//! Provides the `BraveClient` implementation of `SearchClient` used by
//! the `search` and `research` subcommands. Returns real source URLs
//! (not redirect URLs) and never includes LLM-generated summaries,
//! per `.claude/OUTCOME.md`.

pub(crate) mod client;
pub(crate) mod types;
