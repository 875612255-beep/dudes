//! The real binary: it starts, answers, refuses a second daemon, survives a `SIGKILL` and comes
//! back — and stops on `SIGTERM`.
//!
//! # Why the restart arm is required and not a nicety
//!
//! `LocalTransport::bind` creates and never removes, and `bind_in_with_credential` is
//! `O_CREAT|O_EXCL`. Both are right, and together they mean a daemon that exits *uncleanly* leaves a
//! socket and a credential behind that refuse the next start — **so a crashed daemon could not
//! restart at all**. Measured: dropping a `UnixListener` without unlinking
//! leaves the file present and the second bind fails `Address already in use (os error 48)`; the
//! same bind into a fresh path succeeds.
//!
//! Crash recovery means killing the daemon and replaying, which requires a restart.
//! Startup step 4's reclaim is what makes that possible, and this
//! file is what exercises it against a real process that was really killed.
//!
//! # Why the refusal arm asserts a name and not just a failure
//!
//! What the lock buys is not a corruption check, because two writers over one journal directory do
//! not corrupt it. Measured on this workspace's `dudes-journal`: two `Writer`s over one directory
//! give a reader a **well-formed** journal — no `Corrupt`, no torn tail, no segment gap — in two
//! shapes. Either they split into separate segments, and `lsn` order stops being time order, which
//! is the ordering the journal's atomicity claim rests on; or they share an empty segment and `append`
//! returns the **identical `Lsn` for two different records**, which is invariant 2's
//! `Persisted{lsn}` naming the wrong turn. Nothing at any layer detects either — and that is
//! structural rather than lucky: `writer.rs` opens the segment `.append(true)`, so every
//! `write_all` lands whole at the true end of the file while each writer counts its own `len` from
//! zero. The bytes are always a valid frame sequence; only the lsns handed back are lies.
//!
//! None of that is reachable through this binary, and the type system is why: `open_and_replay`
//! takes the `StateLock` **by value**, so opening the journal without holding the lock does not
//! compile. This file is what proves the door is locked rather than merely shut — and a refusal is
//! only evidence of the lock if the test can tell it from any other startup failure, which is what
//! the name in the assertion is for.
//!
//! # What this file cannot reach, and why
//!
//! This file drives the binary through `SIGTERM` and reads its journal, and that is the whole of
//! what it is for: the reclaim across a real kill. It does not call `agent.hire` or
//! `system.shutdown` against this process — the daemon does answer both to a caller presenting the
//! credential it minted, and `wire.rs` is where that is asserted, in process and against the same
//! composition. Driving them from here would test the same seam through a second door.
//!
//! Supplying a permissive authorizer to widen *this* file would mean changing what the binary
//! links, which is exactly what must not be done — and this package could not: `AdmitAll` and
//! `RefuseAll` are behind a `dudes-rpc` feature it does not enable.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dudes_proto::rpc::Method;
use dudes_rpc::Client;

/// A directory name holding a newline that would start a forged `duded:` line, and a right-to-left
/// override that would reorder what the operator reads after it.
const HOSTILE: &str = "run\nduded: forged\u{202e}";
/// [`HOSTILE`] as standard error shows it.
const SHOWN: &str = r"run\u{a}duded: forged\u{202e}";

struct Dirs {
    home: tempfile::TempDir,
    state: tempfile::TempDir,
    runtime: tempfile::TempDir,
}

impl Dirs {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().expect("an empty home directory"),
            state: tempfile::tempdir().expect("a state directory"),
            runtime: tempfile::tempdir().expect("a runtime directory"),
        }
    }

    fn socket(&self) -> PathBuf {
        self.runtime.path().join("run").join("duded.sock")
    }
}

/// A daemon under test, killed when the test that started it goes away.
///
/// `std::process::Child`'s own `Drop` neither signals nor waits, so a test that fails an assertion
/// while a daemon is running would leave it behind — holding a lock on a temporary directory the
/// unwind is about to delete, and the leaked daemon's own error message would arrive in the middle
/// of the failure.
struct Daemon(std::process::Child);

impl Daemon {
    /// The command that would start the real binary: `dirs`' runtime directory, `state_dir` for
    /// its state — the one place that path enters — and a scrubbed environment. It spawns nothing;
    /// [`Daemon::start`] and [`Daemon::start_capturing_stderr`] do, and differ only in what they
    /// do with standard error.
    ///
    /// # The one `std::process::Command` in this package, and why the gate had to be answered
    ///
    /// `bins/duded/clippy.toml` disallows the type and clippy runs with
    /// `--all-targets`, so this call **was refused** until the `allow` below was written — measured,
    /// not supposed. That refusal is the gate working: the rule has no module allow-list, so the
    /// exemption cannot hide inside one, and it has to be spelled at the statement that needs it,
    /// with a reason, where a reviewer reads it.
    ///
    /// The distinction it rests on is real: the lint bans the **daemon** from creating children,
    /// and this is a harness starting the binary under test — the other side of the same ban. What
    /// keeps that from becoming a hole in `src/` is `host_seam.rs`, whose scan reads the source text
    /// and therefore catches a `process::Command` there *whatever* attribute sits above it.
    #[allow(
        clippy::disallowed_types,
        reason = "the daemon may not create children; this is the test harness \
                  starting the daemon itself, and src/ is guarded by host_seam.rs's own scan"
    )]
    fn command(dirs: &Dirs, state_dir: &Path) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_duded"));
        command.env_clear();
        for name in ["PATH", "TMPDIR", "LANG"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("HOME", dirs.home.path());
        command.env("DUDES_STATE_DIR", state_dir);
        command.env("DUDES_RUNTIME_DIR", dirs.runtime.path().join("run"));
        command.stdin(std::process::Stdio::null());
        command
    }

    /// Start the real binary over `dirs`, standard error inherited.
    ///
    /// Inherited on purpose: for a daemon that is meant to *run*, its standard error is the
    /// diagnostic a failing run needs to show, and piping it would post it into a pipe no
    /// assertion reads.
    fn start(dirs: &Dirs) -> Self {
        Self(
            Self::command(dirs, dirs.state.path())
                .spawn()
                .expect("the daemon binary starts"),
        )
    }

    /// Start the real binary over `dirs` but on `state_dir`, with standard error captured.
    ///
    /// For the daemons that are meant to *fail*: what they say is the assertion, and taking
    /// `state_dir` rather than using `dirs.state` is what lets one of them be pointed at
    /// something that is not a directory at all.
    fn start_capturing_stderr(dirs: &Dirs, state_dir: &Path) -> Self {
        Self(
            Self::command(dirs, state_dir)
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("the daemon binary starts"),
        )
    }

    /// Start the real binary with `run` as its runtime directory and `home` as its home, standard
    /// error captured.
    fn start_in(dirs: &Dirs, run: &Path, home: &Path) -> Self {
        let mut command = Self::command(dirs, dirs.state.path());
        command
            .env("DUDES_RUNTIME_DIR", run)
            .env("HOME", home)
            .stderr(std::process::Stdio::piped());
        Self(command.spawn().expect("the daemon binary starts"))
    }

    /// Everything it wrote to standard error, for a daemon started by
    /// [`Daemon::start_capturing_stderr`].
    ///
    /// Called *after* [`Daemon::wait_within`], which is the safe order and not the accidental
    /// one: a startup failure prints one short line — far under the pipe buffer — so the child
    /// cannot block writing it while nothing is draining, and by the time it has exited the
    /// write end is closed and this read sees the whole of it and an EOF.
    fn stderr(&mut self) -> String {
        use std::io::Read as _;

        let mut said = String::new();
        self.0
            .stderr
            .take()
            .expect("this daemon was started with its standard error captured")
            .read_to_string(&mut said)
            .expect("its standard error reads to the end");
        said
    }

    /// Signal it.
    fn signal(&self, signal: rustix::process::Signal) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.0.id()).expect("a pid"))
            .expect("a pid");
        rustix::process::kill_process(pid, signal).expect("the signal is delivered");
    }

    /// Wait for it, failing rather than hanging.
    fn wait_within(&mut self, within: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.0.try_wait().expect("wait") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not exit within {within:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Block until `system.ping` is answered over `socket`, and return the answer.
    ///
    /// Polling the **call** rather than the socket file, and that is not belt and braces: the
    /// socket is bound at startup step 4 while the accept loop starts after step 7, and — the case
    /// that actually bit — a daemon that was `SIGKILL`ed leaves its socket file at that path, so a
    /// test that waited for the *file* would sail past a daemon that had not bound anything yet and
    /// connect to a dead run's leftover.
    fn await_ping(&mut self, socket: &Path) -> dudes_proto::rpc::system::PingResult {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(answer) = try_ping(socket) {
                return answer;
            }
            if let Some(status) = self.0.try_wait().expect("wait") {
                panic!("the daemon exited before it answered a ping: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "no ping was answered on {} within 20s",
                socket.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            self.signal(rustix::process::Signal::KILL);
            let _ = self.0.wait();
        }
    }
}

/// One `system.ping` attempt. `None` when nothing is listening yet.
///
/// `system.ping` is the one method a peer that has proved nothing may call,
/// which is exactly what keeps a daemon probeable by a client carrying no credential.
fn try_ping(socket: &Path) -> Option<dudes_proto::rpc::system::PingResult> {
    let mut client = Client::connect(socket).ok()?;
    let response = client.call(Method::SystemPing, None).ok()?;
    serde_json::from_value(response.result?).ok()
}

#[test]
fn a_credential_left_without_its_socket_refuses_the_next_start_and_stays() {
    use std::os::unix::fs::MetadataExt as _;

    let dirs = Dirs::new();
    let socket = dirs.socket();
    let credential = socket.with_file_name(dudes_rpc::OPERATOR_CREDENTIAL_NAME);
    let mut first = Daemon::start(&dirs);
    first.await_ping(&socket);
    first.signal(rustix::process::Signal::TERM);
    assert!(first.wait_within(Duration::from_secs(20)).success());
    let original = std::fs::metadata(&credential).expect("the stopped run left its credential");
    std::fs::remove_file(&socket).expect("remove only the stopped run's socket");

    let mut second = Daemon::start_capturing_stderr(&dirs, dirs.state.path());
    assert!(!second.wait_within(Duration::from_secs(20)).success());
    assert_eq!(
        second.stderr(),
        format!(
            "duded: creating the operator credential at {}: File exists (os error 17)\n",
            credential.display()
        )
    );
    let current = std::fs::metadata(&credential).expect("the credential remains");
    assert_eq!(
        (current.dev(), current.ino()),
        (original.dev(), original.ino())
    );
}

#[test]
fn a_stopped_daemon_restarts_when_both_runtime_files_are_removed() {
    let dirs = Dirs::new();
    let socket = dirs.socket();
    let credential = socket.with_file_name(dudes_rpc::OPERATOR_CREDENTIAL_NAME);
    let mut first = Daemon::start(&dirs);
    first.await_ping(&socket);
    first.signal(rustix::process::Signal::TERM);
    assert!(first.wait_within(Duration::from_secs(20)).success());
    std::fs::remove_file(&socket).expect("remove the stopped run's socket");
    std::fs::remove_file(&credential).expect("remove the stopped run's credential");

    let mut second = Daemon::start(&dirs);
    assert_eq!(second.await_ping(&socket).socket, socket);
    second.signal(rustix::process::Signal::TERM);
    assert!(second.wait_within(Duration::from_secs(20)).success());
}

#[test]
fn a_live_runtime_refuses_a_second_daemon_with_another_state_directory() {
    let dirs = Dirs::new();
    let socket = dirs.socket();
    let other_state = tempfile::tempdir().expect("another state directory");
    let mut first = Daemon::start(&dirs);
    first.await_ping(&socket);

    let mut second = Daemon::start_capturing_stderr(&dirs, other_state.path());
    assert!(!second.wait_within(Duration::from_secs(20)).success());
    let complaint = second.stderr();
    assert!(
        complaint.contains("a daemon is listening there"),
        "{complaint}"
    );
    assert_eq!(first.await_ping(&socket).socket, socket);
    first.signal(rustix::process::Signal::TERM);
    assert!(first.wait_within(Duration::from_secs(20)).success());
}

#[test]
fn a_live_credential_is_untouched_when_its_socket_path_is_removed() {
    use std::os::unix::fs::MetadataExt as _;

    let dirs = Dirs::new();
    let socket = dirs.socket();
    let credential = socket.with_file_name(dudes_rpc::OPERATOR_CREDENTIAL_NAME);
    let other_state = tempfile::tempdir().expect("another state directory");
    let mut first = Daemon::start(&dirs);
    first.await_ping(&socket);
    let original = std::fs::metadata(&credential).expect("the first daemon's credential");
    let mut connection = Client::connect(&socket).expect("keep a live connection");
    std::fs::remove_file(&socket).expect("unlink the live socket's path");

    let mut second = Daemon::start_capturing_stderr(&dirs, other_state.path());
    assert!(!second.wait_within(Duration::from_secs(20)).success());
    let complaint = second.stderr();
    assert!(complaint.contains("operator credential"), "{complaint}");
    assert!(
        complaint.contains(credential.to_str().unwrap()),
        "{complaint}"
    );
    let current = std::fs::metadata(&credential).expect("the original credential remains");
    assert_eq!(
        (current.dev(), current.ino()),
        (original.dev(), original.ino())
    );
    let response = connection
        .call(Method::SystemPing, None)
        .expect("the first daemon still serves");
    let answer: dudes_proto::rpc::system::PingResult =
        serde_json::from_value(response.result.expect("ping result")).expect("a ping result");
    assert_eq!(answer.socket, socket);
    first.signal(rustix::process::Signal::TERM);
    assert!(first.wait_within(Duration::from_secs(20)).success());
}

/// A start-up failure is one line of standard error whatever the runtime directory is called, and a
/// credential left there leads that line rather than the socket beside it.
#[test]
fn a_leftover_credential_in_a_hostile_runtime_directory_is_one_escaped_line() {
    use std::os::unix::fs::DirBuilderExt as _;

    let dirs = Dirs::new();
    let run = dirs.runtime.path().join(HOSTILE);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&run)
        .expect("an owner-only runtime directory");
    std::fs::write(
        run.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
        "left by a dead run",
    )
    .expect("a leftover credential");

    let mut daemon = Daemon::start_in(&dirs, &run, dirs.home.path());
    assert!(!daemon.wait_within(Duration::from_secs(20)).success());
    assert_eq!(
        daemon.stderr(),
        format!(
            "duded: creating the operator credential at {}/{SHOWN}/{}: File exists (os error 17)\n",
            dirs.runtime.path().display(),
            dudes_rpc::OPERATOR_CREDENTIAL_NAME
        )
    );
}

/// A failure that is the socket's still names the socket, on one line: here the runtime directory
/// is readable by other users.
#[test]
fn a_shared_hostile_runtime_directory_is_refused_on_one_escaped_line() {
    use std::os::unix::fs::PermissionsExt as _;

    let dirs = Dirs::new();
    let run = dirs.runtime.path().join(HOSTILE);
    std::fs::create_dir(&run).expect("a runtime directory");
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755))
        .expect("a mode other users can read");

    let mut daemon = Daemon::start_in(&dirs, &run, dirs.home.path());
    assert!(!daemon.wait_within(Duration::from_secs(20)).success());
    let shown = format!("{}/{SHOWN}", dirs.runtime.path().display());
    assert_eq!(
        daemon.stderr(),
        format!(
            "duded: binding {shown}/duded.sock: the runtime directory {shown} is mode 0755, not 0700\n"
        )
    );
}

/// The start-up line names the socket and the operator file on one line, whatever their
/// directories are called.
#[test]
fn the_startup_line_escapes_a_hostile_socket_and_operator_file() {
    let dirs = Dirs::new();
    let run = dirs.runtime.path().join(HOSTILE);
    let home = dirs.home.path().join(HOSTILE);
    std::fs::create_dir(&home).expect("a home directory");

    let mut daemon = Daemon::start_in(&dirs, &run, &home);
    daemon.await_ping(&run.join("duded.sock"));
    daemon.signal(rustix::process::Signal::TERM);
    assert!(daemon.wait_within(Duration::from_secs(20)).success());
    let config = if cfg!(target_os = "macos") {
        "Library/Application Support/dudes/config.toml"
    } else {
        ".config/dudes/config.toml"
    };
    let said = daemon.stderr();
    let (line, rest) = said.split_once('\n').expect("a startup line");
    assert_eq!(rest, "", "one line: {said}");
    let expected = format!(
        "duded {} on {}/{SHOWN}/duded.sock; config {}/{SHOWN}/{config} (absent); ",
        env!("CARGO_PKG_VERSION"),
        dirs.runtime.path().display(),
        dirs.home.path().display()
    );
    assert!(line.starts_with(&expected), "{line}\n{expected}");
}

/// The whole of what the real binary can be held to at this stage, in one run so the
/// directories are shared between the arms that need them to be.
#[test]
fn the_daemon_starts_refuses_a_second_survives_a_kill_and_stops_on_sigterm() {
    let dirs = Dirs::new();
    let socket = dirs.socket();

    // --- it starts and answers -----------------------------------------------------------------
    let mut first = Daemon::start(&dirs);
    let answer = first.await_ping(&socket);
    assert_eq!(
        answer.socket, socket,
        "the daemon reports the socket it actually bound"
    );
    assert_eq!(answer.version, env!("CARGO_PKG_VERSION"));

    // --- a second daemon on the same state directory is refused, *by name* ----------------------
    // Invariant 1, through the real binary: the lock is held by a live process, so this one exits
    // non-zero rather than opening the same journal. `first.await_ping` above is what makes that
    // sentence true — it returns only once the first daemon serves, long past startup step 1, so
    // "the second was refused" cannot be satisfied by "the first never started".
    let mut second = Daemon::start_capturing_stderr(&dirs, dirs.state.path());
    let status = second.wait_within(Duration::from_secs(20));
    assert!(
        !status.success(),
        "a second daemon on the same state directory must refuse to start"
    );
    // The exit status alone is not enough: `main.rs`
    // has one failure code, so a daemon that died of anything else exits `1` identically. Measured
    // rather than supposed — with `flock` deleted from `lock::open_exclusive`, this arm stayed
    // **green**, because the second daemon then got as far as the projection store and redb
    // refused it with "Database already open". Step 1 itself cannot be removed — `open_and_replay`
    // takes the `StateLock` by value — but its *lock* could have been reduced to a file touch
    // without this test noticing.
    //
    // Nor does the obvious substring separate them: `boot.rs` wraps *every* outcome of
    // `StateLock::take` with "taking the single-writer lock on …", so a state directory that is a
    // regular file carries that phrase too — the sibling test below is the arm that measures it.
    // What only the `WouldBlock` re-wrap in `lock::open_exclusive` says is:
    let complaint = second.stderr();
    assert!(
        complaint.contains("another daemon already holds"),
        "the refusal must name a second daemon, not merely fail: {complaint}"
    );
    // …on the lock file, whose name comes from the constant rather than a literal so that renaming
    // it breaks the compile instead of quietly unbinding the assertion.
    let lock_file = dirs.state.path().join(duded::lock::LOCK_FILE_NAME);
    assert!(
        complaint.contains(lock_file.to_string_lossy().as_ref()),
        "and name {}, the lock it was refused on: {complaint}",
        lock_file.display()
    );

    // --- SIGKILL, and the restart the reclaim makes possible ------------------------------------
    // `SIGKILL` runs no exit sequence, so the socket and the credential are left exactly as a
    // crashed daemon leaves them — which is the state startup step 4 has to reclaim.
    first.signal(rustix::process::Signal::KILL);
    first.wait_within(Duration::from_secs(20));
    assert!(
        std::fs::symlink_metadata(&socket).is_ok(),
        "control: the killed daemon left its socket behind, which is what makes the restart hard"
    );
    let credential = dirs
        .runtime
        .path()
        .join("run")
        .join(dudes_rpc::OPERATOR_CREDENTIAL_NAME);
    assert!(
        credential.exists(),
        "control: and its credential, which `create_new` would otherwise refuse"
    );

    let mut third = Daemon::start(&dirs);
    let answer = third.await_ping(&socket);
    assert_eq!(
        answer.socket, socket,
        "a daemon that was SIGKILLed restarts on the same directories and serves again"
    );

    // --- SIGTERM stops it cleanly --------------------------------------------------------------
    // The path `main.rs` blocks the signal for and waits on: nothing here installs a handler, so
    // this is the safe form working end to end.
    third.signal(rustix::process::Signal::TERM);
    let status = third.wait_within(Duration::from_secs(20));
    assert!(
        status.success(),
        "SIGTERM is an orderly stop, not a crash: {status}"
    );
}

/// The arm that gives the refusal above its meaning: a daemon that fails at the lock step for a
/// reason that is *not* a second daemon must not be reported as one.
///
/// Without it, "the second daemon's stderr says `another daemon already holds`" pins a string
/// rather than a cause — and the near-miss is close enough to be worth measuring rather than
/// assuming. Both failures exit `1`, and both stderr lines carry `boot`'s shared
/// `"taking the single-writer lock on …"` context, which is why the control below asserts that the
/// confounder reaches the same wrapping site. Only the `WouldBlock` re-wrap separates them.
///
/// Its own directories, so the shared-directory reasoning of the test above is untouched.
#[test]
fn a_daemon_refused_for_another_reason_does_not_name_the_lock() {
    let dirs = Dirs::new();
    // A regular file where the state directory should be: `StateLock::take` creates the directory
    // first, and `create_dir_all` over a regular file fails `EEXIST`. Startup step 1, so nothing
    // has been bound, spawned or opened when it fails.
    let not_a_directory = dirs.state.path().join("not-a-directory");
    std::fs::write(&not_a_directory, b"").expect("a regular file where a directory should be");

    let mut daemon = Daemon::start_capturing_stderr(&dirs, &not_a_directory);
    // Step 1 is the first thing `boot` does, so this deadline is orders of magnitude more than the
    // arm needs and is not a race this test can lose on a loaded machine. If it were ever missed,
    // `wait_within` fails with "the daemon did not exit within 20s" — which names what actually
    // happened, a daemon that did *not* fail at the lock step, and never mis-reports it as a
    // refusal that did or did not name the lock.
    let status = daemon.wait_within(Duration::from_secs(20));
    assert!(
        !status.success(),
        "control: a daemon whose state directory is a regular file cannot start"
    );

    let complaint = daemon.stderr();
    assert!(
        complaint.contains("single-writer lock"),
        "control: this failure is the lock step's own, so it is the near-miss the arm above has to \
         be distinguished from and not merely some other error: {complaint}"
    );
    assert!(
        !complaint.contains("another daemon already holds"),
        "a startup failure that is not a second daemon must not claim to be one: {complaint}"
    );
}
