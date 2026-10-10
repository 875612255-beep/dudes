//! Two facts about the control socket's *path* that the transport deliberately does not own: what
//! to do with the files a dead daemon left there, and whether the socket standing there now is
//! still the one this daemon bound.
//!
//! # Reclaiming a dead run's files, and why it happens inside the bind step
//!
//! `LocalTransport::bind` creates and never removes, and `bind_in_with_credential` is
//! `O_CREAT|O_EXCL`. Both are right: a transport that unlinked whatever stood at its path would be
//! a transport that could take a live daemon's socket. But together they mean a daemon that exits
//! *uncleanly* leaves a socket and a credential behind, and the next start is refused by both —
//! **so a crashed daemon cannot restart**, which is exactly what a crash test does.
//!
//! On macOS, dropping a `UnixListener` without unlinking
//! leaves the file present and the second bind fails `Address already in use (os error 48)`; the
//! same second bind into a fresh path succeeds.
//!
//! The reclaim is **not an eighth startup step**.
//! The seven-step order is normative — a test pins it — and adding a step to
//! a normative order is a larger change than this fact needs. What the fact actually is, is a
//! precondition of *binding*: step 4 is "bind the control socket", and a path holding a dead run's
//! socket is a path that is not yet bindable. So it lives inside step 4, before the bind, where the
//! single-writer lock is already held.
//!
//! # What authorises the unlink, since the lock does not
//!
//! The lock is on the **state** directory and the socket is in the **runtime** directory, which is a
//! separate path a caller may point elsewhere — so holding the lock proves nothing about who owns
//! this socket. What proves it is the socket itself: a Unix socket file with no process listening
//! refuses `connect(2)` with `ECONNREFUSED`, and one with a live daemon behind it accepts. So the
//! reclaim asks, and:
//!
//! - **anything that is not a socket** — a regular file, and a symbolic link above all — is refused
//!   and never unlinked. This code removes only what it has positively identified as a dead
//!   daemon's socket, which is the narrowest thing that closes the restart hole;
//! - **a socket that accepts a connection** is a live daemon, and is refused;
//! - **a socket that refuses one** is a dead run's leftover, and it and the credential minted
//!   beside it are removed.
//!
//! # The identity re-comparison
//!
//! A same-uid peer may `unlink` this socket and `bind` its own at the same path at
//! any moment after the bind, with **no symlink involved**. Established connections survive, so the
//! daemon observes nothing while new clients reach the other process, and the daemon cannot
//! self-test by comparing `fstat(listener_fd)` against `lstat(path)` — they do not match.
//!
//! What it *can* do is remember `(st_dev, st_ino)` from the post-bind `lstat` and compare that
//! value against the path later. That detects a squat late rather than preventing it, and late
//! detection is strictly more than today's silence.
//!
//! **It raises no breaker rung.** A rung
//! is raised by an `Input::Signal { agent, .. }`; a squatted control socket is a daemon-wide fact
//! with no agent to attach it to, and no `RuntimeSignal` or `BreakerSignal` variant names it —
//! `BreakerSignal` being a wire type, adding one would change the published schemas. So this logs loudly and raises none, and invariant 3's constraint — an `Inferred`
//! event may never be the sole cause of `Stop` or `Done` — is what a test names.

use std::io;
use std::path::Path;

// Read only inside `#[cfg(unix)]` items — the Unix `reclaim_stale` and its test — so the import
// carries the same gate: the Windows arm below reclaims nothing and names nothing, and an import
// nothing reads is a warning on that target.
#[cfg(unix)]
use dudes_rpc::OPERATOR_CREDENTIAL_NAME;

/// What a re-comparison found at the socket's path.
///
/// Four answers rather than a `bool`, for the reason `Liveness` has three: "the file is gone",
/// "something else is there" and "this daemon could not tell" are different facts, and collapsing
/// them would let an unreadable path read as a squat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The same socket this daemon bound.
    Same,
    /// A different file stands at the path: someone unlinked the socket and bound their own.
    Replaced,
    /// Nothing stands at the path any more.
    Gone,
    /// The path could not be read.
    Unreadable,
}

/// The identity of the socket this daemon bound: device and inode, as read immediately after the
/// bind was verified.
///
/// Unix only. `std` exposes no device or inode on Windows, and a named pipe has no path in the
/// filesystem to be squatted at, so the type is `cfg`-gated rather than stubbed — a stub would
/// answer [`Standing::Same`] for ever on a platform where nothing had been checked.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketIdentity {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
impl SocketIdentity {
    /// Read the identity of whatever stands at `path`.
    ///
    /// `symlink_metadata` — `lstat(2)` — never `metadata`: the question is about the file at this
    /// path, and following the last component would answer about a different one.
    ///
    /// # Errors
    ///
    /// Whatever `lstat(2)` returns.
    pub fn of(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = std::fs::symlink_metadata(path)?;
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    /// Compare `path` against this identity now.
    #[must_use]
    pub fn standing(&self, path: &Path) -> Standing {
        match Self::of(path) {
            Ok(now) if now == *self => Standing::Same,
            Ok(_) => Standing::Replaced,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Standing::Gone,
            Err(_) => Standing::Unreadable,
        }
    }
}

/// Remove a dead daemon's socket and credential from `dir`, or refuse.
///
/// Called immediately before the bind, inside startup step 4. See this module's header for what
/// authorises the removal and what is deliberately never removed.
///
/// # Errors
///
/// [`io::ErrorKind::AlreadyExists`] when a live daemon is listening, or when what stands at the
/// socket path is anything but a socket — including a symbolic link, which is refused by name and
/// never followed. Whatever `lstat(2)` or the unlink returns otherwise.
#[cfg(unix)]
pub fn reclaim_stale(dir: &Path, socket_name: &str) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt as _;
    use std::os::unix::net::UnixStream;

    let path = dir.join(socket_name);
    let file_type = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata.file_type(),
        // Nothing there: the ordinary first start, and the ordinary case after a clean exit.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !file_type.is_socket() {
        let what = if file_type.is_symlink() {
            "a symbolic link"
        } else {
            "a file that is not a socket"
        };
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "refusing to reclaim {}: {what} stands there, and only a socket this daemon can \
                 show is dead is ever removed",
                path.display()
            ),
        ));
    }
    match UnixStream::connect(&path) {
        Ok(stream) => {
            // Closed at once: this was a liveness question, not a call.
            drop(stream);
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to reclaim {}: a daemon is listening there",
                    path.display()
                ),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            tracing::warn!(
                socket = %path.display(),
                "reclaiming the socket and credential a previous daemon left behind"
            );
            std::fs::remove_file(&path)?;
            // The credential is minted in the same call that binds the socket, so a leftover socket
            // means a leftover credential, and `bind_in_with_credential` is `create_new`. Removing
            // one without the other would trade this refusal for the next one.
            let credential = dir.join(OPERATOR_CREDENTIAL_NAME);
            match std::fs::remove_file(&credential) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// Nothing to reclaim where there is no socket file: a Windows daemon answers on a named pipe,
/// which has no filesystem path a dead run can leave behind.
#[cfg(not(unix))]
#[allow(clippy::missing_const_for_fn)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the cfg-free bind path keeps one fallible reclaim signature on both transports"
)]
pub fn reclaim_stale(_dir: &Path, _socket_name: &str) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    /// A dead run's socket is reclaimed, and a *live* daemon's is refused. Both arms, because
    /// "removes everything" and "removes nothing" are both wrong and one arm cannot tell them
    /// apart.
    #[test]
    fn a_dead_runs_socket_is_reclaimed_and_a_live_ones_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("duded.sock");
        let credential = dir.path().join(OPERATOR_CREDENTIAL_NAME);

        // Control: with a listener bound, the path is a live daemon's and is refused.
        let listener = UnixListener::bind(&path).expect("bind");
        let error = reclaim_stale(dir.path(), "duded.sock").expect_err("a live daemon is refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{error}");
        assert!(path.exists(), "and nothing was removed");

        // Dropping the listener is what a crashed daemon does: the file stays, nothing listens.
        drop(listener);
        std::fs::write(&credential, "x").expect("write a leftover credential");
        assert!(
            path.exists(),
            "control: the dead run's socket is still there"
        );

        reclaim_stale(dir.path(), "duded.sock").expect("a dead run's socket is reclaimed");
        assert!(!path.exists(), "the socket was removed");
        assert!(
            !credential.exists(),
            "and the credential minted beside it went with it"
        );
    }

    /// Only a socket is ever removed. A regular file and a symbolic link are both refused and both
    /// left standing — the narrow rule this module's header argues for.
    #[test]
    fn nothing_that_is_not_a_socket_is_ever_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("duded.sock");

        std::fs::write(&path, "not a socket").expect("write");
        let error = reclaim_stale(dir.path(), "duded.sock").expect_err("a plain file is refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{error}");
        assert!(path.exists(), "and it is still there");
        std::fs::remove_file(&path).expect("remove");

        std::os::unix::fs::symlink("/nowhere", &path).expect("symlink");
        let error = reclaim_stale(dir.path(), "duded.sock").expect_err("a link is refused");
        assert!(
            error.to_string().contains("a symbolic link"),
            "refused for the right reason: {error}"
        );
        assert!(
            std::fs::symlink_metadata(&path).is_ok(),
            "a dangling link is left exactly where it was"
        );
    }

    /// An empty runtime directory reclaims nothing and succeeds: the ordinary first start.
    #[test]
    fn an_empty_runtime_directory_is_nothing_to_reclaim() {
        let dir = tempfile::tempdir().expect("tempdir");
        reclaim_stale(dir.path(), "duded.sock").expect("nothing there is not an error");
    }

    /// The identity is the file at the path, and a *replacement* at the same path reads
    /// `Replaced` — which is the whole clause. `Same` before it is the control: without it a
    /// `standing` that answered `Replaced` unconditionally would pass.
    #[test]
    fn a_socket_replaced_at_the_same_path_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("duded.sock");
        let mine = UnixListener::bind(&path).expect("bind");
        let identity = SocketIdentity::of(&path).expect("read the identity");
        assert_eq!(
            identity.standing(&path),
            Standing::Same,
            "control: an unmolested socket reads Same"
        );

        // The squat, with no symlink anywhere: unlink the live socket and bind another.
        std::fs::remove_file(&path).expect("unlink");
        assert_eq!(
            identity.standing(&path),
            Standing::Gone,
            "between the unlink and the rebind there is nothing there"
        );
        let theirs = UnixListener::bind(&path).expect("rebind");
        assert_eq!(
            identity.standing(&path),
            Standing::Replaced,
            "a different inode at the same path is a squat"
        );
        drop(theirs);
        drop(mine);
    }
}
