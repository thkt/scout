//! Shared integration-test scaffolding. These binaries cannot use the private
//! `src/test_support.rs` module without exposing test helpers as public API.
//!
//! `mod common` recompiles this file for each binary, so helpers used only by
//! other binaries need the module-level dead-code suppression.
#![allow(dead_code)]

use std::env;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Keep unavailable-loopback policy consistent with `src/test_support.rs`: warn
/// and skip, or panic when SCOUT_NETWORK_TESTS requires actual assertions.
/// Use stderr because integration tests install no tracing subscriber.
/// Explicit parameters allow testing the guard without a real bind failure.
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

/// After binding, address lookup failure must panic even under
/// SCOUT_NETWORK_TESTS; it is not an unavailable-bind skip.
fn addr_and_counter(listener: &TcpListener) -> (SocketAddr, Arc<AtomicUsize>) {
    let addr = listener
        .local_addr()
        .expect("a bound listener reports its address");
    (addr, Arc::new(AtomicUsize::new(0)))
}

pub(crate) fn scout() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scout"))
}

/// Find the JSON envelope among tracing lines sharing stderr.
pub(crate) fn parse_envelope(output: &Output, context: &str) -> serde_json::Value {
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

/// Serve caller-supplied bytes after `delay`, counting each accepted connection.
/// Keep accepting so retries can re-dial; the reason phrase is always OK.
/// Returns None under the shared bind-skip policy. The loop has no normal exit,
/// so joining its handle would hang until process exit.
pub(crate) fn spawn_mock_proxy(
    status: u16,
    delay: Duration,
    body: &[u8],
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
    let listener = bind_loopback("spawn_mock_proxy")?;
    let (addr, connection_count) = addr_and_counter(&listener);
    let counter = Arc::clone(&connection_count);
    let body = body.to_vec();
    let handle = thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            // Drain the request before replying to avoid racing unread request data.
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
            // Closing each stream forces subsequent requests to re-dial.
        }
    });
    Some((format!("http://{addr}"), connection_count, handle))
}

/// One-shot proxy writing response bytes verbatim, including framing or
/// malformed status lines. Returns None under the shared bind-skip policy.
/// Join only after a request; callers must not expect retries to be served.
pub(crate) fn spawn_mock_proxy_raw_response(
    raw_response: &[u8],
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
    let listener = bind_loopback("spawn_mock_proxy_raw_response")?;
    let (addr, connection_count) = addr_and_counter(&listener);
    let counter = Arc::clone(&connection_count);
    let raw_response = raw_response.to_vec();
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        counter.fetch_add(1, Ordering::SeqCst);
        // Drain the request before replying.
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(&raw_response);
    });
    Some((format!("http://{addr}"), connection_count, handle))
}

/// Restore PATH for OS proxy lookup; clear other shell state except coverage
/// output. HOME is not required by scout or its current dependencies.
pub(crate) fn scout_with_clean_env() -> Command {
    let mut cmd = scout_with_env(&env::var("PATH").unwrap_or_default());
    forward_coverage_profile(&mut cmd);
    cmd
}

/// Forward coverage output across env_clear so instrumented child execution
/// is included. Also used by tests that do not restore PATH.
pub(crate) fn forward_coverage_profile(cmd: &mut Command) {
    set_coverage_profile(cmd, env::var("LLVM_PROFILE_FILE").ok().as_deref());
}

/// Take the value explicitly: unsafe_code forbids process-env mutation in tests.
pub(crate) fn set_coverage_profile(cmd: &mut Command, profile: Option<&str>) {
    if let Some(profile) = profile {
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
}

/// Take PATH explicitly so tests need no forbidden process-env mutation.
pub(crate) fn scout_with_env(path: &str) -> Command {
    let mut cmd = scout();
    cmd.env_clear().env("PATH", path);
    cmd
}

/// Require a proxy dial so a DNS/SSRF short-circuit cannot satisfy an assertion
/// that is supposed to exercise HTTP response handling.
pub(crate) fn assert_proxy_was_dialed(
    connection_count: &AtomicUsize,
    context: &str,
    consequence: &str,
) {
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

    // T-C047: forced_run_panics_when_loopback_bind_fails
    // Callers early-return on None; losing the panic would silently skip assertions.
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
        let mut cmd = scout_with_env("/usr/bin");
        set_coverage_profile(&mut cmd, Some("/tmp/scout-123.profraw"));

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
        let mut cmd = scout_with_env("/usr/bin");
        set_coverage_profile(&mut cmd, None);

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
