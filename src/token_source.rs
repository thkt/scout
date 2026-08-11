//! Resolves a GitHub bearer token from env vars or `gh auth token`, behind a
//! trait so tests can skip the subprocess.

use std::env;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::process::Output;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

use crate::redacted::Redacted;

const TOKEN_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Object-safe boxed future returned by [`TokenSource::fetch`].
type TokenFuture<'a> = Pin<Box<dyn Future<Output = Option<Redacted>> + Send + 'a>>;

/// Resolves an optional GitHub bearer token. `Send + Sync` so implementations
/// can sit behind an `Arc<dyn TokenSource>` shared across async tasks.
pub(crate) trait TokenSource: Send + Sync {
    fn fetch(&self) -> TokenFuture<'_>;
}

/// Production source: `GITHUB_TOKEN` env → `GH_TOKEN` env → `gh auth token`.
pub(crate) struct GhCliSource;

impl TokenSource for GhCliSource {
    fn fetch(&self) -> TokenFuture<'_> {
        Box::pin(resolve_from_env_or_gh(|var| env::var(var).ok(), spawn_gh))
    }
}

/// The one place the subprocess is actually launched. Injected into
/// [`resolve_from_env_or_gh`] the same way the env reader is, so the three
/// outcomes that follow it — a token on stdout, a non-zero exit, a run that
/// outlives the timeout — are reachable from a test without a real `gh` on the
/// machine deciding which one a run takes.
fn spawn_gh() -> impl Future<Output = io::Result<Output>> {
    Command::new("gh")
        .args(["auth", "token"])
        .kill_on_drop(true)
        .output()
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

async fn resolve_from_env_or_gh<F, C, Fut>(env_reader: F, run_gh: C) -> Option<Redacted>
where
    F: Fn(&str) -> Option<String>,
    C: FnOnce() -> Fut,
    Fut: Future<Output = io::Result<Output>>,
{
    if let Some(token) = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|var| env_reader(var))
        .find_map(|t| Redacted::new(&t))
    {
        return Some(token);
    }

    let output = timeout(TOKEN_RESOLVE_TIMEOUT, run_gh())
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
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use tokio::time::sleep;

    use super::*;

    /// Build the `Output` a finished `gh auth token` would hand back.
    fn gh_output(code: i32, stdout: &str, stderr: &str) -> io::Result<Output> {
        Ok(Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    /// The subprocess must not run when an env var already answered.
    async fn gh_must_not_run() -> io::Result<Output> {
        panic!("the gh subprocess must not be reached when an env var supplied a token")
    }

    /// [T-TOK001] resolve_from_env_or_gh short-circuits to the GITHUB_TOKEN
    /// value when the env reader returns one, without falling through to the
    /// gh CLI subprocess.
    #[tokio::test]
    async fn resolve_from_env_or_gh_reads_github_token_env() {
        let token = resolve_from_env_or_gh(
            |key| (key == "GITHUB_TOKEN").then(|| "test-token-from-env".to_owned()),
            gh_must_not_run,
        )
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
        let token = resolve_from_env_or_gh(
            |key| match key {
                "GITHUB_TOKEN" => Some("   ".to_owned()),
                "GH_TOKEN" => Some("real-token".to_owned()),
                _ => None,
            },
            gh_must_not_run,
        )
        .await;
        assert_eq!(token.as_ref().map(Redacted::expose), Some("real-token"));
    }

    /// [T-TOK003] with no env token, the subprocess's stdout becomes the token
    ///
    /// `gh auth token` prints a trailing newline, which `Redacted::new` trims.
    /// DR-0018 says these tests catch a change in `gh`'s output contract; until
    /// the subprocess was injectable they could not reach it at all, because
    /// whether this path ran depended on the machine having a logged-in `gh`.
    #[tokio::test]
    async fn resolve_from_env_or_gh_takes_the_subprocess_stdout() {
        let token =
            resolve_from_env_or_gh(|_| None, || async { gh_output(0, "gho_abc123\n", "") }).await;

        assert_eq!(token.as_ref().map(Redacted::expose), Some("gho_abc123"));
    }

    /// [T-TOK004] a non-zero exit yields no token and withholds stderr
    ///
    /// The SEC comment on that arm says stderr is dropped because `gh` can echo
    /// the token back through it. Nothing asserted it, so a later change that
    /// logged stderr "for diagnosis" would have looked harmless.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn resolve_from_env_or_gh_withholds_stderr_on_failure() {
        let token = resolve_from_env_or_gh(
            |_| None,
            || async { gh_output(1, "", "error: token gho_leaked_secret is invalid") },
        )
        .await;

        assert!(token.is_none(), "a failed subprocess yields no token");
        assert!(
            !logs_contain("gho_leaked_secret"),
            "stderr must never reach the log"
        );
        assert!(
            logs_contain("stderr withheld"),
            "the warn should say the stderr was dropped on purpose"
        );
    }

    /// [T-TOK005] an empty stdout on success is no token, not an empty one
    #[tokio::test]
    async fn resolve_from_env_or_gh_rejects_blank_subprocess_output() {
        let token = resolve_from_env_or_gh(|_| None, || async { gh_output(0, "  \n", "") }).await;

        assert!(token.is_none(), "whitespace-only stdout is not a token");
    }

    /// [T-TOK006] a subprocess that outlives the timeout falls back to unauthenticated
    ///
    /// `start_paused` advances the clock rather than waiting the real 5s.
    #[tokio::test(start_paused = true)]
    async fn resolve_from_env_or_gh_times_out() {
        let token = resolve_from_env_or_gh(
            |_| None,
            || async {
                sleep(TOKEN_RESOLVE_TIMEOUT * 2).await;
                gh_output(0, "too-late", "")
            },
        )
        .await;

        assert!(token.is_none(), "a run past the timeout yields no token");
    }
}
