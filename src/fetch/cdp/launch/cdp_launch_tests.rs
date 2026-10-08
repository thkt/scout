use super::*;
use crate::test_support::browser_fixture::FakeBrowser;
use futures::poll;
use std::cell::Cell;
use std::future::{pending, ready};
use std::io;
use tokio::time::Instant;
use tokio::time::pause;

/// [T-BGC001] Drop during TERM grace must stop resistant descendants.
#[tokio::test]
async fn browser_owner_cleans_interrupted_reap() {
    let browser = FakeBrowser::new(false);
    let (mut process, _reader) = spawn_chromium_pgroup(&browser.binary, 0).expect("fake launch");
    browser.wait_ready();
    // Keep TERM grace pending so owner destruction exercises the fallback.
    pause();
    {
        let reap = process.reap();
        tokio::pin!(reap);
        assert!(poll!(reap.as_mut()).is_pending());
    }
    drop(process);
    browser.assert_clean();
}

/// [T-F043]
#[test]
fn t009_launch_args_contain_security_flags() {
    let args = build_launch_args(0);
    for flag in [
        "--disable-webrtc",
        "--disable-background-networking",
        "--disable-features=DnsOverHttps",
        "--disable-domain-reliability",
        "--no-pings",
    ] {
        assert!(
            args.iter().any(|a| a == flag),
            "missing security flag: {flag}"
        );
    }
}

/// [T-F085] The nine non-proxy hardening flags match exactly, including order.
#[test]
fn t085_launch_args_hardening_set_matches_exactly() {
    let args = build_launch_args(0);
    let hardening: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with("--proxy-") && *a != "--disable-quic")
        .collect();
    assert_eq!(
        hardening,
        [
            "--headless=new",
            "--disable-webrtc",
            "--disable-background-networking",
            "--disable-features=DnsOverHttps",
            "--disable-domain-reliability",
            "--no-pings",
            "--disable-extensions",
            "--no-first-run",
            "--disable-default-apps",
        ]
    );
}

/// [T-201-8] Launch includes the SOCKS5 port, loopback bypass override and QUIC disable flag.
#[test]
fn t201_8_launch_args_contain_ssrf_proxy_flags() {
    let args = build_launch_args(54321);
    assert!(
        args.iter()
            .any(|a| a == "--proxy-server=socks5://127.0.0.1:54321"),
        "missing SOCKS5 proxy-server flag with port"
    );
    for flag in ["--proxy-bypass-list=<-loopback>", "--disable-quic"] {
        assert!(args.iter().any(|a| a == flag), "missing proxy flag: {flag}");
    }
}

/// [T-BGC007] Injected OS failures preserve KILL/wait, retry state and bounded wait.
#[tokio::test(start_paused = true)]
#[tracing_test::traced_test]
async fn reap_preserves_fallback_on_os_errors() {
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    let pgid = Pid::from_raw(42);
    for kill_result in [Ok(()), Err(Errno::ESRCH), Err(Errno::EPERM)] {
        let mut armed = true;
        let mut sent = Vec::new();
        let waited = Cell::new(false);
        reap_group(
            pgid,
            &mut armed,
            async {
                waited.set(true);
                Err(io::Error::other("injected wait failure"))
            },
            |pid, signal| {
                assert_eq!(pid, pgid);
                sent.push(signal);
                if signal == Signal::SIGTERM {
                    Err(Errno::EPERM)
                } else {
                    kill_result
                }
            },
        )
        .await;
        assert_eq!(sent, [Signal::SIGTERM, Signal::SIGKILL]);
        assert!(waited.get(), "wait must follow signal failure");
        assert_eq!(armed, kill_result == Err(Errno::EPERM));
        if armed {
            kill_group_with(pgid, &mut armed, |_, signal| {
                assert_eq!(signal, Signal::SIGKILL);
                Ok(())
            });
            assert!(!armed, "successful fallback must disarm");
        }
    }
    assert!(logs_contain("killpg SIGTERM failed"));
    assert!(logs_contain("killpg SIGKILL failed"));
    assert!(logs_contain("chromium child.wait() failed during reap"));

    let mut armed = true;
    let mut sent = Vec::new();
    reap_group(
        pgid,
        &mut armed,
        ready(Ok(ExitStatus::from_raw(0))),
        |_, signal| {
            sent.push(signal);
            Err(Errno::ESRCH)
        },
    )
    .await;
    assert_eq!(sent, [Signal::SIGTERM], "gone group must not receive KILL");
    assert!(!armed);

    armed = true;
    let started = Instant::now();
    reap_group(pgid, &mut armed, pending(), |_, _| Ok(())).await;
    assert!(
        !armed,
        "bounded wait must not rearm an already killed group"
    );
    assert_eq!(
        started.elapsed(),
        PGROUP_SIGTERM_GRACE + Duration::from_secs(2)
    );
    assert!(logs_contain("chromium child did not exit within timeout"));
}
