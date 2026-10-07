//! Owned fake browser resources, shared by library and CLI regressions.
use std::cell::Cell;
use std::fs;
use std::io;
use std::io::ErrorKind;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output};
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
        Self::with_devtools(close_stderr, None)
    }

    pub(crate) fn with_devtools(close_stderr: bool, devtools: Option<&str>) -> Self {
        let dir = tempdir().expect("fixture directory");
        let binary = dir.path().join("chromium");
        let record = dir.path().to_string_lossy().replace('\'', "'\\''");
        let stderr = if close_stderr { "exec 2>/dev/null" } else { "" };
        let devtools = devtools.map_or(String::new(), |url| {
            format!("printf '%s\\n' 'DevTools listening on {url}' >&2\n")
        });
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
{devtools}wait
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

    pub(crate) fn wait_ready(&self) -> (Pid, Pid) {
        assert!(
            wait_until(Duration::from_secs(5), || self.ready()),
            "fake browser never became ready"
        );
        let group = self.pid("group");
        let descendant = self.pid("descendant");
        assert!(self.profile().is_dir(), "launch must create its profile");
        assert_pid_running_with(descendant, query_pid);
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
        (group, descendant)
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
        assert!(
            wait_until(Duration::from_secs(3), || !pid_running(group)
                && !pid_running(descendant)),
            "owned browser or descendant survived cleanup"
        );
        assert!(
            !self.profile().exists(),
            "temporary browser profile survived"
        );
        self.cleaned.set(true);
    }

    // Inject only destructive OS operations and the bounded liveness wait.
    // Keep record parsing and failure aggregation on the actual Drop path.
    pub(crate) fn cleanup_with(
        &self,
        mut kill: impl FnMut(Pid) -> Result<(), Errno>,
        mut wait: impl FnMut(Pid, Option<Pid>) -> Result<(), String>,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> Vec<String> {
        let mut failures = Vec::new();
        if let Some(group) = self.recorded_pid("group") {
            if let Err(e) = kill(group)
                && e != Errno::ESRCH
            {
                failures.push(format!("kill owned fixture group {group}: {e}"));
            }
            if let Err(e) = wait(group, self.recorded_pid("descendant")) {
                failures.push(e);
            }
        }
        if let Ok(record) = fs::read_to_string(self.dir.path().join("profile"))
            && let Err(e) = remove(Path::new(&record))
            && e.kind() != ErrorKind::NotFound
        {
            failures.push(format!("remove fixture profile: {e}"));
        }
        failures
    }

    fn recorded_pid(&self, name: &str) -> Option<Pid> {
        fs::read_to_string(self.dir.path().join(name))
            .ok()?
            .parse::<i32>()
            .ok()
            .filter(|&pid| pid > 0)
            .map(Pid::from_raw)
    }
}

impl Drop for FakeBrowser {
    fn drop(&mut self) {
        if self.cleaned.get() {
            return;
        }
        // Independent backstop restricted to the recorded, positive group.
        let failures = self.cleanup_with(
            |group| killpg(group, Signal::SIGKILL),
            |group, descendant| {
                if wait_until(Duration::from_secs(3), || {
                    !pid_running(group) && !descendant.is_some_and(pid_running)
                }) {
                    Ok(())
                } else {
                    Err("owned fixture group survived backstop SIGKILL".to_owned())
                }
            },
            |profile| fs::remove_dir_all(profile),
        );
        if !failures.is_empty() {
            cleanup_failed(&failures.join("; "));
        }
    }
}

pub(crate) fn wait_until(budget: Duration, mut complete: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + budget;
    while !complete() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

pub(crate) fn cleanup_failed(message: &str) {
    // Never double-panic during a failed assertion, but preserve the evidence
    // that the backstop itself failed. On an otherwise passing path, fail it.
    if thread::panicking() {
        eprintln!("fixture cleanup failed: {message}");
    } else {
        panic!("fixture cleanup failed: {message}");
    }
}

pub(crate) fn pid_running(pid: Pid) -> bool {
    pid_running_with(pid, query_pid)
}

pub(crate) fn pid_running_with(pid: Pid, query: impl FnOnce(Pid) -> io::Result<Output>) -> bool {
    // Absence must be observed before cleanup can complete.
    pid_state_with(pid, query) != PidState::Stopped
}

fn query_pid(pid: Pid) -> io::Result<Output> {
    Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PidState {
    Running,
    Stopped,
    Unobserved,
}

pub(crate) fn assert_pid_running_with(pid: Pid, query: impl FnOnce(Pid) -> io::Result<Output>) {
    // The conservative cleanup predicate cannot establish positive readiness.
    assert_eq!(
        pid_state_with(pid, query),
        PidState::Running,
        "descendant must be observed alive"
    );
}

pub(crate) fn pid_state_with(pid: Pid, query: impl FnOnce(Pid) -> io::Result<Output>) -> PidState {
    let output = match query(pid) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("cannot observe fixture PID {pid}: {error}");
            // Conservatively keep waiting; the bounded caller reports failure.
            return PidState::Unobserved;
        }
    };
    let state = String::from_utf8_lossy(&output.stdout);
    // ps may exit 1 with no output when the selected PID is absent. A
    // diagnostic, another exit code, or signal termination is not absence.
    if !output.stderr.is_empty()
        || !(output.status.success()
            || (output.status.code() == Some(1) && state.trim().is_empty()))
    {
        eprintln!(
            "cannot observe fixture PID {pid}: ps {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return PidState::Unobserved;
    }
    // An orphan zombie cannot execute or write profiles; its reaping belongs
    // to the OS, unlike the directly owned browser parent reaped by scout.
    if state.trim().is_empty() || state.trim().starts_with('Z') {
        PidState::Stopped
    } else {
        PidState::Running
    }
}

pub(crate) struct OwnedScout(pub(crate) Child);

impl Drop for OwnedScout {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn wait_scout(child: &mut Child) -> ExitStatus {
    let mut status = None;
    assert!(
        wait_until(Duration::from_secs(15), || {
            status = child.try_wait().expect("scout wait");
            status.is_some()
        }),
        "scout exceeded bounded shutdown"
    );
    status.expect("completed scout status")
}
