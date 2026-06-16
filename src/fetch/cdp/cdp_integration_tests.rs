use super::*;
use crate::fetch::TokioDnsResolver;
use std::collections::HashSet;
use std::env::temp_dir;
use std::fs::read_dir;
use std::path::PathBuf;

fn chrome_available() -> bool {
    resolve_browser_binary().is_ok()
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

/// [T-F051] cdp renders public url + [T-F057] cdp removes profile dir after fetch
///
/// Both Chrome-gated checks share one real ~2s fetch so they run sequentially in
/// a single process. CI runs each `#[test]` in its own nextest process, so a
/// process-internal lock cannot serialize two separate test fns; the temp-dir
/// snapshot in the #198 check would otherwise capture a concurrent fetch's
/// in-flight `scout-chromium-*` dir and trip a false residual. Merging the two
/// removes the cross-test race entirely.
///
/// - T-F051: the rendered HTML contains example.com page content.
/// - T-F057 (issue #198): the chromium `--user-data-dir` created for the fetch
///   is deleted once the fetch completes, leaving no new `scout-chromium-*` dir
///   in the temp dir. Snapshots the dir set before/after and asserts the
///   difference is empty.
#[tokio::test]
async fn t005_t006_cdp_renders_and_removes_profile_dir() {
    if !chrome_available() {
        eprintln!("SKIP: Chrome not found");
        return;
    }
    let before = chromium_profile_dirs();
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
    let residual: Vec<_> = chromium_profile_dirs()
        .difference(&before)
        .cloned()
        .collect();
    assert!(
        residual.is_empty(),
        "fetch left chromium profile dir(s) behind: {residual:?}"
    );
}
