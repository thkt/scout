//! Browser binary discovery and Chrome process lifecycle for CDP rendering.

use std::borrow::Cow;
#[cfg(feature = "js-rendering")]
use std::future::Future;
#[cfg(feature = "js-rendering")]
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "js-rendering")]
use std::process::ExitStatus;
#[cfg(feature = "js-rendering")]
use std::time::Duration;

#[cfg(feature = "js-rendering")]
use nix::unistd::Pid;
#[cfg(feature = "js-rendering")]
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
};
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
/// locations. Called once per `--js` fetch rather than cached in a
/// process-global `OnceLock`, which broke test isolation and pinned the
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
/// SOCKS5 proxy so connect-time IPs are re-validated:
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
pub(super) async fn check_browser_request(url: &str, resolver: &dyn ssrf::DnsResolver) -> bool {
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

/// Own the entire browser group and its profile across every await point.
///
/// Drop must work even when the runtime is shutting down: send SIGKILL
/// synchronously, before TempDir removes the profile. The Child's existing
/// kill-on-drop handles the directly owned parent; it cannot kill descendants.
/// No asynchronous cleanup task may carry this responsibility (DR-0032).
#[cfg(feature = "js-rendering")]
pub(super) struct ChromiumProcess {
    child: TokioChild,
    pgid: Pid,
    group_armed: bool,
    _profile: TempDir,
}

#[cfg(feature = "js-rendering")]
impl ChromiumProcess {
    /// Preserve TERM grace and bounded parent reaping on completed paths.
    /// Cancellation during the grace period still invokes the Drop fallback.
    pub(super) async fn reap(&mut self) {
        reap_group(self.pgid, &mut self.group_armed, self.child.wait(), killpg).await;
    }

    fn kill_group(&mut self) {
        kill_group_with(self.pgid, &mut self.group_armed, killpg);
    }
}

// Keep the OS boundary injectable without changing the ownership or timing.
#[cfg(feature = "js-rendering")]
async fn reap_group(
    pgid: Pid,
    armed: &mut bool,
    wait: impl Future<Output = io::Result<ExitStatus>>,
    mut signal: impl FnMut(Pid, Signal) -> Result<(), Errno>,
) {
    match signal(pgid, Signal::SIGTERM) {
        Err(Errno::ESRCH) => *armed = false,
        term => {
            if let Err(e) = term {
                warn!(error = %e, pgid = %pgid, "killpg SIGTERM failed");
            }
            sleep(PGROUP_SIGTERM_GRACE).await;
            kill_group_with(pgid, armed, signal);
        }
    }
    // Disarm before parent wait can make its numeric PID available for reuse.
    match timeout(Duration::from_secs(2), wait).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, pgid = %pgid, "chromium child.wait() failed during reap"),
        Err(_) => {
            warn!(timeout_secs = 2, pgid = %pgid, "chromium child did not exit within timeout after SIGKILL")
        }
    }
}

#[cfg(feature = "js-rendering")]
fn kill_group_with(
    pgid: Pid,
    armed: &mut bool,
    mut signal: impl FnMut(Pid, Signal) -> Result<(), Errno>,
) {
    match signal(pgid, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => *armed = false,
        Err(e) => warn!(error = %e, pgid = %pgid, "killpg SIGKILL failed"),
    }
}

#[cfg(feature = "js-rendering")]
impl Drop for ChromiumProcess {
    fn drop(&mut self) {
        if self.group_armed {
            self.kill_group();
        }
    }
}

/// Spawn chromium in a new process group and return its owner and stderr reader.
///
/// Synchronous so the caller captures `pgid` before any timeout can drop the
/// future and orphan the group. The pgid equals the chromium child's pid (the
/// call uses `process_group(0)`, which means "make the child the leader of a
/// new group whose id is its pid"). scout retains the `Child` so the kernel
/// can reap the parent after we kill the group.
///
/// chromiumoxide 0.9 hides `tokio::process::Command` behind a private wrapper,
/// so `BrowserConfig::launch` cannot set `process_group(0)`. We self-spawn and
/// hand the resulting WebSocket URL to `Browser::connect` instead.
#[cfg(feature = "js-rendering")]
pub(super) fn spawn_chromium_pgroup(
    browser_path: &Path,
    proxy_port: u16,
) -> Result<(ChromiumProcess, BufReader<ChildStderr>), BrowserError> {
    use std::process::Stdio;

    // `TempDir` gives each --js fetch a unique profile dir (random suffix avoids
    // chromium's `SingletonLock` failure when two scout processes run --js
    // concurrently) and deletes it on `Drop`. The caller must hold the returned
    // owner until after group termination, because chromium keeps writing
    // profile state during graceful shutdown.
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

    let child = cmd
        .spawn()
        .map_err(|e| BrowserError::ProcessFailed(format!("spawn chromium: {e}")))?;
    let pid = child
        .id()
        .ok_or_else(|| BrowserError::ProcessFailed("chromium pid unavailable".into()))?;
    let pgid = Pid::from_raw(
        i32::try_from(pid)
            .map_err(|_| BrowserError::ProcessFailed("chromium pid out of i32 range".into()))?,
    );

    // Establish group ownership before the next fallible step, with no await
    // between spawn and this guard. Even a missing stderr cleans up the group.
    let mut process = ChromiumProcess {
        child,
        pgid,
        group_armed: true,
        _profile: user_data_dir,
    };
    let stderr = process
        .child
        .stderr
        .take()
        .ok_or_else(|| BrowserError::ProcessFailed("chromium stderr missing".into()))?;
    Ok((process, BufReader::new(stderr)))
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
