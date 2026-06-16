use super::*;
use crate::fetch::TokioDnsResolver;
use std::collections::HashSet;
use std::env::temp_dir;
use std::fs::read_dir;
use std::path::PathBuf;
use tokio::sync::Mutex;

/// Serializes the two Chrome-gated tests. t006 snapshots `scout-chromium-*`
/// dirs before/after its fetch; a t005 fetch running concurrently would leave
/// its in-flight profile dir in t006's `after` set and trip a false residual.
/// Both tests run real ~2s fetches, so the lock is held across the await — an
/// async-aware `Mutex` keeps that sound under tokio.
static CDP_TEST_LOCK: Mutex<()> = Mutex::const_new(());

fn chrome_available() -> bool {
    resolve_browser_binary().is_ok()
}

/// [T-F051] t005_cdp_renders_public_url
#[tokio::test]
async fn t005_cdp_renders_public_url() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let _serial = CDP_TEST_LOCK.lock().await;
    let (cancel, _) = watch::channel(false);
    let html = fetch_with_cdp(
        &ValidatedUrl::for_test("https://example.com"),
        Arc::new(TokioDnsResolver),
        &cancel,
    )
    .await
    .expect("fetch_with_cdp should succeed for public URL");
    assert!(
        html.contains("Example Domain") || html.contains("example"),
        "rendered HTML should contain page content, got {} bytes",
        html.len()
    );
}

/// Collect the set of `<temp>/scout-chromium-*` profile dirs currently on disk.
fn chromium_profile_dirs() -> HashSet<PathBuf> {
    let Ok(entries) = read_dir(temp_dir()) else {
        return HashSet::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("scout-chromium-"))
        })
        .collect()
}

/// [T-F057] t006_cdp_removes_profile_dir_after_fetch
///
/// Verifies issue #198: the chromium `--user-data-dir` created per `--js` fetch
/// is deleted once the fetch completes, leaving no new `scout-chromium-*` dir in
/// the temp dir. Snapshots the dir set before/after the fetch and asserts the
/// difference is empty. `CDP_TEST_LOCK` serializes this against t005 so a
/// concurrent fetch's in-flight profile dir cannot land in the `after` set.
#[tokio::test]
async fn t006_cdp_removes_profile_dir_after_fetch() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let _serial = CDP_TEST_LOCK.lock().await;
    let before = chromium_profile_dirs();
    let (cancel, _) = watch::channel(false);
    fetch_with_cdp(
        &ValidatedUrl::for_test("https://example.com"),
        Arc::new(TokioDnsResolver),
        &cancel,
    )
    .await
    .expect("fetch_with_cdp should succeed for public URL");
    let residual: Vec<_> = chromium_profile_dirs()
        .difference(&before)
        .cloned()
        .collect();
    assert!(
        residual.is_empty(),
        "fetch left chromium profile dir(s) behind: {residual:?}"
    );
}
