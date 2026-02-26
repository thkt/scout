use rmcp::ErrorData as McpError;
use tracing::warn;

use crate::fetch::FetchError;
use crate::gemini::client::GeminiError;
use crate::github;

pub(super) fn parse_repo_param(repository: &str) -> Result<(&str, &str), McpError> {
    github::parse_repo(repository).map_err(github_to_mcp_error)
}

pub(super) fn retriable_error(e: &impl std::fmt::Display) -> McpError {
    McpError::internal_error(format!("{e} (retriable)"), None)
}

pub(super) fn github_to_mcp_error(e: github::GitHubError) -> McpError {
    match &e {
        github::GitHubError::NotFound(_)
        | github::GitHubError::InvalidRepo(_)
        | github::GitHubError::InvalidRef(_)
        | github::GitHubError::InvalidPath(_)
        | github::GitHubError::InvalidLineRange(_)
        | github::GitHubError::InvalidPattern(_) => McpError::invalid_params(e.to_string(), None),
        github::GitHubError::RateLimited => retriable_error(&e),
        github::GitHubError::Forbidden(_) => McpError::internal_error(
            format!("{e} — check that your GITHUB_TOKEN has the required scopes"),
            None,
        ),
        _ => McpError::internal_error(e.to_string(), None),
    }
}

pub(super) fn fetch_to_mcp_error(e: FetchError) -> McpError {
    match &e {
        FetchError::InvalidScheme
        | FetchError::InvalidUrl(_)
        | FetchError::InternalHost
        | FetchError::UnsupportedContentType(_) => McpError::invalid_params(e.to_string(), None),
        _ => McpError::internal_error(e.to_string(), None),
    }
}

pub(super) fn unwrap_or_note<T>(
    result: Result<Vec<T>, github::GitHubError>,
    label: &str,
    notes: &mut Vec<String>,
) -> Vec<T> {
    match result {
        Ok(v) => v,
        Err(e) => {
            warn!(%e, "failed to fetch {}", label);
            notes.push(format!("Could not fetch {label} ({e})"));
            vec![]
        }
    }
}

pub(super) fn gemini_to_mcp_error(e: GeminiError) -> McpError {
    match &e {
        GeminiError::ApiKeyNotSet => McpError::invalid_params(e.to_string(), None),
        GeminiError::RateLimited => retriable_error(&e),
        GeminiError::QuotaExhausted(_) => McpError::invalid_params(
            format!("{e} — check your API billing at https://aistudio.google.com"),
            None,
        ),
        _ => McpError::internal_error(e.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_to_mcp_error_rate_limited_is_retriable() {
        let err = github_to_mcp_error(github::GitHubError::RateLimited);
        assert!(err.message.contains("retriable"));
    }

    #[test]
    fn github_to_mcp_error_forbidden_hints_token() {
        let err = github_to_mcp_error(github::GitHubError::Forbidden("denied".into()));
        assert!(err.message.contains("GITHUB_TOKEN"));
    }

    #[test]
    fn fetch_to_mcp_error_invalid_scheme_is_invalid_params() {
        let err = fetch_to_mcp_error(FetchError::InvalidScheme);
        assert!(err.message.contains("HTTP(S)"), "got: {}", err.message);
        assert_eq!(err.code, rmcp::model::ErrorCode(-32602));
    }

    #[test]
    fn fetch_to_mcp_error_http_is_internal_error() {
        let err = fetch_to_mcp_error(FetchError::Status(500));
        assert_eq!(err.code, rmcp::model::ErrorCode(-32603));
    }
}
