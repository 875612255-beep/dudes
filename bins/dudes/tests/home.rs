//! Without an absolute `HOME`, the CLI refuses a directory no variable names, in the daemon's own
//! words, instead of resolving it under the directory the command runs in.
//!
//! Each run happens in an empty temporary directory, so "nothing was created here" is a directory
//! listing. A build that still resolved against the working directory would start a daemon whose
//! journal and socket sit in that directory; the guard stops any daemon such a build left behind.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

mod common;

use common::{Cli, Run};

const SPAWN_MARKER: &str = "quickstart started duded pid ";

/// The three ways to have no absolute home: unset, empty and relative.
const NO_HOME: [Option<&str>; 3] = [None, Some(""), Some("relative/home")];

fn daemon_binary() -> PathBuf {
    let mut daemon = PathBuf::from(env!("CARGO_BIN_EXE_dudes"));
    daemon.set_file_name("duded");
    assert!(
        daemon.is_file(),
        "{} is absent; run `cargo build -p duded --locked` before a filtered nextest run",
        daemon.display()
    );
    daemon
}

fn cli_in(cwd: &Path, home: Option<&str>) -> Cli {
    let mut cli = Cli::bare()
        .env("PATH", "/usr/bin:/bin")
        .env("DUDES_DAEMON", daemon_binary())
        .current_dir(cwd);
    if let Some(home) = home {
        cli = cli.env("HOME", home);
    }
    cli
}

/// Every `duded.log` under `root`, wherever a build put one.
fn logs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "duded.log") {
                found.push(path);
            }
        }
    }
    found
}

/// Stops every daemon a quickstart under `root` started, when dropped.
struct Daemons<'a>(&'a Path);

impl Drop for Daemons<'_> {
    fn drop(&mut self) {
        for log in logs(self.0) {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            for pid in text
                .lines()
                .filter_map(|line| line.strip_prefix(SPAWN_MARKER)?.parse::<i32>().ok())
            {
                let pid = Pid::from_raw(pid);
                let _ = kill(pid, Signal::SIGTERM);
                let deadline = Instant::now() + Duration::from_secs(10);
                while kill(pid, None).is_ok() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = kill(pid, Signal::SIGKILL);
            }
        }
    }
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read the working directory")
        .map(|entry| entry.expect("a directory entry").path())
        .collect()
}

fn assert_refused(run: &Run, cwd: &Path, override_name: &str, home: Option<&str>) {
    assert_eq!(run.code(), dudes::exit::OTHER, "HOME {home:?}: {run:?}");
    assert_eq!(
        run.stderr.trim_end(),
        format!("dudes: HOME is unavailable; set an absolute HOME or {override_name}"),
        "HOME {home:?}: the daemon's own words"
    );
    assert_eq!(
        entries(cwd),
        Vec::<PathBuf>::new(),
        "HOME {home:?}: nothing may be created in the working directory"
    );
}

/// `quickstart` names the state directory first, as the daemon does, and starts nothing.
#[test]
fn quickstart_refuses_without_an_absolute_home() {
    for home in NO_HOME {
        let cwd = tempfile::tempdir().expect("a working directory");
        let _daemons = Daemons(cwd.path());
        let run = cli_in(cwd.path(), home).arg("quickstart").run();
        assert_refused(&run, cwd.path(), "DUDES_STATE_DIR", home);
    }
}

/// An operator command names the runtime directory, where its socket and credential live.
#[test]
fn an_operator_command_refuses_without_an_absolute_home() {
    for home in NO_HOME {
        let cwd = tempfile::tempdir().expect("a working directory");
        let run = cli_in(cwd.path(), home).arg("ping").run();
        assert_refused(&run, cwd.path(), "DUDES_RUNTIME_DIR", home);
    }
}

/// An empty override is refused in the daemon's words, even beside an absolute `HOME`, and is
/// never read as the working directory.
#[test]
fn an_empty_override_is_refused() {
    let cwd = tempfile::tempdir().expect("a working directory");
    let home = tempfile::tempdir().expect("a home");
    let home = home.path().to_str().expect("a UTF-8 temporary path");
    for (name, command) in [
        ("DUDES_RUNTIME_DIR", "ping"),
        ("DUDES_STATE_DIR", "quickstart"),
    ] {
        let _daemons = Daemons(cwd.path());
        let run = cli_in(cwd.path(), Some(home))
            .env(name, "")
            .arg(command)
            .run();
        assert_eq!(run.code(), dudes::exit::OTHER, "{name}: {run:?}");
        assert_eq!(
            run.stderr.trim_end(),
            format!("dudes: {name} is empty"),
            "{name}"
        );
        assert_eq!(entries(cwd.path()), Vec::<PathBuf>::new(), "{name}");
    }
}

/// The control: a runtime directory named outright needs no home, so the same command goes on to
/// look for the daemon there and reports it unreachable at that socket.
#[test]
fn an_explicit_runtime_directory_needs_no_home() {
    let cwd = tempfile::tempdir().expect("a working directory");
    let root = tempfile::tempdir().expect("a daemon root");
    let run = cli_in(cwd.path(), None)
        .env("DUDES_RUNTIME_DIR", root.path().join("run"))
        .arg("ping")
        .run();
    assert_eq!(run.code(), dudes::exit::UNREACHABLE, "{run:?}");
    assert!(
        run.stderr.starts_with(&format!(
            "dudes: cannot reach the daemon at {}:",
            root.path().join("run/duded.sock").display()
        )),
        "{run:?}"
    );
    assert_eq!(entries(cwd.path()), Vec::<PathBuf>::new());
}
