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

/// Probes PATH and known install locations per fetch; caching would pin the
/// first PATH result across injected browser fixtures.
#[cfg(feature = "js-rendering")]
pub(super) fn resolve_browser_binary() -> Result<PathBuf, BrowserError> {
    // Compile only the host platform's discovery table.
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

/// Validates browser subrequests (DR-0001). WebSocket URLs use the HTTP(S)
/// allowlist; synthetic browser schemes pass without external egress, and
/// unrecognized schemes are blocked.
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
    // This allowlist check resolves in scout, independently of proxy routing.
    ssrf::ssrf_check(&check_url, resolver, &ssrf::EgressMode::Direct)
        .await
        .is_ok()
}

/// TERM grace after CDP close, before forced group termination.
#[cfg(feature = "js-rendering")]
const PGROUP_SIGTERM_GRACE: Duration = Duration::from_millis(50);

/// Holds the group and profile across awaits. Drop sends SIGKILL synchronously
/// before profile deletion, even during runtime shutdown (DR-0032).
/// Child kill-on-drop alone cannot stop descendants.
#[cfg(feature = "js-rendering")]
pub(super) struct ChromiumProcess {
    child: TokioChild,
    pgid: Pid,
    group_armed: bool,
    _profile: TempDir,
}

#[cfg(feature = "js-rendering")]
impl ChromiumProcess {
    /// Completed paths retain bounded reap; cancellation invokes the Drop fallback.
    pub(super) async fn reap(&mut self) {
        reap_group(self.pgid, &mut self.group_armed, self.child.wait(), killpg).await;
    }

    fn kill_group(&mut self) {
        kill_group_with(self.pgid, &mut self.group_armed, killpg);
    }
}

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
    // Successful KILL or ESRCH disarms before wait can release the numeric PID.
    // Failed KILL leaves the Drop retry armed.
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

/// Spawns a new group and captures ownership without an intervening await.
/// chromiumoxide 0.9 hides the Command needed for `process_group(0)`, so scout
/// self-spawns and connects to the resulting DevTools URL.
#[cfg(feature = "js-rendering")]
pub(super) fn spawn_chromium_pgroup(
    browser_path: &Path,
    proxy_port: u16,
) -> Result<(ChromiumProcess, BufReader<ChildStderr>), BrowserError> {
    use std::process::Stdio;

    // A unique profile avoids SingletonLock collisions between concurrent fetches.
    // Its owner must survive group termination because chromium writes on shutdown.
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

    // Capture the group before fallible stderr extraction, without an await.
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

/// Reads stderr until chromium announces its DevTools WebSocket URL.
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
