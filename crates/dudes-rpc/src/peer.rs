//! Reading what the operating system reports about the peer of a Unix socket.
//!
//! `std::os::unix::net::UnixStream::peer_cred` is unstable on the pinned toolchain
//! (`peer_credentials_unix_socket`, rust-lang/rust#42839), and this workspace forbids `unsafe`
//! outside two data-plane crates, so the `getsockopt` calls come from `nix`, which does the
//! `unsafe` internally and exposes a safe `fn`.
//!
//! **This module reports; it does not conclude.** Nothing here decides who a peer is. See
//! [`PeerIdentity`] for why the pid in particular is not an identity on macOS, and read that before
//! building anything on the value this module returns.
//!
//! A failed read is [`PeerPid::Unreadable`], the only process-read failure value. Under
//! default-deny it is the least privileged answer, so a platform that cannot answer falls towards
//! refusal rather than trust — and `system.ping`, the one method a peer that proved nothing may
//! call, keeps working so the daemon stays probeable.

use std::os::unix::net::UnixStream;

use crate::auth::{PeerIdentity, PeerPid};

/// What the kernel reports about the peer at the other end of `stream`.
#[must_use]
pub fn identity(stream: &UnixStream) -> PeerIdentity {
    resolve(stream)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn resolve(stream: &UnixStream) -> PeerIdentity {
    use nix::sys::socket::{getsockopt, sockopt};

    match getsockopt(stream, sockopt::PeerCredentials) {
        // `unix(7)`: `SO_PEERCRED` returns the credentials "in effect at the time of the call to
        // `connect(2)`" — captured once and immutable for the life of the socket. That is a
        // stronger guarantee than the macOS arm below has, and the difference is the reason this
        // module is written per platform rather than once.
        Ok(cred) => PeerIdentity {
            uid: Some(cred.uid()),
            gid: Some(cred.gid()),
            // No `pidversion` here: it is macOS's, and `SO_PEERCRED` is captured at `connect(2)`
            // rather than live, so the recycling window the token option closes is narrower on
            // this platform to begin with.
            pid: PeerPid::Reported {
                pid: cred.pid(),
                pidversion: None,
            },
        },
        Err(errno) => {
            tracing::warn!(%errno, "could not read peer credentials; the peer stays unknown");
            PeerIdentity::unknown()
        }
    }
}

/// Index of the pid inside an `audit_token_t`, as `bsm/libbsm.h` defines `audit_token_to_pid`:
/// `((pid_t)((atoken).val[5]))`.
///
/// `nix` documents the token's layout as opaque and points at the BSM routines instead — and macOS
/// 11 deprecated almost all of those, which is why this reads the field directly. The constant is
/// not taken on trust: [`tests::the_audit_token_pid_is_this_process`] asserts the value against
/// `std::process::id()` over a real connection, so a layout change fails the suite rather than
/// silently returning someone else's pid.
#[cfg(target_os = "macos")]
const AUDIT_TOKEN_PID_INDEX: usize = 5;

/// Index of the euid inside an `audit_token_t` (`audit_token_to_euid`, `val[1]`).
///
/// Read only by [`tests::the_audit_token_layout_is_pinned_at_two_independent_indices`], which is
/// what stops [`AUDIT_TOKEN_PIDVERSION_INDEX`] from being a guess: pinning two *different* fields
/// against two *different* oracles — this one against `LOCAL_PEERCRED`, the pid against
/// `std::process::id()` — fixes the struct's field order, and the pidversion is the field the same
/// header places after them.
#[cfg(all(target_os = "macos", test))]
const AUDIT_TOKEN_EUID_INDEX: usize = 1;

/// Index of the pidversion inside an `audit_token_t` (`audit_token_to_pidversion`, `val[7]`).
///
/// **Weaker than the pid's pinning, and named as such.** There is no in-process oracle for a
/// pidversion the way `std::process::id()` is one for a pid, so what the test can pin is the
/// *layout* around it (see [`AUDIT_TOKEN_EUID_INDEX`]) rather than the value. The value is carried
/// and, today, compared against nothing: the adoption key records no pidversion, so there
/// is nothing to compare it to until that type grows a field.
#[cfg(target_os = "macos")]
const AUDIT_TOKEN_PIDVERSION_INDEX: usize = 7;

#[cfg(target_os = "macos")]
fn resolve(stream: &UnixStream) -> PeerIdentity {
    use nix::sys::socket::{getsockopt, sockopt};

    // `LOCAL_PEERCRED` is cached under XNU's `UNP_HAVEPC` flag: the uid is captured, and it is the
    // trustworthy half of what this call returns.
    let (uid, gid) = match getsockopt(stream, sockopt::LocalPeerCred) {
        Ok(cred) => (Some(cred.uid()), cred.groups().first().copied()),
        Err(errno) => {
            tracing::warn!(%errno, "could not read the peer's uid");
            (None, None)
        }
    };

    // `LOCAL_PEERTOKEN` rather than `LOCAL_PEERPID`: the token option returns `EINVAL` for a dead
    // identifier where the pid options return one silently, and XNU recycles identifiers — so a
    // walk that started from a pid option could begin at a recycled identifier and produce a
    // silent false match. Narrowing the input does not make the value an identity: it is still a
    // live field the peer can move, and reading it at accept does not fix that (see
    // [`PeerIdentity`]).
    //
    // `EINVAL` here is the expected case: the identifier named no live process. It resolves to
    // `Unreadable`, which a resolver turns into the third classification column — never an error and never a
    // match.
    let pid = match getsockopt(stream, sockopt::LocalPeerToken) {
        Ok(token) => {
            i32::try_from(token.val[AUDIT_TOKEN_PID_INDEX]).map_or(PeerPid::Unreadable, |pid| {
                PeerPid::Reported {
                    pid,
                    pidversion: Some(token.val[AUDIT_TOKEN_PIDVERSION_INDEX]),
                }
            })
        }
        Err(errno) => {
            tracing::warn!(%errno, "could not read the peer's audit token");
            PeerPid::Unreadable
        }
    };

    PeerIdentity { uid, gid, pid }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn resolve(_stream: &UnixStream) -> PeerIdentity {
    // Every other Unix: no peer-credential path is implemented, so the peer is unknown and
    // default-deny refuses everything but the liveness probe. Stated as code rather than left to a
    // compile error, so a build on such a platform fails closed instead of failing to build.
    //
    // `PeerIdentity::unknown()` is `PeerPid::Unreadable`, which is the right answer and not merely
    // the convenient one: this is still a Unix socket with a process at the far end, and the door
    // that has no process is a different transport entirely.
    tracing::warn!("no peer-credential path on this platform; the peer stays unknown");
    PeerIdentity::unknown()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A socket pair whose peer is this process.
    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("a socket pair")
    }

    /// The uid this module reports is a real uid, and both ends of a pair agree on it — the
    /// control that the `getsockopt` is reading the socket rather than returning a zeroed struct.
    #[test]
    fn both_ends_of_a_pair_report_the_same_uid() {
        let (a, b) = pair();
        let left = identity(&a);
        let right = identity(&b);
        assert!(left.uid.is_some(), "a uid was read: {left:?}");
        assert_eq!(left.uid, right.uid, "the peer of each end is this process");
    }

    /// The pid this module reports for a peer that *is* this process equals this process's pid.
    ///
    /// On macOS this is what pins [`AUDIT_TOKEN_PID_INDEX`] to the field `audit_token_to_pid`
    /// reads: a wrong index would return some other word of the token, and the equality below
    /// would fail rather than quietly hand a caller a number that means nothing.
    ///
    /// It also pins the variant: a live socket peer is `Reported`; a failed read is `Unreadable`.
    #[test]
    fn the_audit_token_pid_is_this_process() {
        let (a, _b) = pair();
        let peer = identity(&a);
        let expected = i32::try_from(std::process::id()).expect("a pid fits in i32");
        assert_eq!(
            peer.pid.reported(),
            Some(expected),
            "the peer of this socket is this process"
        );
    }

    /// The `audit_token_t` field order is pinned at **two** independent indices against **two**
    /// independent oracles, which is what makes the pidversion index a reading of a known layout
    /// rather than a guess.
    ///
    /// `val[5]` is checked against `std::process::id()` above; `val[1]` is checked here against
    /// the uid `LOCAL_PEERCRED` reports, which is a different `getsockopt` on a different option.
    /// Two matching fields at two known offsets fix the struct's order; `audit_token_to_pidversion`
    /// is `val[7]` in the same header. If Apple ever reorders the token, this test fails before a
    /// caller is handed a pidversion that is some other word.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_audit_token_layout_is_pinned_at_two_independent_indices() {
        use nix::sys::socket::{getsockopt, sockopt};

        let (a, _b) = pair();
        let token = getsockopt(&a, sockopt::LocalPeerToken).expect("the peer's audit token");
        let cred = getsockopt(&a, sockopt::LocalPeerCred).expect("the peer's credentials");

        assert_eq!(
            token.val[AUDIT_TOKEN_PID_INDEX],
            std::process::id(),
            "val[5] is the pid"
        );
        assert_eq!(
            token.val[AUDIT_TOKEN_EUID_INDEX],
            cred.uid(),
            "val[1] is the euid, per a different option on the same socket"
        );
        assert_ne!(
            token.val[AUDIT_TOKEN_PIDVERSION_INDEX], 0,
            "a live process has a nonzero pidversion; the value is carried, not yet compared"
        );
        // Not a positive control — it compares two constants and can only fail if someone edits
        // one of them. It is kept as a guard with a stated job: if the two indices were ever
        // collapsed onto each other, the pid assertion above would pass *and* a caller would be
        // handed the pid as a pidversion, which is the one way that assertion can be satisfied by
        // a wrong reading. There is no independent oracle for `val[7]` on this platform, and the
        // absence of one is the reason this file's claim about the pidversion is a reading of a
        // known layout rather than a measurement.
        assert_ne!(
            AUDIT_TOKEN_PIDVERSION_INDEX, AUDIT_TOKEN_PID_INDEX,
            "the two indices this file reads must stay different words of the token"
        );
    }

    /// The pidversion rides along on macOS and is absent everywhere else — so a caller that
    /// eventually compares it can tell "this platform has none" from "this one was not read".
    #[test]
    fn a_reported_pid_carries_a_pidversion_only_where_the_platform_has_one() {
        let (a, _b) = pair();
        let PeerPid::Reported { pidversion, .. } = identity(&a).pid else {
            panic!("a socket pair's peer is a live process");
        };
        assert_eq!(pidversion.is_some(), cfg!(target_os = "macos"));
    }
}
