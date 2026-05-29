use super::*;
use crate::fetch::TokioDnsResolver;

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
