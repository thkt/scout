use super::browser_fixture::*;
use std::cell::RefCell;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tempfile::tempdir;

/// [T-BGC008] Invalid group records cannot signal; cleanup aggregates OS failures.
#[test]
fn fixture_cleanup_preserves_scope_and_collects_failures() {
    let browser = FakeBrowser::new(false);
    // Remove synthetic records before FakeBrowser drops, including on panic.
    struct ClearGroup(PathBuf);
    impl Drop for ClearGroup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _records = ClearGroup(browser.path().join("group"));
    for record in [None, Some("0"), Some("-1"), Some("not-a-pid")] {
        if let Some(record) = record {
            fs::write(browser.path().join("group"), record).unwrap();
        }
        assert!(
            browser
                .cleanup_with(
                    |_| panic!("invalid group must not be signalled"),
                    |_, _| panic!("invalid group must not be queried"),
                    |_| panic!("absent profile must not be removed"),
                )
                .is_empty()
        );
    }
    fs::write(browser.path().join("group"), "42").unwrap();
    fs::write(browser.path().join("descendant"), "43").unwrap();
    // Keep a safe owned path even if injected removal unexpectedly changes.
    let profile = tempdir().unwrap();
    fs::write(
        browser.path().join("profile"),
        profile.path().to_str().unwrap(),
    )
    .unwrap();
    let operations = RefCell::new(Vec::new());
    let failures = browser.cleanup_with(
        |group| {
            assert_eq!(group, Pid::from_raw(42));
            operations.borrow_mut().push("kill");
            Err(Errno::EPERM)
        },
        |group, descendant| {
            assert_eq!(group, Pid::from_raw(42));
            assert_eq!(descendant, Some(Pid::from_raw(43)));
            operations.borrow_mut().push("wait");
            Err("owned fixture group survived backstop SIGKILL".into())
        },
        |path| {
            assert_eq!(path, profile.path());
            operations.borrow_mut().push("remove");
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected deletion failure",
            ))
        },
    );
    assert_eq!(*operations.borrow(), ["kill", "wait", "remove"]);
    assert_eq!(failures.len(), 3);
    assert!(failures[0].contains("kill owned fixture group 42"));
    assert!(failures[1].contains("survived backstop"));
    assert!(failures[2].contains("remove fixture profile"));
    assert!(
        browser
            .cleanup_with(
                |_| Err(Errno::ESRCH),
                |_, _| Ok(()),
                |_| Err(io::Error::from(io::ErrorKind::NotFound)),
            )
            .is_empty()
    );
    // Never let the real destructor send a signal to the synthetic PID.
    fs::remove_file(browser.path().join("group")).unwrap();
}

/// [T-BGC009] Fixture Drop stops its group, removes its profile and preserves the panic.
#[test]
fn fixture_backstop_cleans_on_unwind() {
    let browser = FakeBrowser::new(false);
    let profile = tempdir().unwrap();
    let profile_path = profile.path().to_path_buf();
    let mut parent = OwnedScout(
        Command::new(&browser.binary)
            .arg(format!("--user-data-dir={}", profile_path.display()))
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    // The fixture is under test: retain a separate group backstop derived
    // from spawn, so even a broken fixture Drop cannot leak descendants.
    struct KillOwnedGroup(Pid);
    impl Drop for KillOwnedGroup {
        fn drop(&mut self) {
            if let Err(error) = killpg(self.0, Signal::SIGKILL)
                && error != Errno::ESRCH
            {
                eprintln!("independent fixture group backstop failed: {error}");
            }
        }
    }
    let backstop = KillOwnedGroup(Pid::from_raw(i32::try_from(parent.0.id()).unwrap()));
    let (group, descendant) = browser.wait_ready();
    let failure = catch_unwind(AssertUnwindSafe(move || {
        let _browser = browser;
        panic!("original regression assertion");
    }));
    assert_eq!(
        failure.unwrap_err().downcast_ref::<&str>(),
        Some(&"original regression assertion")
    );
    assert!(!pid_running(group));
    assert!(!pid_running(descendant));
    assert!(!profile_path.exists());
    // Retire the backstop before wait releases the leader's numeric PID.
    drop(backstop);
    assert!(!wait_scout(&mut parent.0).success());
}

/// [T-BGC012] ps failures establish neither readiness nor cleanup; absence/zombies stop waiting.
#[test]
fn fixture_liveness_distinguishes_absence_from_observation_failure() {
    let pid = Pid::from_raw(42);
    for (status, stdout, stderr, expected) in [
        (0, "", "", PidState::Stopped),
        (1 << 8, "\n", "", PidState::Stopped),
        (0, " Z+\n", "", PidState::Stopped),
        (0, " S\n", "", PidState::Running),
        (1 << 8, "", "ps: permission denied", PidState::Unobserved),
        (0, "", "ps: observation incomplete", PidState::Unobserved),
        (2 << 8, "", "", PidState::Unobserved),
        (9, "", "", PidState::Unobserved),
    ] {
        let query = |selected| {
            assert_eq!(selected, pid);
            Ok(Output {
                status: ExitStatus::from_raw(status),
                stdout: stdout.as_bytes().to_vec(),
                stderr: stderr.as_bytes().to_vec(),
            })
        };
        assert_eq!(pid_state_with(pid, query), expected);
        assert_eq!(pid_running_with(pid, query), expected != PidState::Stopped);
        let readiness = catch_unwind(|| assert_pid_running_with(pid, query));
        assert_eq!(
            readiness.is_ok(),
            expected == PidState::Running,
            "status={status}, stdout={stdout:?}, stderr={stderr:?}"
        );
    }
    let failed_query = |_| Err(io::Error::from(io::ErrorKind::PermissionDenied));
    assert_eq!(pid_state_with(pid, failed_query), PidState::Unobserved);
    assert!(pid_running_with(pid, failed_query));
    assert!(catch_unwind(|| assert_pid_running_with(pid, failed_query)).is_err());
}

/// [T-BGC010] Polling expires; cleanup failure panics without masking an existing panic.
/// CLI wait preserves exit 23.
#[test]
fn fixture_wait_and_diagnostics_are_bounded() {
    assert!(!wait_until(Duration::ZERO, || false));
    let mut attempts = 0;
    assert!(wait_until(Duration::from_secs(1), || {
        attempts += 1;
        attempts == 2
    }));
    let failure = catch_unwind(|| cleanup_failed("injected backstop failure"));
    assert!(failure.is_err());
    struct FailedCleanup;
    impl Drop for FailedCleanup {
        fn drop(&mut self) {
            cleanup_failed("injected backstop failure during unwind");
        }
    }
    let failure = catch_unwind(|| {
        let _cleanup = FailedCleanup;
        panic!("original failure");
    });
    assert_eq!(
        failure.unwrap_err().downcast_ref::<&str>(),
        Some(&"original failure")
    );

    let mut command = OwnedScout(
        Command::new("/bin/sh")
            .args(["-c", "exit 23"])
            .spawn()
            .unwrap(),
    );
    assert_eq!(wait_scout(&mut command.0).code(), Some(23));
}
