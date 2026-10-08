use super::*;
use crate::envelope::CommandOutput;
use crate::fetch::TokioDnsResolver;
use crate::signals::InterruptSignal;
use crate::test_support::browser_fixture::FakeBrowser;
use crate::{Outcome, drive};
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokio::time::{Instant, advance, pause, sleep};

/// [T-BGC004] Injected reap command completes signal drain and cleans its group/profile.
#[tokio::test]
async fn signal_drain_completion_cleans_browser_group_and_profile() {
    for (signal, code) in [
        (InterruptSignal::Sigint, 130),
        (InterruptSignal::Sigterm, 143),
    ] {
        let browser = FakeBrowser::new(false);
        let (cancel, mut rx) = watch::channel(false);
        let completed = Cell::new(false);
        let command = async {
            let (mut process, _reader) =
                spawn_chromium_pgroup(&browser.binary, 0).expect("fake launch");
            rx.wait_for(|&cancelled| cancelled)
                .await
                .expect("cancel notification");
            process.reap().await;
            completed.set(true);
            Ok(CommandOutput::ok(String::new(), serde_json::json!({})))
        };
        let interrupt = async {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !browser.ready() {
                assert!(
                    Instant::now() < deadline,
                    "browser launch never became ready"
                );
                sleep(Duration::from_millis(10)).await;
            }
            browser.wait_ready();
            signal
        };
        let outcome = drive(command, interrupt, &cancel).await;
        assert!(matches!(outcome, Outcome::Interrupted(sig) if sig.exit_code() == code));
        assert!(
            completed.get(),
            "command must finish reap within drain, not be cut off"
        );
        browser.assert_clean();
    }
}

/// [T-BGC002] DevTools URL timeout cleans the owned group and profile.
#[tokio::test]
async fn internal_devtools_timeout_cleans_browser_group_and_profile() {
    let browser = FakeBrowser::new(false);
    let (cancel, _rx) = watch::channel(false);
    let url = ValidatedUrl::for_test("https://example.com");
    let fetch = fetch_with_cdp_with(&url, &browser.binary, Arc::new(TokioDnsResolver), &cancel);
    tokio::pin!(fetch);
    tokio::select! {
        result = &mut fetch => panic!("browser must wait for DevTools: {result:?}"),
        () = async {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !browser.ready() {
                assert!(Instant::now() < deadline, "browser launch never became ready");
                sleep(Duration::from_millis(10)).await;
            }
        } => {}
    }
    browser.wait_ready();
    // Only virtualize time after OS resources are demonstrably ready.
    pause();
    advance(CDP_TIMEOUT).await;
    assert!(matches!(fetch.await, Err(BrowserError::TimedOut)));
    browser.assert_clean();
}

/// [T-BGC003] Stderr EOF cleans the owned group and profile.
#[tokio::test]
async fn devtools_eof_cleans_browser_group_and_profile() {
    let browser = FakeBrowser::new(true);
    let (cancel, _rx) = watch::channel(false);
    let error = fetch_with_cdp_with(
        &ValidatedUrl::for_test("https://example.com"),
        &browser.binary,
        Arc::new(TokioDnsResolver),
        &cancel,
    )
    .await
    .expect_err("no DevTools URL");
    assert!(matches!(error, BrowserError::ProcessFailed(_)));
    browser.assert_clean();
}

/// [T-F060] A nonexistent injected browser path reports a chromium spawn failure.
#[tokio::test]
async fn t007_fetch_with_cdp_with_injects_browser_path() {
    let (cancel, _) = watch::channel(false);
    let bogus = Path::new("/nonexistent/scout-test-no-such-chromium");
    let err = fetch_with_cdp_with(
        &ValidatedUrl::for_test("https://example.com"),
        bogus,
        Arc::new(TokioDnsResolver),
        &cancel,
    )
    .await
    .expect_err("spawning a nonexistent browser binary must fail");
    // Distinguish spawn failure from proxy/profile setup errors.
    let BrowserError::ProcessFailed(msg) = &err else {
        panic!("expected ProcessFailed for a nonexistent binary, got {err:?}");
    };
    assert!(
        msg.contains("spawn chromium"),
        "failure must come from spawning the injected binary, got {msg:?}"
    );
}

/// [T-F051] Rendered content; [T-F057] deletion of this launch's recorded profile.
/// Requires real chromium; recording only this profile avoids concurrent fixtures.
#[tokio::test]
#[ignore = "requires chromium"]
async fn t005_t006_cdp_renders_and_removes_profile_dir() {
    let recording = tempdir().expect("profile recorder");
    let profile_record = recording.path().join("profile");
    let wrapper = recording.path().join("chromium");
    let browser = resolve_browser_binary().expect("requires chromium");
    let browser = browser.to_string_lossy().replace('\'', "'\\''");
    let record = profile_record.to_string_lossy().replace('\'', "'\\''");
    fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        --user-data-dir=*) printf '%s' "${{arg#--user-data-dir=}}" > '{record}';;
    esac
done
exec '{browser}' "$@"
"#
        ),
    )
    .expect("recording wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("executable wrapper");
    let (cancel, _) = watch::channel(false);
    let html = fetch_with_cdp_with(
        &ValidatedUrl::for_test("https://example.com"),
        &wrapper,
        Arc::new(TokioDnsResolver),
        &cancel,
    )
    .await
    .expect("fetch_with_cdp should succeed for public URL");
    // Chrome's own error page can include the requested hostname. The hostname
    // alone therefore cannot prove the document actually rendered.
    assert!(
        html.contains("Example Domain"),
        "rendered HTML should contain page content, got {} bytes",
        html.len()
    );
    let profile =
        PathBuf::from(fs::read_to_string(profile_record).expect("recorded Chrome profile"));
    assert!(
        !profile.exists(),
        "fetch left its chromium profile behind: {profile:?}"
    );
}

/// [T-BGC011] A DevTools endpoint that cannot connect still requires group
/// cleanup; discovery success alone must not release ownership.
#[tokio::test]
async fn devtools_connect_failure_cleans_browser_group_and_profile() {
    let browser =
        FakeBrowser::with_devtools(false, Some("ws://127.0.0.1:0/devtools/browser/fixture"));
    let (cancel, _rx) = watch::channel(false);
    let error = fetch_with_cdp_with(
        &ValidatedUrl::for_test("https://example.com"),
        &browser.binary,
        Arc::new(TokioDnsResolver),
        &cancel,
    )
    .await
    .expect_err("port zero cannot serve a DevTools endpoint");
    assert!(
        matches!(error, BrowserError::ProcessFailed(ref message) if message.contains("browser connect:"))
    );
    browser.assert_clean();
}
