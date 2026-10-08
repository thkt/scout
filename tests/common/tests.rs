use super::common::{
    assert_proxy_was_dialed, guard_loopback_bind, scout_with_env, set_coverage_profile,
};
use std::ffi::OsStr;
use std::io;
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;

fn bind_refused() -> io::Result<TcpListener> {
    Err(io::Error::other("bind refused"))
}

// T-C047: Forced bind failure panics; callers returning on None could otherwise skip assertions.
#[test]
#[should_panic(expected = "SCOUT_NETWORK_TESTS is set")]
fn forced_run_panics_when_loopback_bind_fails() {
    guard_loopback_bind("forced_run", bind_refused(), true);
}

// T-C028
#[test]
fn unforced_run_skips_when_loopback_bind_fails() {
    assert!(guard_loopback_bind("unforced_run", bind_refused(), false).is_none());
}

// T-C035
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

// T-C036
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

// T-C037
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
