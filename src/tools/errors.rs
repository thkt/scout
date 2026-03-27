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
            | FetchError::UnsupportedContentType(_)
            | FetchError::RedirectMissingLocation => Self::user_error(e.to_string()),
            FetchError::Browser(_) => Self::user_error(e.to_string()),
            FetchError::Timeout(_) | FetchError::DnsResolution(_) => Self::internal(e.to_string()),
            FetchError::Http(_)
            | FetchError::Status(_)
            | FetchError::TooLarge
            | FetchError::TooManyRedirects(_) => Self::internal(e.to_string()),
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
    fn user_errors_have_exit_code_1() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::NotFound("/test".into()).into(),
            github::GitHubError::RateLimited.into(),
            github::GitHubError::Forbidden("denied".into()).into(),
            github::GitHubError::InvalidRepo("bad".into()).into(),
            FetchError::InvalidScheme.into(),
            FetchError::InternalHost.into(),
            FetchError::UnsupportedContentType("image/png".into()).into(),
            FetchError::RedirectMissingLocation.into(),
            FetchError::Browser("not installed".into()).into(),
            SlackError::TokenNotSet.into(),
            SlackError::Api { error: "err".into() }.into(),
            GeminiError::ApiKeyNotSet.into(),
            GeminiError::RateLimited.into(),
            GeminiError::QuotaExhausted("limit".into()).into(),
        ];
        for err in &cases {
            assert_eq!(err.exit_code(), 1, "expected user error (1): {err}");
        }
    }

    #[test]
    fn internal_errors_have_exit_code_2() {
        let cases: Vec<ScoutError> = vec![
            github::GitHubError::Api {
                code: 500,
                message: "server error".into(),
            }
            .into(),
            github::GitHubError::Decode("decode error".into()).into(),
            FetchError::Status(500).into(),
            FetchError::TooLarge.into(),
            FetchError::TooManyRedirects(10).into(),
            SlackError::Network("err".into()).into(),
            SlackError::Timeout("err".into()).into(),
            SlackError::Decode("err".into()).into(),
            GeminiError::Api {
                code: 500,
                message: "err".into(),
            }
            .into(),
        ];
        for err in &cases {
            assert_eq!(err.exit_code(), 2, "expected internal error (2): {err}");
        }
    }

    #[test]
    fn github_forbidden_hints_token() {
        let err = ScoutError::from(github::GitHubError::Forbidden("denied".into()));
        assert!(err.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn quota_exhausted_hints_billing_url() {
        let err = ScoutError::from(GeminiError::QuotaExhausted("limit".into()));
        assert!(err.to_string().contains("aistudio.google.com"));
    }
}
