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
    // Explicit --js must launch the browser even for source-preserving media.
    let body = b"<catalog><item id=\"42\">fixture</item></catalog>";
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let Some((proxy, connections, _handle)) = common::spawn_mock_proxy_raw_response(&response)
    else {
        return;
    };
    let browser = FakeBrowser::new(false);
    let path = env::join_paths(
        once(
            browser
                .binary
                .parent()
                .expect("fixture browser directory")
                .to_path_buf(),
        )
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
        .args(["fetch", "--js", "http://scout-audit.example/catalog.xml"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut scout = OwnedScout(command.spawn().expect("start scout"));
    browser.wait_ready();
    common::assert_proxy_was_dialed(
        &connections,
        "browser cleanup",
        "fetch never reached the explicit XML fixture",
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
    assert_eq!(
        status.code(),
        Some(expected_code),
        "explicit --js must not return the XML source as a successful fetch"
    );
    if signal.is_some() {
        // No DevTools URL means signal drain must reach its 7s cutoff.
        assert!(
            started.elapsed() >= Duration::from_secs(7),
            "signal skipped bounded drain"
        );
    }
    browser.assert_clean();
}

/// [T-BGC005] Explicit --js on XML launches the browser; outer timeout returns
/// 124 and cleans owned PIDs/profile.
#[test]
fn outer_fetch_timeout_reaps_browser_descendant_and_profile() {
    run_cleanup(None, 124);
}

/// [T-BGC006] Explicit --js on XML launches the browser; both OS signals clean
/// owned PIDs/profile after the 7s drain cutoff.
#[test]
fn signal_drain_cutoff_reaps_browser_descendant_and_profile() {
    for (signal, code) in [(Signal::SIGINT, 130), (Signal::SIGTERM, 143)] {
        run_cleanup(Some(signal), code);
    }
}
