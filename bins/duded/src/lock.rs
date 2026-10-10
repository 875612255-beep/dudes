//! Startup step 1: the single-writer lock on the state directory.
//!
//! Invariant 1 says persistent state exists only as records appended to the journal **by the
//! daemon** — singular. Nothing in `dudes-journal` enforces that: two `Writer`s over one directory
//! each believe they are the only one, and that takes two shapes — separate segments, or a shared
//! empty one where `append` hands back the **identical `Lsn` for two different records**. Both are
//! measured, with what a reader sees afterwards, under "Why the refusal arm asserts a name and not
//! just a failure" in `tests/restart.rs`; this comment cites that rather than restating it. redb's
//! exclusive lock on `state.redb` is a backstop that guards the wrong file and degrades silently
//! on filesystems without lock support.
//!
//! # Why the lock is an open descriptor and not a file that exists
//!
//! A pid file has to answer "is that pid still the daemon", which is the same
//! identifier-reuse question `dudes-host` exists for, and it has to answer it *before* anything
//! can start — including after a `SIGKILL`, when nothing ran to clean up. An advisory lock on an
//! open descriptor answers it for free: the kernel releases the lock when the descriptor closes,
//! which happens on every exit a process can have, unwinding or not. So a crashed daemon leaves a
//! `LOCK` file that the next start takes immediately, and a *live* daemon's lock is refused
//! whatever the file says.
//!
//! On Unix that is `flock(2)`, non-blocking. On Windows it is the same idea through the sharing
//! mode: a handle opened with `share_mode(0)` cannot be opened a second time, and closes with the
//! process.
//!
//! # What holding it does and does not prove
//!
//! It proves no other process is writing **this state directory**. It says nothing about the
//! runtime directory, which is a separate path a caller may point somewhere else — which is why
//! [`crate::socket`]'s reclaim asks the socket itself whether a daemon is listening rather than
//! resting on this.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// The name the lock file takes inside the state directory.
pub const LOCK_FILE_NAME: &str = "LOCK";

/// The held single-writer lock. Dropping it releases the lock, and so does the process exiting.
///
/// The `File` is held rather than used: its whole purpose is to keep the descriptor open. Named
/// `_file` would hide that from a reader, so it is named and this comment says why.
#[derive(Debug)]
pub struct StateLock {
    /// The open descriptor the kernel associates the lock with.
    file: File,
    /// Where it is, for a failure message and for a test.
    path: PathBuf,
}

impl StateLock {
    /// Create `dir` if it is not there, and take the exclusive lock inside it.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::WouldBlock`] when another process already holds it — the second-daemon
    /// case, refused by name so a caller can tell it from a permission problem — and whatever
    /// creating the directory or the file returns.
    pub fn take(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(LOCK_FILE_NAME);
        let file = open_exclusive(&path)?;
        Ok(Self { file, path })
    }

    /// Where the lock file is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The open descriptor, so a caller can see the lock is a live thing rather than a file that
    /// exists. Used by nothing but the type's own tests today.
    #[must_use]
    pub const fn file(&self) -> &File {
        &self.file
    }
}

/// Open the lock file and take the platform's exclusive hold on it, without blocking.
#[cfg(unix)]
fn open_exclusive(path: &Path) -> io::Result<File> {
    use rustix::fs::{FlockOperation, flock};

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    // Non-blocking on purpose: a daemon that *waited* for the lock would look like a slow start
    // rather than a refusal, and "a second daemon on the same directory is refused" is the thing
    // being asserted.
    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|errno| {
        let error = io::Error::from(errno);
        if error.kind() == io::ErrorKind::WouldBlock {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "another daemon already holds the single-writer lock on {}",
                    path.display()
                ),
            )
        } else {
            error
        }
    })?;
    Ok(file)
}

/// Windows has no `flock`; the sharing mode is the same guarantee reached the other way.
#[cfg(windows)]
fn open_exclusive(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    // `share_mode(0)`: no other handle may be opened on this file at all. A second daemon's open
    // fails with a sharing violation, which is reported as `PermissionDenied` rather than
    // `WouldBlock` — so callers match on "the take failed", never on the kind.
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "another daemon may already hold the single-writer lock on {}: {error}",
                    path.display()
                ),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant 1: one writer. A second take on the same directory is refused while the first is
    /// held, and succeeds once it is released — the release arm is the control, without which a
    /// `take` that always failed would pass.
    #[test]
    fn a_second_take_is_refused_while_the_first_is_held() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = StateLock::take(dir.path()).expect("control: the first take succeeds");
        assert!(
            first.path().exists(),
            "the lock file is where it says it is"
        );

        let second = StateLock::take(dir.path());
        assert!(
            second.is_err(),
            "a second daemon on the same state directory is refused"
        );

        drop(first);
        StateLock::take(dir.path()).expect("control: the lock is released when it is dropped");
    }

    /// A different directory is a different lock: the refusal is about the state directory and not
    /// about the process. Without this arm a `take` that refused every second call would pass the
    /// test above.
    #[test]
    fn two_state_directories_are_two_locks() {
        let one = tempfile::tempdir().expect("tempdir");
        let two = tempfile::tempdir().expect("tempdir");
        let _first = StateLock::take(one.path()).expect("the first directory");
        let _second = StateLock::take(two.path()).expect("a different directory is a second lock");
    }
}
