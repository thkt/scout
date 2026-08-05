use super::*;
use crate::fetch::TokioDnsResolver;
use std::collections::HashSet;
use std::env::temp_dir;
use std::fs::read_dir;
use std::path::{Path, PathBuf};

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

/// [T-F060] `fetch_with_cdp_with` honors the injected browser binary path.
///
/// Guards issue #227: the binary path is now an explicit parameter (no
/// process-global `OnceLock` cache), so a test can drive the launch path
/// without a real Chrome on the host. Injecting a path that is not an
/// executable makes `spawn_chromium_pgroup` fail at spawn time, surfacing
/// `BrowserError::ProcessFailed`. This exercises the seam (#191 DI pattern)
/// host-independently — no Chrome required, unlike the Chrome-gated test below.
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
    // Pin the failure to the chromium spawn (launch.rs), not the SSRF proxy or
    // profile-dir setup that also surface as `ProcessFailed` — otherwise the
    // test could pass without the injected path being the cause.
    let BrowserError::ProcessFailed(msg) = &err else {
        panic!("expected ProcessFailed for a nonexistent binary, got {err:?}");
    };
    assert!(
        msg.contains("spawn chromium"),
        "failure must come from spawning the injected binary, got {msg:?}"
    );
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
///   in the temp dir.
///
/// Ignored by default: hosts without a chromium binary would otherwise fail
/// `fetch_with_cdp` at `resolve_browser_binary`'s `BrowserError::NotFound`.
/// Run explicitly with:
/// `cargo nextest run --features js-rendering --run-ignored all --profile ci`
#[tokio::test]
#[ignore = "requires chromium"]
async fn t005_t006_cdp_renders_and_removes_profile_dir() {
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
