use crate::classify::Classification;
use crate::envelope::ErrorCode;
use crate::retry::MAX_GITHUB_RESPONSE_BYTES;

#[derive(Debug, thiserror::Error)]
pub(crate) enum GitHubError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error(
        "GitHub API rate limit exceeded. Set GITHUB_TOKEN or run `gh auth login` for higher limits."
    )]
    RateLimited { retry_after: Option<u64> },

    #[error("Access denied: {0}")]
    Forbidden(String),

    #[error("GitHub API error ({code}): {message}")]
    Api { code: u16, message: String },

    #[error("Network error: {0}")]
    Network(#[source] reqwest::Error),

    #[error("Invalid repository format: expected 'owner/repo', got '{0}'")]
    InvalidRepo(String),

    #[error("Invalid ref: {0}")]
    InvalidRef(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("'{0}' is a directory, not a file")]
    PathIsDirectory(String),

    #[error("Invalid line range: '{0}'. Use formats like '1-80', '50-', or '100' (first N lines).")]
    InvalidLineRange(String),

    #[error("Invalid glob pattern: {0}")]
    InvalidPattern(String),

    #[error("GitHub API response too large (>{MAX_GITHUB_RESPONSE_BYTES} bytes)")]
    ResponseTooLarge,

    #[error("Content decode error: {0}")]
    Decode(String),

    #[error("{0}")]
    NonUtf8(String),

    #[error("Insecure URL: HTTPS required for token-bearing request")]
    InsecureUrl,
}

/// Hand-written (not `#[from]`) so the conversion strips the request URL:
/// reqwest's `Display` appends `for url (…)` including the query string.
impl From<reqwest::Error> for GitHubError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.without_url())
    }
}

impl GitHubError {
    /// Map each variant to its ADR-0011 priority-table [`Classification`].
    ///
    /// Arm order is load-bearing: `Api { code: 401 }` precedes the generic 4xx
    /// arm so a reorder cannot silently demote 401 from UsageError(64) to
    /// DataError(65).
    pub(crate) fn classify(&self) -> Classification {
        match self {
            // Priority 1: USAGE_ERROR
            Self::Forbidden(_) => Classification::new(ErrorCode::UsageError)
                .with_hint("Check that your GITHUB_TOKEN has the required scopes"),
            // 401 must precede the 4xx arm below to avoid falling into DataError.
            Self::Api { code: 401, .. } => Classification::new(ErrorCode::UsageError)
                .with_hint("Set GITHUB_TOKEN or run `gh auth login` to authenticate"),
            // Priority 2: DATA_ERROR
            Self::InvalidRepo(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Use 'owner/repo' format, e.g., 'facebook/react'"),
            Self::InvalidRef(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Use a branch name, tag, or commit SHA"),
            Self::InvalidPath(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Use a path within the repository"),
            Self::PathIsDirectory(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Use `scout repo-tree` to list a directory, or pass a file path"),
            Self::InvalidLineRange(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Use format like '1-80', '50-', or '100' (first N lines)"),
            Self::InvalidPattern(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Use a glob pattern like '*.rs' or '*.{ts,tsx}'"),
            Self::NonUtf8(_) => Classification::new(ErrorCode::DataError)
                .with_hint("Pass --encoding to decode non-UTF-8 files (e.g., shift_jis)"),
            Self::InsecureUrl => Classification::new(ErrorCode::DataError),
            // Priority 3: NOT_FOUND
            Self::NotFound(_) => Classification::new(ErrorCode::NotFound)
                .with_hint("Check that the repository or path exists, and that you have access"),
            // Priority 4: TEMP_FAILURE
            Self::RateLimited { retry_after } => Classification::new(ErrorCode::TempFailure)
                .with_hint(match retry_after {
                    Some(secs) => format!(
                        "Retry after {secs} seconds, or set GITHUB_TOKEN to increase rate limit"
                    ),
                    None => "Set GITHUB_TOKEN to increase rate limit".to_owned(),
                }),
            // Priority 4 (TIMEOUT) and 退避: see `Classification::from_reqwest`
            Self::Network(re) => Classification::from_reqwest(re),
            // Priority 5: INTERNAL — scout-side bug (unexpected schema) or a
            // response that overran the byte cap (issue #186; peer to
            // BraveError::ResponseTooLarge). Non-retriable: a retry would refetch
            // the same oversized body.
            Self::Decode(_) | Self::ResponseTooLarge => Classification::new(ErrorCode::Internal),
            // Every remaining status follows the ADR-0003 table.
            Self::Api { code, .. } => Classification::from_http_status(*code),
        }
    }
}

#[cfg(test)]
mod classify_tests;
