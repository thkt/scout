//! Offline CLI acceptance checks for browser cleanup after Future destruction.
#[path = "../../src/test_support/browser_fixture.rs"]
mod browser_fixture;
use super::common;

use std::env;
use std::iter::once;
use std::process::Stdio;
use std::time::{Duration, Instant};

use browser_fixture::{FakeBrowser, OwnedScout, wait_scout};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

fn run_cleanup(signal: Option<Signal>, expected_code: i32) {
    let Some((proxy, connections, _handle)) =
        common::spawn_mock_proxy(200, Duration::ZERO, b"<html><body>fixture</body></html>")
    else {
        return;
    };
    let browser = FakeBrowser::new(false);
    let path = env::join_paths(
        once(browser.path().to_path_buf())
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .expect("fixture PATH");
    let mut command = common::scout_with_clean_env();
    command
        .env("PATH", path)
        .env("TMPDIR", browser.path())
        .env("HTTP_PROXY", proxy)
        .env(
            "SCOUT_FETCH_TIMEOUT_SECS",
            if signal.is_some() { "600" } else { "2" },
        )
        .args(["fetch", "--js", "http://scout-audit.example/html"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut scout = OwnedScout(command.spawn().expect("start scout"));
    browser.wait_ready();
    common::assert_proxy_was_dialed(
        &connections,
        "browser cleanup",
        "fetch never reached HTML fixture",
    );
    let started = Instant::now();
    if let Some(signal) = signal {
        kill(
            Pid::from_raw(i32::try_from(scout.0.id()).expect("scout PID")),
            signal,
        )
        .expect("send interrupt to scout");
    }
    let status = wait_scout(&mut scout.0);
    assert_eq!(status.code(), Some(expected_code));
    if signal.is_some() {
        // A silent fake browser is still awaiting DevTools, so cancellation
        // cannot complete CDP's close path: this must exhaust the 7s drain.
        assert!(
            started.elapsed() >= Duration::from_secs(7),
            "signal skipped bounded drain"
        );
    }
    browser.assert_clean();
}

/// [T-BGC005] Actual Scout::fetch outer timeout, rather than a local wrapper.
#[test]
fn outer_fetch_timeout_reaps_browser_descendant_and_profile() {
    run_cleanup(None, 124);
}

/// [T-BGC006] Both OS signals retain their codes after drain cuts CDP off.
#[test]
fn signal_drain_cutoff_reaps_browser_descendant_and_profile() {
    for (signal, code) in [(Signal::SIGINT, 130), (Signal::SIGTERM, 143)] {
        run_cleanup(Some(signal), code);
    }
}
