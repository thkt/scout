//! Owned fake browser resources, shared by library and CLI regressions.
#![expect(
    dead_code,
    reason = "Library and CLI test crates use different fixture helpers"
)]

use std::cell::Cell;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tempfile::{TempDir, tempdir};

pub(crate) struct FakeBrowser {
    dir: TempDir,
    pub(crate) binary: PathBuf,
    cleaned: Cell<bool>,
}

impl FakeBrowser {
    pub(crate) fn new(close_stderr: bool) -> Self {
        let dir = tempdir().expect("fixture directory");
        let binary = dir.path().join("chromium");
        let record = dir.path().to_string_lossy().replace('\'', "'\\''");
        let stderr = if close_stderr { "exec 2>/dev/null" } else { "" };
        // Record the group before spawning descendants, so even failure before
        // the ready marker gives Drop enough information to clean up.
        fs::write(
            &binary,
            format!(
                r#"#!/bin/sh
record='{record}'
printf '%s' "$$" > "$record/group"
for arg in "$@"; do
    case "$arg" in
        --user-data-dir=*) printf '%s' "${{arg#--user-data-dir=}}" > "$record/profile";;
    esac
done
trap '' TERM INT
sleep 120 2>/dev/null &
printf '%s' "$!" > "$record/descendant"
touch "$record/ready"
{stderr}
wait
"#
            ),
        )
        .expect("write fake browser");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("executable");
        // Resolve every Linux discovery candidate to this fixture too.
        for name in ["google-chrome-stable", "google-chrome", "chromium-browser"] {
            symlink(&binary, dir.path().join(name)).expect("browser alias");
        }
        Self {
            dir,
            binary,
            cleaned: Cell::new(false),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn ready(&self) -> bool {
        self.dir.path().join("ready").exists()
    }

    pub(crate) fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.ready() {
            assert!(Instant::now() < deadline, "fake browser never became ready");
            thread::sleep(Duration::from_millis(10));
        }
        let group = self.pid("group");
        let descendant = self.pid("descendant");
        assert!(self.profile().is_dir(), "launch must create its profile");
        assert!(pid_running(descendant), "descendant must start alive");
        for pid in [group, descendant] {
            let output = Command::new("ps")
                .args(["-o", "pgid=", "-p", &pid.to_string()])
                .output()
                .expect("recorded process group");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                group.to_string(),
                "fixture must run in its owned browser group"
            );
        }
    }

    pub(crate) fn pid(&self, name: &str) -> Pid {
        Pid::from_raw(
            fs::read_to_string(self.dir.path().join(name))
                .expect("recorded PID")
                .parse()
                .expect("numeric PID"),
        )
    }

    pub(crate) fn profile(&self) -> PathBuf {
        PathBuf::from(fs::read_to_string(self.dir.path().join("profile")).expect("profile path"))
    }

    pub(crate) fn assert_clean(&self) {
        let group = self.pid("group");
        let descendant = self.pid("descendant");
        let deadline = Instant::now() + Duration::from_secs(3);
        while pid_running(group) || pid_running(descendant) {
            assert!(
                Instant::now() < deadline,
                "owned browser or descendant survived cleanup"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !self.profile().exists(),
            "temporary browser profile survived"
        );
        self.cleaned.set(true);
    }
}

impl Drop for FakeBrowser {
    fn drop(&mut self) {
        if self.cleaned.get() {
            return;
        }
        let mut failures = Vec::new();
        // Best-effort backstop independent of the production guard. Restrict
        // signals to the recorded group owned by this fixture, even on panic.
        if let Ok(record) = fs::read_to_string(self.dir.path().join("group"))
            && let Ok(pid) = record.parse::<i32>()
            && pid > 0
        {
            if let Err(e) = killpg(Pid::from_raw(pid), Signal::SIGKILL)
                && e != Errno::ESRCH
            {
                failures.push(format!("kill owned fixture group {pid}: {e}"));
            }
            let descendant = fs::read_to_string(self.dir.path().join("descendant"))
                .ok()
                .and_then(|record| record.parse::<i32>().ok())
                .map(Pid::from_raw);
            let deadline = Instant::now() + Duration::from_secs(3);
            while pid_running(Pid::from_raw(pid)) || descendant.is_some_and(pid_running) {
                if Instant::now() >= deadline {
                    failures.push("owned fixture group survived backstop SIGKILL".to_owned());
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        if let Ok(record) = fs::read_to_string(self.dir.path().join("profile"))
            && let Err(e) = fs::remove_dir_all(record)
            && e.kind() != ErrorKind::NotFound
        {
            failures.push(format!("remove fixture profile: {e}"));
        }
        if !failures.is_empty() {
            cleanup_failed(&failures.join("; "));
        }
    }
}

fn cleanup_failed(message: &str) {
    // Never double-panic during a failed assertion, but preserve the evidence
    // that the backstop itself failed. On an otherwise passing path, fail it.
    if thread::panicking() {
        eprintln!("fixture cleanup failed: {message}");
    } else {
        panic!("fixture cleanup failed: {message}");
    }
}

fn pid_running(pid: Pid) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("query owned PID");
    let state = String::from_utf8_lossy(&output.stdout);
    // An orphan zombie cannot execute or write profiles; its reaping belongs
    // to the OS, unlike the directly owned browser parent reaped by scout.
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

pub(crate) struct OwnedScout(pub(crate) Child);

impl Drop for OwnedScout {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn wait_scout(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().expect("scout wait") {
            return status;
        }
        assert!(Instant::now() < deadline, "scout exceeded bounded shutdown");
        thread::sleep(Duration::from_millis(10));
    }
}
