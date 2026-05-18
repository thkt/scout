//! Resolves a GitHub bearer token from env vars or `gh auth token`, behind a
//! trait so tests can skip the subprocess.

use std::env;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::info;

use crate::redacted::Redacted;

const TOKEN_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Object-safe boxed future returned by [`TokenSource::fetch`].
pub(crate) type TokenFuture<'a> = Pin<Box<dyn Future<Output = Option<Redacted>> + Send + 'a>>;

/// Resolves an optional GitHub bearer token. `Send + Sync` so implementations
/// can sit behind an `Arc<dyn TokenSource>` shared across async tasks.
pub(crate) trait TokenSource: Send + Sync {
    fn fetch(&self) -> TokenFuture<'_>;
}

/// Production source: `GITHUB_TOKEN` env → `GH_TOKEN` env → `gh auth token`.
pub(crate) struct GhCliSource;

impl TokenSource for GhCliSource {
    fn fetch(&self) -> TokenFuture<'_> {
        Box::pin(resolve_from_env_or_gh(|var| env::var(var).ok()))
    }
}

/// Test source that returns its constructor argument verbatim.
#[cfg(test)]
pub(crate) struct StaticTokenSource(pub Option<Redacted>);

#[cfg(test)]
impl TokenSource for StaticTokenSource {
    fn fetch(&self) -> TokenFuture<'_> {
        let token = self.0.clone();
        Box::pin(async move { token })
    }
}

async fn resolve_from_env_or_gh<F>(env_reader: F) -> Option<Redacted>
where
    F: Fn(&str) -> Option<String>,
{
    let from_env = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|var| env_reader(var))
        .map(|t| t.trim().to_owned())
        .find(|t| !t.is_empty());

    if let Some(token) = from_env {
        return Some(Redacted::new(&token));
    }

    let output = timeout(
        TOKEN_RESOLVE_TIMEOUT,
        Command::new("gh")
            .args(["auth", "token"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .inspect_err(|_| {
        info!(
            "gh auth token timed out after {}s",
            TOKEN_RESOLVE_TIMEOUT.as_secs()
        )
    })
    .ok()?
    .inspect_err(|e| info!("gh auth token command failed: {e}"))
    .ok()?;

    if !output.status.success() {
        info!(
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "gh auth token failed"
        );
        return None;
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!token.is_empty()).then(|| Redacted::new(&token))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-TS001] resolve_from_env_or_gh short-circuits to the GITHUB_TOKEN
    /// value when the env reader returns one, without falling through to the
    /// gh CLI subprocess.
    #[tokio::test]
    async fn resolve_from_env_or_gh_reads_github_token_env() {
        let token = resolve_from_env_or_gh(|key| {
            (key == "GITHUB_TOKEN").then(|| "test-token-from-env".to_owned())
        })
        .await;
        assert_eq!(
            token.as_ref().map(Redacted::expose),
            Some("test-token-from-env")
        );
    }

    /// [T-TS002] Empty/whitespace env values must not register as "set"; the
    /// resolver falls through to the next candidate in the chain.
    #[tokio::test]
    async fn resolve_from_env_or_gh_skips_whitespace_env() {
        let token = resolve_from_env_or_gh(|key| match key {
            "GITHUB_TOKEN" => Some("   ".to_owned()),
            "GH_TOKEN" => Some("real-token".to_owned()),
            _ => None,
        })
        .await;
        assert_eq!(token.as_ref().map(Redacted::expose), Some("real-token"));
    }

    /// [T-TS003] StaticTokenSource(Some(...)) propagates the constructor value
    /// to fetch() callers without ever spawning the gh subprocess.
    #[tokio::test]
    async fn static_token_source_returns_constructor_value() {
        let source = StaticTokenSource(Some(Redacted::new("fixed")));
        let token = source.fetch().await;
        assert_eq!(token.as_ref().map(Redacted::expose), Some("fixed"));
    }

    /// [T-TS004] StaticTokenSource(None) simulates the unauthenticated path so
    /// callers can verify rate-limit-tier handling.
    #[tokio::test]
    async fn static_token_source_none_returns_none() {
        let source = StaticTokenSource(None);
        let token = source.fetch().await;
        assert!(token.is_none());
    }
}
