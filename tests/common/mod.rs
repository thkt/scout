//! Shared test scaffolding for integration test binaries under `tests/`.
//!
//! `tests/*.rs` binaries compile as separate crates from `src/`, so they
//! cannot see `src/test_support.rs`'s items even where that module marks them
//! `pub(crate)` — the `pub(crate)` visibility scopes to the `scout` crate
//! itself, not to a downstream integration-test crate that merely links
//! against it. That module also stays private on purpose (making it `pub`
//! would put a test-only server on the library's public API), so it cannot be
//! re-exported either. A helper `tests/` needs has to live here instead, even
//! where it mirrors something `src/test_support.rs` already does.
//!
//! `mod common;` recompiles this file separately per `tests/*.rs` binary, so
//! an item only one binary calls reads as dead code in every other binary
//! that also includes the module. `dead_code` is silenced at module level
//! rather than per item so a helper added for a future binary doesn't need
//! its own suppression.
#![allow(dead_code)]

use std::env;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Mirrors `guard_loopback_bind` / `bind_loopback` in `src/test_support.rs`:
/// one bind-failure decision for every loopback-binding helper here, so a
/// restricted environment produces the same outcome across both crates —
/// skip (`None` + warn), or a panic when `SCOUT_NETWORK_TESTS` asserts the
/// network must exist. Without this, a caller's
/// `let Some(..) = spawn_mock_proxy(..) else { return; }` turns a lost bind
/// into a file full of tests that pass while asserting nothing.
///
/// The warn goes to `eprintln!` rather than `tracing::warn!` as the mirrored
/// original does: these `tests/*.rs` binaries install no tracing subscriber,
/// so a `tracing` record would be dropped instead of reaching the operator
/// deciding whether the skip was expected.
///
/// `bind_result` and `force` arrive as parameters, matching
/// `try_spawn_with_bind` in the original, so a test can drive the skip-vs-panic
/// decision without an environment that actually refuses to bind.
fn guard_loopback_bind(
    test_name: &str,
    bind_result: io::Result<TcpListener>,
    force: bool,
) -> Option<TcpListener> {
    match bind_result {
        Ok(listener) => Some(listener),
        Err(e) => {
            if force {
                panic!(
                    "[network-guard] {test_name}: bind failed and SCOUT_NETWORK_TESTS is set: {e}"
                );
            }
            eprintln!("[network-guard] {test_name}: loopback bind unavailable, early return");
            None
        }
    }
}

/// Bind a loopback listener under the guard policy above.
fn bind_loopback(test_name: &str) -> Option<TcpListener> {
    let force = env::var("SCOUT_NETWORK_TESTS").is_ok();
    guard_loopback_bind(test_name, TcpListener::bind("127.0.0.1:0"), force)
}

/// Launches the built `scout` binary. Shared by every `tests/*.rs` binary so
/// the lookup rule (`CARGO_BIN_EXE_scout`, set by Cargo for integration
/// tests) lives in one place instead of once per test binary.
pub fn scout() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scout"))
}

/// Every `--json` error test needs the envelope line before it can assert
/// anything, so the rule for finding it — scan stderr line by line for the
/// first line that parses as JSON, because `init_tracing`'s WARN/INFO lines
/// share stderr with the envelope — lives here once.
pub fn parse_envelope(output: &Output, context: &str) -> serde_json::Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| {
            panic!("{context} stderr should contain a JSON envelope line, got:\n{stderr}")
        });
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("{context} envelope must be valid JSON ({e}): {line}"))
}

/// Forward proxy mock that loops `accept` instead of serving one connection,
/// so a client that re-dials (retry after failure, connection churn, ...)
/// keeps getting served instead of hitting a closed listener.
///
/// Mirrors `spawn_forward_proxy` in `src/test_support.rs` (bind loopback,
/// hand the connection to a spawned thread, write a canned response), with
/// two deviations the plan calls for: the single `listener.accept()` call
/// becomes a loop, and the fixed 200/no-delay/text-body response becomes the
/// caller-supplied `status` / `delay` / `body` below.
///
/// - `status`: the numeric status line code the response opens with. The
///   reason phrase is always written as `OK` regardless of `status`; a caller
///   asserting on the reason phrase itself needs a different helper.
/// - `delay`: slept through after the request is drained and before the
///   response is written.
/// - `body`: written verbatim after a `Content-Length` header sized to it —
///   unlike `spawn_forward_proxy`'s `&str` body, a non-UTF-8 payload survives.
///
/// Returns `(base_url, connection_count, join_handle)`, or `None` when
/// `bind_loopback` above skips for an unavailable loopback bind, matching
/// `spawn_forward_proxy`'s early return for restricted environments.
/// `connection_count` increments once per accepted connection, so a test that
/// drives several requests (a proxy retry, several keep-alive-less calls, ...)
/// through the returned base URL can assert how many dials actually reached
/// the proxy.
///
/// The accept loop has no exit condition other than a fatal `accept` error
/// (e.g. the OS closing the socket), so `join_handle` is not for a caller to
/// `.join()` and wait on, which would hang until the test process exits.
pub fn spawn_mock_proxy(
    status: u16,
    delay: Duration,
    body: &[u8],
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
    let listener = bind_loopback("spawn_mock_proxy")?;
    let addr = listener.local_addr().ok()?;
    let connection_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connection_count);
    let body = body.to_vec();
    let handle = thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                // Fatal accept error (e.g. listener torn down): stop looping
                // rather than spin.
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            // Drain the request so the write below is the response, not
            // racing an unread request buffer, matching
            // `spawn_forward_proxy`'s rationale.
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let mut response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response);
            // Dropping the stream per iteration is what forces a
            // keep-alive-unaware client to re-dial for its next request and
            // exercise the accept loop again.
        }
    });
    Some((format!("http://{addr}"), connection_count, handle))
}

/// One-shot forward proxy mock that answers the single connection it accepts
/// with `raw_response` written verbatim — no status line, no
/// `Content-Length` framing added by this helper, unlike `spawn_mock_proxy`.
/// Exercises the non-HTTP-bytes response path `spawn_mock_proxy` cannot
/// reach, since that helper always writes a well-formed `HTTP/1.1 ...`
/// status line ahead of `body`.
///
/// Single-shot rather than looping (unlike `spawn_mock_proxy`): the `fetch`
/// path this proves calls nothing from `src/retry.rs` at all, so a caller of
/// this helper never dials the mock proxy more than once.
///
/// Returns `(base_url, connection_count, join_handle)`, or `None` when
/// `bind_loopback` above skips for an unavailable loopback bind, matching
/// `spawn_mock_proxy`.
pub fn spawn_mock_proxy_raw_response(
    raw_response: &'static [u8],
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
    let listener = bind_loopback("spawn_mock_proxy_raw_response")?;
    let addr = listener.local_addr().ok()?;
    let connection_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connection_count);
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            // Loopback bind unavailable races aside, a failed accept here is a
            // test-environment fault; return rather than hang the caller.
            return;
        };
        counter.fetch_add(1, Ordering::SeqCst);
        // Drain the request so the write below is the response, not racing an
        // unread request buffer, matching `spawn_mock_proxy`'s rationale.
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(raw_response);
    });
    Some((format!("http://{addr}"), connection_count, handle))
}

/// `PATH` is restored so the OS proxy lookup still resolves, and
/// `LLVM_PROFILE_FILE` survives the clear because an instrumented child that
/// loses it writes no `.profraw`, dropping every line it drives from the
/// coverage report. Everything else from the invoking shell stays cleared so
/// it cannot leak into the contract under test.
///
/// `HOME` is deliberately not restored: neither `src/` nor any crate in
/// `Cargo.lock` reads it. The macOS proxy lookup goes through
/// `system-configuration` and the Linux one reads proxy env vars only.
pub fn scout_with_clean_env() -> Command {
    scout_with_env(
        &env::var("PATH").unwrap_or_default(),
        env::var("LLVM_PROFILE_FILE").ok().as_deref(),
    )
}

/// Testable core `scout_with_clean_env` wraps. The values arrive as
/// parameters rather than being read here because `unsafe_code = "forbid"`
/// (Cargo.toml) blocks a test from mutating the real process env, which would
/// otherwise be the only way to reach the coverage-output branch.
pub fn scout_with_env(path: &str, coverage_output: Option<&str>) -> Command {
    let mut cmd = scout();
    cmd.env_clear().env("PATH", path);
    if let Some(profile) = coverage_output {
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
    cmd
}

/// Assert the mock proxy was dialed at least once, so a run that reached its
/// expected outcome without ever leaving scout (a DNS or SSRF short-circuit
/// landing on the same result by coincidence) fails instead of passing as a
/// false positive. `consequence` stays a parameter so the panic names what
/// the caller's own assertions rest on, which differs per call site.
pub fn assert_proxy_was_dialed(connection_count: &AtomicUsize, context: &str, consequence: &str) {
    assert!(
        connection_count.load(Ordering::SeqCst) >= 1,
        "{context}: no connection reached the mock proxy, so {consequence}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn bind_refused() -> io::Result<TcpListener> {
        Err(io::Error::other("bind refused"))
    }

    // T-C027: forced_run_panics_when_loopback_bind_fails
    //
    // The branch the callers cannot reach on their own: every `spawn_mock_proxy`
    // caller turns `None` into an early return, so without this the guard could
    // stop panicking and the suite would still be green while asserting nothing.
    #[test]
    #[should_panic(expected = "SCOUT_NETWORK_TESTS is set")]
    fn forced_run_panics_when_loopback_bind_fails() {
        guard_loopback_bind("forced_run", bind_refused(), true);
    }

    // T-C028: unforced_run_skips_when_loopback_bind_fails
    #[test]
    fn unforced_run_skips_when_loopback_bind_fails() {
        assert!(guard_loopback_bind("unforced_run", bind_refused(), false).is_none());
    }

    // T-C035: command_sets_llvm_profile_file_to_same_value_when_coverage_output_is_given
    #[test]
    fn command_sets_llvm_profile_file_to_same_value_when_coverage_output_is_given() {
        let cmd = scout_with_env("/usr/bin", Some("/tmp/scout-123.profraw"));

        let llvm_profile_file = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("LLVM_PROFILE_FILE"))
            .and_then(|(_, value)| value);

        assert_eq!(
            llvm_profile_file,
            Some(OsStr::new("/tmp/scout-123.profraw")),
            "LLVM_PROFILE_FILE should carry the same coverage output value the caller passed"
        );
    }

    // T-C036: command_does_not_set_llvm_profile_file_when_coverage_output_is_absent
    #[test]
    fn command_does_not_set_llvm_profile_file_when_coverage_output_is_absent() {
        let cmd = scout_with_env("/usr/bin", None);

        let has_llvm_profile_file = cmd
            .get_envs()
            .any(|(key, _)| key == OsStr::new("LLVM_PROFILE_FILE"));

        assert!(
            !has_llvm_profile_file,
            "LLVM_PROFILE_FILE should not be set on the Command when no coverage output is given"
        );
    }

    // T-C037: zero_connections_panics_with_the_given_consequence
    #[test]
    #[should_panic(expected = "stdout asserted below did not come from the fixture")]
    fn zero_connections_panics_with_the_given_consequence() {
        let connection_count = AtomicUsize::new(0);
        assert_proxy_was_dialed(
            &connection_count,
            "some context",
            "stdout asserted below did not come from the fixture",
        );
    }
}
