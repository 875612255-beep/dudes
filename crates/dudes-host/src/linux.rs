//! Linux: `starttime` from `/proc/<pid>/stat`, boot identity from `/proc/sys/kernel/random/boot_id`,
//! and a pidfd so a signal names a *process* rather than a number.
//!
//! **No `unsafe` in this file, and `tests/unsafe_grant.rs` is what says so.** Nothing here is
//! reached through a raw syscall: the two reads are ordinary files and the pidfd calls are safe
//! `rustix` wrappers.
//!
//! # The start value, and the one derivation that is forbidden
//!
//! Field 22 of `/proc/<pid>/stat` is `starttime`, in clock ticks **since boot**. It is stored and
//! compared exactly as read. It is never added to a boot time to make a wall-clock instant, and
//! that is not a stylistic preference: the boot time available for that addition is `btime` from
//! `/proc/stat`, which the kernel *recomputes*, so two reads either side of an NTP step disagree
//! and a child that never moved appears to have restarted. A boot-relative value plus a boot
//! *identity* answers the same question — did this machine restart? — without the arithmetic, and
//! `the_linux_start_value_is_never_derived_from_a_boot_time` in `tests/platform_key.rs` fails if
//! any of the forbidden spellings reappears in this file.

use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::process::{Child, Command};

use dudes_supervisor::host::ProbeAnswer;
use rustix::process::{Pid, PidfdFlags};

use crate::kill::{Boundary, Delivery, Sig, TargetState};
use crate::peer::{Containment, Process, Read, Source};
use crate::{Spawned, unix};

/// The process-table view used by one peer ancestry walk.
pub(crate) struct PeerSource {
    boot: Option<String>,
}

impl PeerSource {
    pub(crate) const fn new(boot: Option<String>) -> Self {
        Self { boot }
    }
}

impl Source for PeerSource {
    fn process(&mut self, pid: u32) -> Read<Process> {
        let Some(boot) = self.boot.as_ref() else {
            return Read::Refused;
        };
        let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Read::Missing,
            Err(_) => return Read::Refused,
        };
        let Some((parent, start_time)) = process_fields(&text) else {
            return Read::Refused;
        };
        Read::Found(Process {
            parent,
            start_time,
            boot: Some(boot.clone()),
        })
    }

    fn containment(&mut self, _peer: u32, _root: u32) -> Read<Containment> {
        Read::Found(Containment::NotAvailable)
    }
}

/// Nothing extra is needed between building the command and spawning it on this platform.
pub(crate) struct Prep;

/// What the host holds for one identifier on Linux.
pub(crate) struct Grip {
    /// A file descriptor that refers to the *process*, not to the number. Signalling through it
    /// cannot reach a recycled identifier: once the process has been waited on, the descriptor
    /// answers `ESRCH` rather than resolving to whatever now holds the number. `None` on a kernel
    /// without `pidfd_open`, or when the process was already gone when the descriptor was asked
    /// for; the key comparison is the guarantee either way and this only narrows the window.
    pidfd: Option<OwnedFd>,
    /// The process group to sweep, and `None` unless the identifier is that group's leader.
    group: Option<Pid>,
}

pub(crate) const fn exit_group(grip: &Grip) -> Option<Pid> {
    grip.group
}

/// The boot this host is running: opaque, compared for equality, never parsed.
///
/// Read from `/proc/sys/kernel/random/boot_id`, which the kernel generates once per boot and does
/// not recompute. `Ok(None)` is not reachable here — Linux start values are boot-relative, so a
/// key from this host always carries a boot identity or the spawn fails.
pub(crate) fn boot_identity() -> io::Result<Option<String>> {
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let boot = boot.trim();
    if boot.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "/proc/sys/kernel/random/boot_id is empty",
        ));
    }
    Ok(Some(boot.to_owned()))
}

/// Make the child a group leader. See [`unix::prepare`].
// Uniform across the three platform layers on purpose: this shim cannot fail here and can on
// another, and one call site in `lib.rs` reads all three. Collapsing the signature per platform
// would move the `cfg` from the layer into the caller.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn prepare(command: &mut Command) -> io::Result<Prep> {
    unix::prepare(command);
    Ok(Prep)
}

/// The grip on a child this host has just spawned and holds unreaped.
///
/// A missing pidfd narrows no ownership boundary and is retained as an optional leader handle. A
/// missing process group is different: descendants can escape the leader-only grip, so it reports
/// a weakened post-exec setup while preserving every capability acquired.
pub(crate) fn grip_spawned(_prep: Prep, child: &Child) -> Spawned {
    let pid = child.id();
    let group = unix::group_of_leader(pid);
    let weakened = group
        .is_none()
        .then(|| io::Error::other("the spawned child is not the leader of its process group"));
    Spawned {
        grip: Grip {
            pidfd: open_pidfd(pid),
            // `prepare` made this child a group leader, so the group is its own identifier. Asked
            // rather than assumed, because that is the assumption whose failure orphans
            // descendants.
            group,
        },
        boundary: if group.is_some() {
            Boundary::ProcessGroup
        } else {
            Boundary::LeaderOnly
        },
        weakened,
    }
}

#[cfg(test)]
pub(crate) fn grip_spawned_without_boundary_for_test(_prep: Prep, child: &Child) -> Spawned {
    Spawned {
        grip: Grip {
            pidfd: open_pidfd(child.id()),
            group: None,
        },
        boundary: Boundary::LeaderOnly,
        weakened: Some(io::Error::other(
            "the spawned child is not the leader of its process group",
        )),
    }
}

/// The grip on an identifier this host did not spawn — a key adopted from the journal.
///
/// The pidfd is opened **before** the caller compares the key, and that ordering is the point: a
/// descriptor opened while the process still exists refers to that process for as long as it is
/// held, so the comparison and the signal cannot be separated by a recycle. If the identifier had
/// already moved before this call, the comparison that follows is what catches it.
pub(crate) fn grip_adopted(pid: u32) -> Grip {
    Grip {
        pidfd: open_pidfd(pid),
        group: unix::group_of_leader(pid),
    }
}

/// A pidfd for `pid`, or `None` on any failure — an old kernel, a restricted sandbox, or a
/// process that is already gone. Never an error: this is a narrowing, not a requirement.
fn open_pidfd(pid: u32) -> Option<OwnedFd> {
    rustix::process::pidfd_open(unix::as_pid(pid)?, PidfdFlags::empty()).ok()
}

/// `starttime` for `pid`, straight out of `/proc/<pid>/stat`.
///
/// `ENOENT` is the process table saying there is no such identifier, which is positive evidence of
/// absence and the one thing that may become `NoSuchPid`. Every other failure — a permission
/// error, a truncated read, a line that does not parse — is [`ProbeAnswer::CouldNotAnswer`],
/// because a read that failed is not evidence about the child.
pub(crate) fn probe(_grip: &Grip, pid: u32) -> ProbeAnswer {
    let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return ProbeAnswer::NoSuchPid,
        Err(_) => return ProbeAnswer::CouldNotAnswer,
    };
    starttime_of(&text).map_or(ProbeAnswer::CouldNotAnswer, ProbeAnswer::StartValue)
}

/// Field 22 of a `/proc/<pid>/stat` line.
///
/// Split from the **last** `)` and not the first, and not on whitespace from the start: field 2 is
/// the executable name in parentheses and a process is free to be called `foo) 0 (bar`. Everything
/// after the last `)` is fields 3 onwards, so `starttime` — field 22 — is index 19 there.
fn starttime_of(stat: &str) -> Option<u64> {
    process_fields(stat).map(|(_, start_time)| start_time)
}

/// Parent field 4 and start field 22 from the same `/proc/<pid>/stat` row.
fn process_fields(stat: &str) -> Option<(u32, u64)> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let parent = fields.nth(1)?.parse().ok()?;
    let start_time = fields.nth(17)?.parse().ok()?;
    Some((parent, start_time))
}

/// Signal the leader through the pidfd where there is one, then sweep its group.
///
/// The leader and group results stay separate in the shutdown path; ordinary kill returns either
/// failure and treats only `ESRCH` as an already-absent success.
pub(crate) fn signal(grip: &Grip, pid: u32, sig: Sig) -> io::Result<()> {
    signal_leader(grip, pid, sig)?;
    signal_boundary(grip, sig).map(drop)
}

/// Signal the direct leader independently of its process group.
pub(crate) fn signal_leader(grip: &Grip, pid: u32, sig: Sig) -> io::Result<Delivery> {
    if let Some(pidfd) = &grip.pidfd {
        unix::classify_signal(rustix::process::pidfd_send_signal(
            pidfd,
            unix::signal_of(sig),
        ))
        .map(delivery)
    } else {
        let pid = unix::as_pid(pid).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "not a process identifier")
        })?;
        unix::classify_signal(rustix::process::kill_process(pid, unix::signal_of(sig)))
            .map(delivery)
    }
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
    use super::*;

    /// The comm field is attacker-controlled — it is the executable's name — so the parse must
    /// survive spaces and parentheses inside it. Splitting on the first `)` or on whitespace from
    /// the start reads a different field and silently produces a wrong start value, which is a
    /// wrong `Gone` or a wrong `Alive`.
    #[test]
    fn field_22_is_read_from_the_last_paren() {
        // pid comm state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime
        // stime cutime cstime priority nice threads itrealvalue starttime …
        let fields: Vec<String> = (3..=22).map(|n| n.to_string()).collect();
        let stat = format!("42 (plain) {}", fields.join(" "));
        assert_eq!(starttime_of(&stat), Some(22));
        assert_eq!(process_fields(&stat), Some((4, 22)));

        let nasty = format!("42 (we) ird) na me) {}", fields.join(" "));
        assert_eq!(starttime_of(&nasty), Some(22));
        assert_eq!(process_fields(&nasty), Some((4, 22)));
    }

    /// A line that does not parse must answer nothing, not zero: `StartValue(0)` would compare
    /// equal to another failed read and turn two absences of evidence into an `Alive`.
    #[test]
    fn a_line_that_does_not_parse_yields_nothing() {
        assert_eq!(starttime_of(""), None);
        assert_eq!(starttime_of("42 (short) S 0 0"), None);
        assert_eq!(starttime_of("no parens here at all"), None);
    }
}
