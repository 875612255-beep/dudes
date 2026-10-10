//! macOS: the BSD process info block's creation time as the start value, and no boot identity.
//!
//! **No `unsafe` in this file, and `tests/unsafe_grant.rs` is what says so.** `libproc` wraps
//! `proc_pidinfo` safely, and the signals go through `rustix`.
//!
//! # The start value
//!
//! `BSDInfo::pbi_start_tvsec` and `pbi_start_tvusec` are the process's creation time as the kernel
//! recorded it at creation: a wall-clock anchor, not an offset from boot, so it needs no boot
//! identity beside it and [`AdoptionKey::boot`](dudes_supervisor::host::AdoptionKey::boot) is
//! `None` on this platform. The two halves are packed into one integer at microsecond resolution
//! and compared as read. Packing is not converting — no arithmetic is done against a clock, no
//! epoch is chosen, and the number is never rendered as a time. It exists to be compared with an
//! earlier reading of itself.
//!
//! # The one window this platform does not close
//!
//! macOS offers nothing that reserves an identifier the way a pidfd or a process handle does. On
//! the adoption path, therefore, the interval between reading the start value and sending the
//! signal is open. It is narrowed to what it can be narrowed to: the probe and the signal are two
//! adjacent statements inside [`crate::kill::escalate`] with no allocation, no lock and nothing
//! awaited between them. Named here rather than left to be discovered.

use std::io;
use std::process::{Child, Command};

use dudes_supervisor::host::ProbeAnswer;
use libproc::bsd_info::BSDInfo;
use libproc::proc_pid::{pidinfo, validate_pidversion};
use rustix::io::Errno;
use rustix::process::Pid;

use crate::kill::{Boundary, Delivery, Sig, TargetState};
use crate::peer::{Containment, Process, Read, Source};
use crate::{Spawned, unix};

/// The process-table view used by one listener-captured peer execution walk.
pub(crate) struct PeerSource {
    starting: Option<(u32, u32)>,
}

impl PeerSource {
    pub(crate) const fn new(pid: u32, pidversion: u32) -> Self {
        Self {
            starting: Some((pid, pidversion)),
        }
    }
}

impl Source for PeerSource {
    fn process(&mut self, pid: u32) -> Read<Process> {
        // Buffer the numeric-pid row first. Validation runs second, and this starting marker is
        // consumed before returning so no later walk step can read the starting pid again.
        let buffered = read_process(pid);
        let Some((starting_pid, pidversion)) = self.starting else {
            return buffered;
        };
        if pid != starting_pid {
            return buffered;
        }
        self.starting = None;
        let Read::Found(process) = buffered else {
            return buffered;
        };
        match validate_pidversion(pid, pidversion) {
            Ok(true) => Read::Found(process),
            Ok(false) | Err(_) => Read::Refused,
        }
    }

    fn containment(&mut self, _peer: u32, _root: u32) -> Read<Containment> {
        Read::Found(Containment::NotAvailable)
    }
}

fn read_process(pid: u32) -> Read<Process> {
    let Ok(pid_i32) = i32::try_from(pid) else {
        return Read::Refused;
    };
    if let Ok(info) = pidinfo::<BSDInfo>(pid_i32, 0) {
        Read::Found(Process {
            parent: info.pbi_ppid,
            start_time: pack(info.pbi_start_tvsec, info.pbi_start_tvusec),
            boot: None,
        })
    } else {
        let existence = unix::as_pid(pid).map(rustix::process::test_kill_process);
        match existence {
            Some(Err(Errno::SRCH)) => Read::Missing,
            _ => Read::Refused,
        }
    }
}

/// Witness that the command requested its own group. Consumed only after that command successfully
/// spawns, while its child is still owned and unreaped; adoption cannot construct this witness.
pub(crate) struct Prep {
    _group_requested: (),
}

/// What the host holds for one identifier on macOS: the group to sweep, and nothing else. The
/// absence of a leader reference is the platform's, not an omission — see the module docs.
pub(crate) struct Grip {
    /// The group established at owned spawn, or independently resolved for an adopted leader.
    group: Option<Pid>,
}

pub(crate) const fn exit_group(grip: &Grip) -> Option<Pid> {
    grip.group
}

/// macOS start values are captured at creation and mean the same thing across a reboot, so keys
/// from this host carry no boot identity.
// Uniform across the three platform layers on purpose: this shim cannot fail here and can on
// another, and one call site in `lib.rs` reads all three. Collapsing the signature per platform
// would move the `cfg` from the layer into the caller.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn boot_identity() -> io::Result<Option<String>> {
    Ok(None)
}

/// Make the child a group leader. See [`unix::prepare`].
// Uniform across the three platform layers on purpose: this shim cannot fail here and can on
// another, and one call site in `lib.rs` reads all three. Collapsing the signature per platform
// would move the `cfg` from the layer into the caller.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn prepare(command: &mut Command) -> io::Result<Prep> {
    unix::prepare(command);
    Ok(Prep {
        _group_requested: (),
    })
}

/// The grip on a child this host has just spawned and holds unreaped.
///
/// Successful prepared spawn established PGID == child PID. macOS can answer ESRCH after exit but
/// before reap; that loses the live lookup, not the group already created. A contradictory live
/// group or another query error still withholds the group and reports weakened ownership.
pub(crate) fn grip_spawned(prep: Prep, child: &Child) -> Spawned {
    let group = unix::as_pid(child.id())
        .and_then(|pid| spawned_group(prep, pid, rustix::process::getpgid(Some(pid))));
    let weakened = group
        .is_none()
        .then(|| io::Error::other("the spawned child is not the leader of its process group"));
    Spawned {
        grip: Grip { group },
        boundary: if group.is_some() {
            Boundary::ProcessGroup
        } else {
            Boundary::LeaderOnly
        },
        weakened,
    }
}

fn spawned_group(_prep: Prep, pid: Pid, observed: rustix::io::Result<Pid>) -> Option<Pid> {
    match observed {
        Ok(group) if group == pid => Some(group),
        Err(Errno::SRCH) => Some(pid),
        Ok(_) | Err(_) => None,
    }
}

#[cfg(test)]
pub(crate) fn grip_spawned_without_boundary_for_test(_prep: Prep, _child: &Child) -> Spawned {
    Spawned {
        grip: Grip { group: None },
        boundary: Boundary::LeaderOnly,
        weakened: Some(io::Error::other(
            "the spawned child is not the leader of its process group",
        )),
    }
}

/// The grip on an identifier this host did not spawn.
pub(crate) fn grip_adopted(pid: u32) -> Grip {
    Grip {
        group: unix::group_of_leader(pid),
    }
}

/// The creation time of `pid`, packed.
///
/// `proc_pidinfo` reports failure as a message rather than an errno, so a failed read cannot be
/// told apart from an absent identifier by its own answer. Rather than give that up — which would
/// cost this platform positive absence, and with it every `Gone` a reaper could observe — the
/// question is put a second way: signal 0 delivers nothing and runs the permission and existence
/// checks, and `ESRCH` from it is the process table saying there is no such identifier.
///
/// That second question is **not** the guard before a signal and must never be used as one: it
/// answers "is something there", and the guard needs "is it the same one". It is used here, inside
/// the probe, only to tell an absence apart from an unreadable answer.
pub(crate) fn probe(_grip: &Grip, pid: u32) -> ProbeAnswer {
    probe_with_argument(pid, 0)
}

/// Read a child while its owner still holds it unreaped. The BSD info query accepts argument 1
/// for an exited child, whereas the liveness probe keeps argument 0.
pub(crate) fn unreaped_start_value(_grip: &Grip, pid: u32) -> ProbeAnswer {
    probe_with_argument(pid, 1)
}

fn probe_with_argument(pid: u32, argument: u64) -> ProbeAnswer {
    let Ok(pid_i32) = i32::try_from(pid) else {
        return ProbeAnswer::CouldNotAnswer;
    };
    match pidinfo::<BSDInfo>(pid_i32, argument) {
        Ok(info) => ProbeAnswer::StartValue(pack(info.pbi_start_tvsec, info.pbi_start_tvusec)),
        Err(_) => match unix::as_pid(pid).map(rustix::process::test_kill_process) {
            Some(Err(Errno::SRCH)) => ProbeAnswer::NoSuchPid,
            _ => ProbeAnswer::CouldNotAnswer,
        },
    }
}

/// The two halves of the creation time as one opaque integer, microseconds wide.
///
/// Saturating, not wrapping and not truncating: a value that overflowed would compare equal to
/// other overflowed values and manufacture an `Alive`. `pbi_start_tvsec` is seconds since the
/// epoch, so the multiplication has roughly three hundred thousand years of headroom and the
/// saturation is a guard rather than a behaviour.
const fn pack(seconds: u64, micros: u64) -> u64 {
    seconds.saturating_mul(1_000_000).saturating_add(micros)
}

/// Signal the leader by identifier, then sweep its group.
///
/// There is no stronger reference to use here. What makes this safe is not the mechanism but the
/// comparison that ran immediately before it, and the reservation on the same-run path: a child
/// this host has not reaped cannot have had its identifier reused.
pub(crate) fn signal(grip: &Grip, pid: u32, sig: Sig) -> io::Result<()> {
    signal_leader(grip, pid, sig)?;
    signal_boundary(grip, sig).map(drop)
}

/// Signal the direct leader independently of its process group.
pub(crate) fn signal_leader(_grip: &Grip, pid: u32, sig: Sig) -> io::Result<Delivery> {
    let pid = unix::as_pid(pid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a process identifier"))?;
    unix::classify_signal(rustix::process::kill_process(pid, unix::signal_of(sig))).map(delivery)
}

/// Signal the addressable process group independently of the direct leader.
pub(crate) fn signal_boundary(grip: &Grip, sig: Sig) -> io::Result<Delivery> {
    unix::signal_group(grip.group, sig).map(|result| result.map_or(Delivery::Unavailable, delivery))
}

/// The checked state of the addressable target.
pub(crate) fn target_state(grip: &Grip, pid: u32) -> io::Result<TargetState> {
    unix::presence_checked(grip.group, pid).map(|presence| match presence {
        // Unsignallable is present and only present: something is in the addressable target. Either
        // kind of absence here would release a row over a target that still holds something.
        unix::Presence::Present | unix::Presence::Unsignallable => TargetState::Present,
        unix::Presence::Absent if grip.group.is_some() => TargetState::WholeAbsent,
        unix::Presence::Absent => TargetState::LeaderAbsent,
    })
}

const fn delivery(result: unix::SignalResult) -> Delivery {
    match result {
        unix::SignalResult::Accepted => Delivery::Accepted,
        // Never `Absent`: the group is there, and nothing in it took the signal.
        unix::SignalResult::NothingSignalled => Delivery::NothingSignalled,
        unix::SignalResult::Absent => Delivery::Absent,
    }
}

/// See [`unix::quiet`].
pub(crate) fn quiet(grip: &Grip, pid: u32) -> bool {
    unix::quiet(grip.group, pid)
}

pub(crate) fn quiet_checked(grip: &Grip, pid: u32) -> io::Result<bool> {
    unix::quiet_checked(grip.group, pid)
}

#[cfg(test)]
mod tests {

    /// a sweep that signalled nothing maps to its own `Delivery`, and in
    /// particular never to `Absent`. A pure mapping, asserted here because the compiler only forces
    /// the arm to *exist* and says nothing about which one it picks.
    #[test]
    fn a_sweep_that_signalled_nothing_is_never_delivered_absence() {
        assert_eq!(
            super::delivery(crate::unix::SignalResult::NothingSignalled),
            crate::kill::Delivery::NothingSignalled
        );
        assert_ne!(
            super::delivery(crate::unix::SignalResult::NothingSignalled),
            crate::kill::Delivery::Absent,
            "a group that still exists was reported as absent delivery"
        );
        assert_ne!(
            super::delivery(crate::unix::SignalResult::NothingSignalled),
            crate::kill::Delivery::Accepted,
            "a signal nothing took was reported as accepted"
        );
        assert_eq!(
            super::delivery(crate::unix::SignalResult::Accepted),
            crate::kill::Delivery::Accepted
        );
        assert_eq!(
            super::delivery(crate::unix::SignalResult::Absent),
            crate::kill::Delivery::Absent
        );
    }
    use std::os::unix::process::CommandExt as _;
    use std::process::{Child, Command, Stdio};

    use rustix::io::Errno;
    use rustix::process::{Pid, WaitId, WaitIdOptions};

    use super::{
        Prep, grip_adopted, grip_spawned, grip_spawned_without_boundary_for_test, pack, prepare,
        spawned_group,
    };
    use crate::kill::Boundary;

    struct TestChild(Child);

    impl Drop for TestChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    fn sleeper(nonleader: bool) -> (Prep, TestChild) {
        let mut command = Command::new("/bin/sleep");
        command
            .arg("30")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let prep = prepare(&mut command).expect("request own group");
        if nonleader {
            command.process_group(rustix::process::getpgrp().as_raw_nonzero().get());
        }
        (prep, TestChild(command.spawn().expect("live test child")))
    }

    #[test]
    fn fresh_group_lookup_recovers_only_esrch() {
        let pid = Pid::from_raw(123).expect("positive pid");
        let other = Pid::from_raw(124).expect("different pid");
        for (observed, expected) in [
            (Ok(pid), Some(pid)),
            (Err(Errno::SRCH), Some(pid)),
            (Ok(other), None),
            (Err(Errno::PERM), None),
            (Err(Errno::INTR), None),
            (Err(Errno::IO), None),
        ] {
            let prep = prepare(&mut Command::new("unused-query-control")).expect("prepare");
            assert_eq!(spawned_group(prep, pid, observed), expected);
        }
    }

    #[test]
    fn live_nonleaders_adoption_and_missing_boundaries_stay_strict() {
        let (prep, child) = sleeper(false);
        let pid = Pid::from_raw(i32::try_from(child.0.id()).expect("pid fits")).expect("pid");
        let spawned = grip_spawned(prep, &child.0);
        assert_eq!(spawned.grip.group, Some(pid));
        assert!(spawned.weakened.is_none());
        assert_eq!(grip_adopted(child.0.id()).group, Some(pid));

        let (prep, nonleader) = sleeper(true);
        let spawned = grip_spawned(prep, &nonleader.0);
        assert!(spawned.grip.group.is_none() && spawned.weakened.is_some());
        assert!(matches!(spawned.boundary, Boundary::LeaderOnly));
        assert!(grip_adopted(nonleader.0.id()).group.is_none());
        assert!(grip_adopted(0).group.is_none());
        assert!(grip_adopted(u32::MAX).group.is_none());

        let (prep, missing) = sleeper(false);
        let spawned = grip_spawned_without_boundary_for_test(prep, &missing.0);
        assert!(spawned.grip.group.is_none() && spawned.weakened.is_some());
        assert!(matches!(spawned.boundary, Boundary::LeaderOnly));
    }

    #[test]
    fn an_exited_unreaped_child_is_not_fresh_evidence_for_adoption() {
        let mut command = Command::new("/usr/bin/false");
        let prep = prepare(&mut command).expect("prepare");
        let child = TestChild(command.spawn().expect("fast child"));
        let pid = Pid::from_raw(i32::try_from(child.0.id()).expect("pid fits")).expect("pid");
        let status = rustix::process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
        )
        .expect("observe exit without reaping")
        .expect("exit status");
        assert_eq!(status.exit_status(), Some(1));
        assert_eq!(rustix::process::getpgid(Some(pid)), Err(Errno::SRCH));
        assert!(
            grip_adopted(child.0.id()).group.is_none(),
            "adoption cannot recover from ESRCH"
        );
        let spawned = grip_spawned(prep, &child.0);
        assert_eq!(spawned.grip.group, Some(pid));
        assert!(spawned.weakened.is_none());
    }

    /// The pack must be injective over the values a process can have, or two different creation
    /// times compare equal and the key stops distinguishing a child from its successor.
    #[test]
    fn packing_keeps_the_microseconds() {
        assert_eq!(pack(1, 0), 1_000_000);
        assert_ne!(pack(1, 0), pack(1, 1));
        assert_ne!(pack(1, 999_999), pack(2, 0));
        assert_eq!(pack(u64::MAX, u64::MAX), u64::MAX);
    }
}
