//! Browser binary discovery and Chrome process lifecycle for CDP rendering.

#[cfg(feature = "js-rendering")]
use nix::unistd::Pid;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "js-rendering")]
use std::time::Duration;
#[cfg(feature = "js-rendering")]
use tempfile::{Builder, TempDir};
#[cfg(feature = "js-rendering")]
use tokio::io::{AsyncBufRead, BufReader};
#[cfg(feature = "js-rendering")]
use tokio::process::{Child as TokioChild, ChildStderr, Command as TokioCommand};
#[cfg(feature = "js-rendering")]
use tokio::time::{sleep, timeout};
use tracing::warn;

use super::BrowserError;
use crate::fetch::ssrf::{self, RedactedLogUrl};

/// Discover the chromium/Chrome binary by probing `PATH` then known install
/// locations. Called once per `--js` fetch (issue #227 removed the prior
/// process-global `OnceLock` cache, which broke test isolation and pinned the
/// first result for the process lifetime); the few `which` probes cost ~1-5 ms,
/// negligible against the ~2 s chromium render that follows.
#[cfg(feature = "js-rendering")]
pub(super) fn resolve_browser_binary() -> Result<PathBuf, BrowserError> {
    // Compile-time `#[cfg]` (not runtime `cfg!`) so each platform's table is the
    // only one compiled: the other OS's lines never enter `cargo llvm-cov`, so
    // the diff-coverage gate does not flag the macOS table as uncovered on the
    // Linux CI runner (where it is unreachable). Mirrors transport.rs's exclusion
    // of OS-I/O that the offline suite cannot exercise.
    #[cfg(target_os = "macos")]
    let path_commands: &[&str] = &["chromium"];
    #[cfg(not(target_os = "macos"))]
    let path_commands: &[&str] = &[
        "google-chrome-stable",
        "google-chrome",
        "chromium-browser",
        "chromium",
    ];

    #[cfg(target_os = "macos")]
    let known_paths: &[&Path] = &[
        Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        Path::new("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ];
    #[cfg(not(target_os = "macos"))]
    let known_paths: &[&Path] = &[];

    resolve_browser_binary_from(path_commands, known_paths)
}

/// See ADR-0021 (CDP Chromium Launch Egress Flags) for rationale.
///
/// The proxy flags route every chromium TCP egress through scout's loopback
/// SOCKS5 proxy so connect-time IPs are re-validated (issue #201):
/// - `--proxy-server=socks5://127.0.0.1:{proxy_port}`: SOCKS5 (not v4) so the
///   target host is resolved by the proxy, not chromium, closing DNS rebinding.
/// - `--proxy-bypass-list=<-loopback>`: subtracts chromium's implicit DIRECT
///   bypass for loopback AND link-local (169.254/16, the IMDS range), forcing
///   even those through the proxy.
/// - `--disable-quic`: QUIC/HTTP3 egresses over UDP, which a TCP SOCKS5 proxy
///   cannot intercept; disabling it keeps all egress on the proxied TCP path.
#[cfg(feature = "js-rendering")]
fn build_launch_args(proxy_port: u16) -> Vec<String> {
    vec![
        "--headless=new".to_owned(),
        "--disable-webrtc".to_owned(),
        "--disable-background-networking".to_owned(),
        "--disable-features=DnsOverHttps".to_owned(),
        "--disable-domain-reliability".to_owned(),
        "--no-pings".to_owned(),
        "--disable-extensions".to_owned(),
        "--no-first-run".to_owned(),
        "--disable-default-apps".to_owned(),
        format!("--proxy-server=socks5://127.0.0.1:{proxy_port}"),
        "--proxy-bypass-list=<-loopback>".to_owned(),
        "--disable-quic".to_owned(),
    ]
}

/// SSRF check for a browser-initiated subrequest URL (CDP `Fetch.RequestPaused`).
///
/// Scheme handling rationale:
/// - `http`/`https`: passed directly to `ssrf::ssrf_check`
/// - `ws`/`wss`: WebSocket can reach internal services; rewritten to http(s) for SSRF allowlist check
/// - `data:`/`about:`/`chrome:`/`blob:`: synthetic browser schemes with no external egress, allowed without SSRF check
/// - Unrecognized scheme: blocked (warn + return false) because the scheme cannot be classified
///
/// See ADR-0001 for the SSRF defense architecture.
#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
pub(crate) async fn check_browser_request(url: &str, resolver: &dyn ssrf::DnsResolver) -> bool {
    let check_url = if url.starts_with("http://") || url.starts_with("https://") {
        Cow::Borrowed(url)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        Cow::Owned(format!("http://{rest}"))
    } else if let Some(rest) = url.strip_prefix("wss://") {
        Cow::Owned(format!("https://{rest}"))
    } else if url.starts_with("data:")
        || url.starts_with("about:")
        || url.starts_with("chrome:")
        || url.starts_with("blob:")
    {
        return true;
    } else {
        warn!(url = %RedactedLogUrl(url), "SSRF: blocked browser subrequest with unrecognized scheme");
        return false;
    };
    // Direct: the CDP path routes chromium egress through scout's loopback
    // SOCKS5 proxy (ADR-0021), but this subrequest allowlist check runs in
    // scout's own process, which resolves directly — so the DNS pre-check
    // applies as in a direct fetch.
    ssrf::ssrf_check(&check_url, resolver, &ssrf::EgressMode::Direct)
        .await
        .is_ok()
}

/// Grace period between SIGTERM and SIGKILL when reaping the chromium pgroup.
/// 50 ms is enough for chromium subprocess (Helper Renderer, GPU, Network) to
/// observe the signal after `browser.close()` already drove the graceful path.
#[cfg(feature = "js-rendering")]
const PGROUP_SIGTERM_GRACE: Duration = Duration::from_millis(50);

#[cfg(feature = "js-rendering")]
pub(super) fn spawn_chromium_pgroup(
    browser_path: &Path,
    proxy_port: u16,
) -> Result<(TokioChild, Pid, BufReader<ChildStderr>, TempDir), BrowserError> {
    use std::process::Stdio;

    // `TempDir` gives each --js fetch a unique profile dir (random suffix avoids
    // chromium's `SingletonLock` failure when two scout processes run --js
    // concurrently) and deletes it on `Drop`. The caller must hold the returned
    // guard until after `reap_pgroup`, because chromium keeps writing profile
    // state during graceful shutdown (issue #198).
    let user_data_dir = Builder::new()
        .prefix("scout-chromium-")
        .tempdir()
        .map_err(|e| BrowserError::ProcessFailed(format!("create chromium profile dir: {e}")))?;
    let mut cmd = TokioCommand::new(browser_path);
    cmd.arg("--remote-debugging-port=0")
        .arg(format!(
            "--user-data-dir={}",
            user_data_dir.path().display()
        ))
        .args(build_launch_args(proxy_port))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| BrowserError::ProcessFailed(format!("spawn chromium: {e}")))?;
    let pid = child
        .id()
        .ok_or_else(|| BrowserError::ProcessFailed("chromium pid unavailable".into()))?;
    let pgid = Pid::from_raw(
        i32::try_from(pid)
            .map_err(|_| BrowserError::ProcessFailed("chromium pid out of i32 range".into()))?,
    );

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| BrowserError::ProcessFailed("chromium stderr missing".into()))?;
    Ok((child, pgid, BufReader::new(stderr), user_data_dir))
}

/// Read chromium stderr line-by-line until `DevTools listening on ws://...`.
///
/// Mirrors chromiumoxide 0.9's `ws_url_from_output` — the marker has been
/// stable in Chrome/Chromium for years. Generic over `AsyncBufRead` so unit
/// tests can drive it with an in-memory cursor.
#[cfg(feature = "js-rendering")]
pub(super) async fn parse_ws_url_from_lines<R>(reader: R) -> Result<String, BrowserError>
where
    R: AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut lines = reader.lines();
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| BrowserError::ProcessFailed(format!("stderr read: {e}")))?;
        let Some(line) = line else {
            return Err(BrowserError::ProcessFailed(
                "chromium exited before announcing DevTools URL".into(),
            ));
        };
        if let Some((_, ws)) = line.rsplit_once("listening on ")
            && ws.starts_with("ws")
            && ws.contains("devtools/browser")
        {
            return Ok(ws.trim().to_owned());
        }
    }
}

/// Send SIGTERM to the pgroup, wait a short grace, then SIGKILL.
///
/// `ESRCH` from the first killpg means the group already exited (the common
/// case after a successful `browser.close()`); skip the grace + SIGKILL in
/// that branch to avoid a 50 ms cleanup penalty on every `--js` fetch. The
/// `Child` is awaited unconditionally so the kernel can reap the parent and
/// we don't leave a zombie pid behind.
#[cfg(feature = "js-rendering")]
pub(super) async fn reap_pgroup(pgid: Pid, child: &mut TokioChild) {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};

    let term = killpg(pgid, Signal::SIGTERM);
    let already_gone = matches!(term, Err(Errno::ESRCH));
    if let Err(e) = term
        && e != Errno::ESRCH
    {
        warn!(error = %e, pgid = %pgid, "killpg SIGTERM failed");
    }
    if !already_gone {
        sleep(PGROUP_SIGTERM_GRACE).await;
        if let Err(e) = killpg(pgid, Signal::SIGKILL)
            && e != Errno::ESRCH
        {
            warn!(error = %e, pgid = %pgid, "killpg SIGKILL failed");
        }
    }
    // Reap so the kernel can release the parent slot. `Ok(Err)` means waitpid
    // itself failed (rare; e.g. ECHILD if a prior wait already reaped); `Err`
    // means the 2s budget elapsed before chromium exited, which is the zombie
    // path scout must surface so SHUTDOWN_DRAIN_TIMEOUT calibration stays
    // honest (issue #152).
    match timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, pgid = %pgid, "chromium child.wait() failed during reap"),
        Err(_) => warn!(
            timeout_secs = 2,
            pgid = %pgid,
            "chromium child did not exit within timeout after SIGKILL"
        ),
    }
}

/// Borrows browser so the caller retains ownership for cleanup on timeout.
#[cfg_attr(not(feature = "js-rendering"), allow(dead_code))]
fn resolve_browser_binary_from(
    path_commands: &[&str],
    known_paths: &[&Path],
) -> Result<PathBuf, BrowserError> {
    for cmd in path_commands {
        if let Ok(output) = Command::new("which").arg(cmd).output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            return Ok(PathBuf::from(path));
        }
    }

    for path in known_paths {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
    }

    Err(BrowserError::NotFound)
}

#[cfg(test)]
mod browser_binary_tests;
#[cfg(test)]
mod browser_request_tests;
#[cfg(all(test, feature = "js-rendering"))]
mod cdp_launch_tests;
#[cfg(all(test, feature = "js-rendering"))]
mod ws_url_parse_tests;
