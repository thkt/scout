//! Resolves a GitHub bearer token from env vars or `gh auth token`, behind a
//! trait so tests can skip the subprocess.

use std::env;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

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
    if let Some(token) = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|var| env_reader(var))
        .find_map(|t| Redacted::new(&t))
    {
        return Some(token);
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
        warn!(
            "gh auth token timed out after {}s; falling back to unauthenticated",
            TOKEN_RESOLVE_TIMEOUT.as_secs()
        )
    })
    .ok()?
    .inspect_err(
        |e| warn!(error = %e, "gh auth token command failed; falling back to unauthenticated"),
    )
    .ok()?;

    if !output.status.success() {
        // SEC: `gh auth token` stderr can echo back the token or other secrets,
        // so the raw stderr is dropped; only the exit code is reported.
        warn!(
            redacted_reason = %format!(
                "non-zero exit (code {}); stderr withheld",
                output.status.code().unwrap_or(-1)
            ),
            "gh auth token failed; falling back to unauthenticated"
        );
        return None;
    }

    Redacted::new(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-TOK001] resolve_from_env_or_gh short-circuits to the GITHUB_TOKEN
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

    /// [T-TOK002] Empty/whitespace env values must not register as "set"; the
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
}
