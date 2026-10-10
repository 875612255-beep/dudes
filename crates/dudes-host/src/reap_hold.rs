//! A test-only hold on one child's reap, and the one thing that may lift it.
//!
//! **Why this exists.** The defect it covers lives in a window nothing in this crate could
//! previously open on purpose: the child has exited, the watcher has not yet called `wait`, and a
//! `kill` running at that instant asks the platform about a process group whose only member is an
//! unreaped zombie. On Darwin that query answers `EPERM`. Holding the window open with a sleep
//! reproduces it — that is what the diagnosis instrument did — but a sleep-shaped test is an
//! intermittent test, so the window here is opened by a latch and closed by an observation.
//!
//! **Why the observation has to come from the production query.** The obvious releaser is the test
//! itself, polling the group until it answers `EPERM` and then letting the watcher go. That loses
//! the race it is trying to create: the test's poll sees the zombie microseconds after the child
//! dies, while `kill::wait_until_quiet` re-examines the target only every `POLL`. The reap would
//! land inside that gap and the kill's next query would see `ESRCH` — a green run that proves
//! nothing, on the unfixed tree as much as on the fixed one. So the release is caused by the path
//! under test: [`release_for`] is called from `unix::presence_checked`, on the errno, and a hold
//! that says it was released that way is a hold whose child was a zombie *while the kill was
//! looking at it*.
//!
//! **The errno, not the classification.** The hook keys on `Err(EPERM)` from the platform rather
//! than on [`Presence`](crate::unix::Presence), so it behaves identically before and after the
//! classification change and the same test compiles and runs on both. That is what makes the
//! red-before honest rather than a different test.
//!
//! Nothing in this module — and no parameter, field or call that reaches it — exists outside
//! `cfg(test)`.

#[cfg(target_os = "macos")]
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a hold waits for its release before giving up.
///
/// A failsafe and not a timeout anyone should reach: a run that reaches it has already failed, and
/// the point of the bound is that it fails by assertion instead of hanging a suite. It is long
/// enough that a loaded machine cannot reach it while the mechanism is working, because the thing
/// it is waiting for is one `kill` away and not one schedule away.
const FAILSAFE: Duration = Duration::from_secs(30);

/// Why a hold stopped holding. The distinction is the evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Release {
    /// The production presence query answered `EPERM` for this child's group, so the kill was
    /// looking at the target while it was an unreaped zombie.
    #[cfg(target_os = "macos")]
    PresenceEperm,
    /// The production **delivered** group sweep answered `EPERM` for this child's group, so the
    /// signal itself reached a group holding nothing but an unreaped zombie.
    ///
    /// Its own variant rather than a reuse of [`Release::PresenceEperm`]: they name different
    /// observations made by different syscalls at different points of the escalation, and a test
    /// that cannot tell them apart cannot say which window it entered. Both are keyed on the
    /// platform's errno rather than on any classification, so an assertion naming one of them is an
    /// assertion about what the kernel answered.
    ///
    /// Gated with its producer: only `unix::signal_group`'s Darwin hook sets it, so on any other
    /// target this would be a variant nothing constructs.
    #[cfg(target_os = "macos")]
    GroupSignalEperm,
    /// A test-only cleanup adapter let it go at a named point in the cleanup it is driving.
    ///
    /// The third arm of what this module is for, and the only platform-neutral one. The two above
    /// open a window and wait for production code to walk into it, each keyed on an errno; this one
    /// holds an ordering *closed* until the step that depends on it has run, and is released by
    /// name rather than by observation. An assertion naming it says a test-side step ran, which is
    /// why the two errno-keyed reasons exist separately.
    Explicit,
    /// Nothing released it within [`FAILSAFE`]. The window was never entered, and a test that
    /// accepts this reason is not testing the window.
    Failsafe,
}

/// The shared half of a hold.
#[derive(Debug, Default)]
struct Inner {
    /// `None` while the hold is holding.
    state: Mutex<Option<Release>>,
    /// Signalled by [`release_for`] and by the failsafe path.
    woken: Condvar,
}

/// A hold on one child's reap, shared between the test that arms it, the watcher that waits on it
/// and the presence query that releases it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hold(Arc<Inner>);

/// Every armed hold, by the pid of the child whose reap it holds.
///
/// Keyed rather than a single global because `cargo test` runs this crate's unit tests as threads
/// in one process: an unkeyed hold would be released by any other test's child that happened to
/// answer `EPERM`. Every child this host spawns is its own group leader (`unix::prepare`), so the
/// pid the presence query carries is also the group's name and one key serves both.
#[cfg(target_os = "macos")]
static HOLDS: Mutex<BTreeMap<u32, Hold>> = Mutex::new(BTreeMap::new());

impl Hold {
    /// Register this hold as the hold on `pid`'s reap.
    ///
    /// Called from `RealHost::spawn` once the child exists, which is before any caller can reach
    /// `kill`, so the registration cannot lose to the query that releases it.
    #[cfg(target_os = "macos")]
    pub(crate) fn arm(&self, pid: u32) {
        HOLDS
            .lock()
            .expect("the armed-hold table")
            .insert(pid, self.clone());
    }

    /// Block until this hold is released, or until [`FAILSAFE`] expires.
    pub(crate) fn wait(&self) {
        let deadline = Instant::now() + FAILSAFE;
        let mut state = self.0.state.lock().expect("a held reap's state");
        while state.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                *state = Some(Release::Failsafe);
                break;
            }
            let (next, _) = self
                .0
                .woken
                .wait_timeout(state, remaining)
                .expect("a held reap's state");
            state = next;
        }
    }

    /// Let this hold go, by name, from the step whose ordering it was protecting.
    ///
    /// Idempotent and safe to call when nothing is waiting: an ordering seam is released from a
    /// path that may run more than once, or not at all, and a release that only worked once would
    /// make the seam's correctness depend on the shape of the code it is measuring.
    pub(crate) fn release(&self) {
        let mut state = self.0.state.lock().expect("a held reap's state");
        if state.is_none() {
            *state = Some(Release::Explicit);
        }
        self.0.woken.notify_all();
    }

    /// Why this hold stopped holding, or `None` while it still holds.
    pub(crate) fn release_reason(&self) -> Option<Release> {
        *self.0.state.lock().expect("a held reap's state")
    }
}

/// Release the hold on `pid`, if one is armed, because the presence query answered `EPERM`.
///
/// The entry is taken rather than read, so the release happens once and a later child that reuses
/// the identifier finds nothing armed.
#[cfg(target_os = "macos")]
pub(crate) fn release_for(pid: u32) {
    release_with(pid, Release::PresenceEperm);
}

/// Release the hold on `pid`, if one is armed, because the **delivered group sweep** answered
/// `EPERM` — the site this hold exists for.
///
/// A second entry point rather than a parameter on the first, so each call site reads as the one
/// observation it makes and neither can pass the other's reason by accident.
#[cfg(target_os = "macos")]
pub(crate) fn release_for_group_signal(pid: u32) {
    release_with(pid, Release::GroupSignalEperm);
}

/// The release both errno-keyed sites share, once the observation has been named.
///
/// Gated with the registry it reads: `Hold::release` is the platform-neutral path and touches no
/// table, because an ordering released by name knows which hold it is releasing.
#[cfg(target_os = "macos")]
fn release_with(pid: u32, reason: Release) {
    let Some(hold) = HOLDS.lock().expect("the armed-hold table").remove(&pid) else {
        return;
    };
    let mut state = hold.0.state.lock().expect("a held reap's state");
    if state.is_none() {
        *state = Some(reason);
    }
    hold.0.woken.notify_all();
}
