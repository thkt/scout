use std::fmt;
use tracing::warn;

use crate::fetch::FetchError;
use crate::gemini::client::GeminiError;
use crate::github;
use crate::slack::SlackError;

#[derive(Debug)]
pub struct ScoutError {
    message: String,
    exit_code: i32,
}

impl fmt::Display for ScoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ScoutError {}

impl ScoutError {
    pub(super) fn user_error(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 1,
        }
    }

    pub(super) fn internal(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 2,
        }
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
}

pub(super) fn parse_repo_param(repository: &str) -> Result<(&str, &str), ScoutError> {
    github::parse_repo(repository).map_err(ScoutError::from)
}

impl From<github::GitHubError> for ScoutError {
    fn from(e: github::GitHubError) -> Self {
        match &e {
            github::GitHubError::NotFound(_)
            | github::GitHubError::InvalidRepo(_)
            | github::GitHubError::InvalidRef(_)
            | github::GitHubError::InvalidPath(_)
            | github::GitHubError::InvalidLineRange(_)
            | github::GitHubError::InvalidPattern(_) => Self::user_error(e.to_string()),
            github::GitHubError::RateLimited => Self::user_error(e.to_string()),
            github::GitHubError::Forbidden(_) => Self::user_error(format!(
                "{e} — check that your GITHUB_TOKEN has the required scopes"
            )),
            github::GitHubError::Api { .. }
            | github::GitHubError::Network(_)
            | github::GitHubError::Decode(_) => Self::internal(e.to_string()),
        }
    }
}

impl From<FetchError> for ScoutError {
    fn from(e: FetchError) -> Self {
        match &e {
            FetchError::InvalidScheme
            | FetchError::InvalidUrl(_)
            | FetchError::InternalHost
            | FetchError::UnsupportedContentType(_) => Self::user_error(e.to_string()),
            FetchError::Playwright(_) => Self::user_error(e.to_string()),
            FetchError::Timeout(_) | FetchError::DnsResolution(_) => Self::internal(e.to_string()),
            FetchError::Http(_) | FetchError::Status(_) | FetchError::TooLarge => {
                Self::internal(e.to_string())
            }
        }
    }
}

impl From<SlackError> for ScoutError {
    fn from(e: SlackError) -> Self {
        match &e {
            SlackError::TokenNotSet | SlackError::Api { .. } => Self::user_error(e.to_string()),
            SlackError::Network(_) | SlackError::Timeout(_) | SlackError::Decode(_) => {
                Self::internal(e.to_string())
            }
        }
    }
}

impl From<GeminiError> for ScoutError {
    fn from(e: GeminiError) -> Self {
        match &e {
            GeminiError::ApiKeyNotSet => Self::user_error(e.to_string()),
            GeminiError::RateLimited => Self::user_error(e.to_string()),
            GeminiError::QuotaExhausted(_) => Self::user_error(format!(
                "{e} — check your API billing at https://aistudio.google.com"
            )),
            GeminiError::Api { .. } | GeminiError::Network(_) => Self::internal(e.to_string()),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_not_found_is_user_error() {
        let err = ScoutError::from(github::GitHubError::NotFound("/test".into()));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn github_rate_limited_is_user_error() {
        let err = ScoutError::from(github::GitHubError::RateLimited);
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("rate limit"));
    }

    #[test]
    fn github_forbidden_hints_token() {
        let err = ScoutError::from(github::GitHubError::Forbidden("denied".into()));
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn github_api_error_is_internal() {
        let err = ScoutError::from(github::GitHubError::Api {
            code: 500,
            message: "server error".into(),
        });
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn fetch_invalid_scheme_is_user_error() {
        let err = ScoutError::from(FetchError::InvalidScheme);
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn fetch_status_is_internal() {
        let err = ScoutError::from(FetchError::Status(500));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn fetch_playwright_is_user_error() {
        let err = ScoutError::from(FetchError::Playwright("not installed".into()));
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("playwright"));
    }

    #[test]
    fn gemini_api_key_not_set_is_user_error() {
        let err = ScoutError::from(GeminiError::ApiKeyNotSet);
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("GEMINI_API_KEY"));
    }

    #[test]
    fn gemini_rate_limited_is_user_error() {
        let err = ScoutError::from(GeminiError::RateLimited);
        assert_eq!(err.exit_code(), 1);
    }
}
