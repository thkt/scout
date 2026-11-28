//! Resolves the Brave and Slack secrets from their env var, then from the
//! macOS Keychain (DR-0039), behind a trait so tests never read the real
//! Keychain.

use std::env;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::process::Output;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::redacted::Redacted;

const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(5);
const SECURITY_BIN: &str = "/usr/bin/security";
/// `security find-generic-password` exit code for "the specified item could not
/// be found in the keychain", the ordinary state for a user who never stored it.
const ITEM_NOT_FOUND: i32 = 44;

/// Set to `1` to skip the Keychain lookup, so a run on a Mac that holds the
/// secret in its Keychain can still exercise the secret-unset path.
pub(crate) const NO_KEYCHAIN_VAR: &str = "SCOUT_NO_KEYCHAIN";

/// Object-safe boxed future returned by [`SecretSource::fetch`].
type SecretFuture<'a> = Pin<Box<dyn Future<Output = Option<Redacted>> + Send + 'a>>;

/// Resolves the secret an env var names. `Send + Sync` so implementations can
/// sit behind an `Arc<dyn SecretSource>` shared across async tasks.
pub(crate) trait SecretSource: Send + Sync {
    fn fetch<'a>(&'a self, name: &'a str) -> SecretFuture<'a>;
}

/// Production source: the env var, then the Keychain item whose service is the
/// env var's name.
pub(crate) struct EnvThenKeychain;

impl SecretSource for EnvThenKeychain {
    fn fetch<'a>(&'a self, name: &'a str) -> SecretFuture<'a> {
        Box::pin(resolve(
            name,
            |var| env::var(var).ok(),
            cfg!(target_os = "macos"),
            spawn_security,
        ))
    }
}

/// The one place the subprocess is launched, injected into [`resolve`] so the
/// outcomes that follow it are reachable from a test without a real Keychain.
/// The absolute path keeps a `security` earlier on PATH from supplying the secret.
fn spawn_security(service: &str) -> impl Future<Output = io::Result<Output>> + use<> {
    Command::new(SECURITY_BIN)
        .args(["find-generic-password", "-s", service, "-w"])
        .kill_on_drop(true)
        .output()
}

/// Test source that returns its constructor argument for every name.
#[cfg(test)]
pub(crate) struct StaticSecretSource(pub Option<Redacted>);

#[cfg(test)]
impl SecretSource for StaticSecretSource {
    fn fetch<'a>(&'a self, _name: &'a str) -> SecretFuture<'a> {
        let secret = self.0.clone();
        Box::pin(async move { secret })
    }
}

async fn resolve<F, C, Fut>(
    name: &str,
    env_reader: F,
    keychain_supported: bool,
    run_security: C,
) -> Option<Redacted>
where
    F: Fn(&str) -> Option<String>,
    C: FnOnce(&str) -> Fut,
    Fut: Future<Output = io::Result<Output>>,
{
    if let Some(secret) = env_reader(name).and_then(|value| Redacted::new(&value)) {
        return Some(secret);
    }
    if !keychain_supported || env_reader(NO_KEYCHAIN_VAR).is_some_and(|v| v.trim() == "1") {
        return None;
    }

    let output = timeout(KEYCHAIN_TIMEOUT, run_security(name))
        .await
        .inspect_err(|_| {
            warn!(
                secret = name,
                "keychain lookup timed out after {}s",
                KEYCHAIN_TIMEOUT.as_secs()
            )
        })
        .ok()?
        .inspect_err(|e| warn!(secret = name, error = %e, "keychain lookup could not start"))
        .ok()?;

    if !output.status.success() {
        // SEC: `security` stderr is dropped so nothing it echoes reaches the
        // log; only the exit code is reported.
        let code = output.status.code().unwrap_or(-1);
        if code == ITEM_NOT_FOUND {
            debug!(secret = name, code, "no keychain item; stderr withheld");
        } else {
            warn!(
                secret = name,
                code, "keychain lookup failed; stderr withheld"
            );
        }
        return None;
    }

    Redacted::new(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use std::future::{Ready, pending, ready};

    use super::*;

    const NAME: &str = "BRAVE_SEARCH_API_KEY";

    /// Build the `Output` a finished `security find-generic-password` would hand back.
    fn security_output(code: i32, stdout: &str, stderr: &str) -> io::Result<Output> {
        Ok(Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    /// A Keychain that holds `stdout` for every service. The tests that must not
    /// read the Keychain pass one too, and their assertions show its value was
    /// not used.
    fn keychain_holding(stdout: &'static str) -> impl FnOnce(&str) -> Ready<io::Result<Output>> {
        move |_| ready(security_output(0, stdout, ""))
    }

    fn env_of(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let env: HashMap<&str, &str> = pairs.iter().copied().collect();
        move |key| env.get(key).map(|value| (*value).to_owned())
    }

    /// [T-KC001] An env value wins, and the Keychain is never read.
    #[tokio::test]
    async fn env_value_is_used_without_reading_the_keychain() {
        let secret = resolve(
            NAME,
            env_of(&[(NAME, "from-env")]),
            true,
            keychain_holding("from-keychain\n"),
        )
        .await;

        assert_eq!(secret.as_ref().map(Redacted::expose), Some("from-env"));
    }

    /// [T-KC002] With the env var unset, the Keychain item named after the env
    /// var is read, and its trailing newline is trimmed.
    #[tokio::test]
    async fn unset_env_falls_back_to_the_keychain_item_of_the_same_name() {
        let asked = Cell::new(None);
        let secret = resolve(NAME, env_of(&[]), true, |service| {
            asked.set(Some(service.to_owned()));
            async { security_output(0, "from-keychain\n", "") }
        })
        .await;

        assert_eq!(secret.as_ref().map(Redacted::expose), Some("from-keychain"));
        assert_eq!(asked.take().as_deref(), Some(NAME));
    }

    /// [T-KC003] A whitespace-only env value counts as unset.
    #[tokio::test]
    async fn whitespace_only_env_falls_back_to_the_keychain() {
        let secret = resolve(
            NAME,
            env_of(&[(NAME, "   ")]),
            true,
            keychain_holding("from-keychain\n"),
        )
        .await;

        assert_eq!(secret.as_ref().map(Redacted::expose), Some("from-keychain"));
    }

    /// [T-KC004] A missing item (exit 44) and other non-zero exits yield no
    /// secret, and the stderr never reaches the log.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn failed_lookup_yields_no_secret_and_withholds_stderr() {
        for code in [44, 1] {
            let secret = resolve(NAME, env_of(&[]), true, |_| async move {
                security_output(code, "", "leaked_keychain_detail")
            })
            .await;
            assert!(secret.is_none(), "exit {code} must yield no secret");
        }

        assert!(!logs_contain("leaked_keychain_detail"));
        assert!(logs_contain("stderr withheld"));
    }

    /// [T-KC005] A lookup that cannot start yields no secret.
    #[tokio::test]
    async fn lookup_that_cannot_start_yields_no_secret() {
        let secret = resolve(NAME, env_of(&[]), true, |_| async {
            Err(io::Error::from(io::ErrorKind::NotFound))
        })
        .await;

        assert!(secret.is_none());
    }

    /// [T-KC006] A lookup that outlives the timeout yields no secret.
    ///
    /// `start_paused` advances the clock rather than waiting the real 5s.
    #[tokio::test(start_paused = true)]
    async fn lookup_past_the_timeout_yields_no_secret() {
        let secret = resolve(NAME, env_of(&[]), true, |_| pending()).await;

        assert!(secret.is_none());
    }

    /// [T-KC007] An empty stdout on success is no secret, not an empty one.
    #[tokio::test]
    async fn blank_keychain_value_yields_no_secret() {
        let secret = resolve(NAME, env_of(&[]), true, keychain_holding(" \n")).await;

        assert!(secret.is_none());
    }

    /// [T-KC008] `SCOUT_NO_KEYCHAIN=1` skips the Keychain.
    #[tokio::test]
    async fn no_keychain_var_skips_the_keychain() {
        let secret = resolve(
            NAME,
            env_of(&[(NO_KEYCHAIN_VAR, "1")]),
            true,
            keychain_holding("from-keychain\n"),
        )
        .await;

        assert!(secret.is_none());
    }

    /// [T-KC009] Where the Keychain is unsupported (every OS but macOS), it is
    /// never read.
    #[tokio::test]
    async fn unsupported_os_never_reads_the_keychain() {
        let secret = resolve(
            NAME,
            env_of(&[]),
            false,
            keychain_holding("from-keychain\n"),
        )
        .await;

        assert!(secret.is_none());
    }
}
