//! The platform-neutral half of resolving one peer against recorded agent children.

use std::collections::BTreeSet;

use dudes_supervisor::host::AdoptionKey;
#[cfg(windows)]
use dudes_supervisor::host::ProbeAnswer;

/// One process-table row.
#[derive(Clone, Debug)]
pub(crate) struct Process {
    pub(crate) parent: u32,
    pub(crate) start_time: u64,
    pub(crate) boot: Option<String>,
}

/// A platform read without an invented answer for failure.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Read<T> {
    Found(T),
    Missing,
    Refused,
}

/// The optional platform containment fact at a full adoption-key match.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Containment {
    /// This platform has no additional containment relation; ancestry is the complete check.
    #[cfg(any(target_os = "linux", target_os = "macos", test))]
    NotAvailable,
    /// The Windows job query confirms membership.
    #[cfg(any(windows, test))]
    Matched,
    /// The Windows job query confirms non-membership.
    #[cfg(any(windows, test))]
    Mismatched,
}

/// Preserve the process host's three-way probe without teaching a platform module new meanings.
#[cfg(windows)]
pub(crate) const fn from_probe(answer: ProbeAnswer) -> Read<u64> {
    match answer {
        ProbeAnswer::StartValue(value) => Read::Found(value),
        ProbeAnswer::NoSuchPid => Read::Missing,
        ProbeAnswer::CouldNotAnswer => Read::Refused,
    }
}

/// The platform facts the walk consumes.
pub(crate) trait Source {
    fn process(&mut self, pid: u32) -> Read<Process>;
    fn containment(&mut self, peer: u32, root: u32) -> Read<Containment>;
}

/// Which recorded child, if any, owns the peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    Matched(usize),
    NoMatch,
    Unresolvable,
}

/// Whether two complete adoption keys name the same recorded process.
pub(crate) fn same_key(left: &AdoptionKey, right: &AdoptionKey) -> bool {
    left.pid == right.pid
        && left.start_time == right.start_time
        && left.boot.as_deref() == right.boot.as_deref()
}

/// Walk upward from `peer`, requiring ancestry and job membership to agree.
pub(crate) fn resolve(source: &mut impl Source, peer: u32, children: &[AdoptionKey]) -> Resolution {
    let mut visited = BTreeSet::new();
    let mut current = peer;
    let mut starting = true;
    loop {
        if current == 0 || !visited.insert(current) {
            return Resolution::NoMatch;
        }
        let process = match source.process(current) {
            Read::Found(process) => process,
            Read::Missing if starting => return Resolution::Unresolvable,
            Read::Missing => return Resolution::NoMatch,
            Read::Refused => return Resolution::Unresolvable,
        };
        starting = false;

        let observed = AdoptionKey {
            pid: current,
            start_time: process.start_time,
            boot: process.boot,
        };
        if let Some(index) = children.iter().position(|child| same_key(child, &observed)) {
            return match source.containment(peer, current) {
                #[cfg(any(target_os = "linux", target_os = "macos", test))]
                Read::Found(Containment::NotAvailable) => Resolution::Matched(index),
                #[cfg(any(windows, test))]
                Read::Found(Containment::Matched) => Resolution::Matched(index),
                #[cfg(any(windows, test))]
                Read::Found(Containment::Mismatched) => Resolution::NoMatch,
                Read::Missing | Read::Refused => Resolution::Unresolvable,
            };
        }
        if children.is_empty() && process.parent == 1 {
            return Resolution::NoMatch;
        }
        current = process.parent;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Debug, Default)]
    struct Scripted {
        processes: BTreeMap<u32, Read<Process>>,
        jobs: BTreeMap<(u32, u32), Read<Containment>>,
        reads: usize,
        read_cap: usize,
        stop_after: Option<usize>,
    }

    impl Source for Scripted {
        fn process(&mut self, pid: u32) -> Read<Process> {
            self.reads += 1;
            assert!(self.reads <= self.read_cap, "the walk did not terminate");
            if self.stop_after.is_some_and(|cap| self.reads > cap) {
                return Read::Missing;
            }
            self.processes.get(&pid).cloned().unwrap_or(Read::Missing)
        }

        fn containment(&mut self, peer: u32, root: u32) -> Read<Containment> {
            self.jobs
                .get(&(peer, root))
                .copied()
                .unwrap_or(Read::Refused)
        }
    }

    fn child(pid: u32, start_time: u64) -> AdoptionKey {
        AdoptionKey {
            pid,
            start_time,
            boot: None,
        }
    }

    fn source(rows: &[(u32, u32, u64)]) -> Scripted {
        Scripted {
            processes: rows
                .iter()
                .map(|(pid, parent, start_time)| {
                    (
                        *pid,
                        Read::Found(Process {
                            parent: *parent,
                            start_time: *start_time,
                            boot: None,
                        }),
                    )
                })
                .collect(),
            read_cap: rows.len() + 1,
            ..Scripted::default()
        }
    }

    #[test]
    fn ancestry_and_job_membership_together_name_the_child() {
        let mut facts = source(&[(30, 20, 300), (20, 10, 200)]);
        facts
            .jobs
            .insert((30, 20), Read::Found(Containment::Matched));

        assert_eq!(
            resolve(&mut facts, 30, &[child(20, 200)]),
            Resolution::Matched(0)
        );
    }

    #[test]
    fn ancestry_is_complete_when_containment_is_not_available() {
        let mut facts = source(&[(30, 20, 300), (20, 10, 200)]);
        facts
            .jobs
            .insert((30, 20), Read::Found(Containment::NotAvailable));

        assert_eq!(
            resolve(&mut facts, 30, &[child(20, 200)]),
            Resolution::Matched(0)
        );
    }

    #[test]
    fn ancestry_without_job_membership_is_no_match() {
        let mut facts = source(&[(30, 20, 300), (20, 10, 200)]);
        facts
            .jobs
            .insert((30, 20), Read::Found(Containment::Mismatched));

        assert_eq!(
            resolve(&mut facts, 30, &[child(20, 200)]),
            Resolution::NoMatch
        );
    }

    #[test]
    fn a_pid_match_without_the_full_adoption_key_is_no_match() {
        let mut facts = source(&[(30, 20, 300), (20, 10, 201), (10, 0, 100)]);

        assert_eq!(
            resolve(&mut facts, 30, &[child(20, 200)]),
            Resolution::NoMatch
        );
    }

    #[test]
    fn a_boot_mismatch_is_not_a_full_key_match() {
        let mut facts = source(&[(30, 20, 300), (20, 0, 200)]);
        facts.processes.insert(
            20,
            Read::Found(Process {
                parent: 0,
                start_time: 200,
                boot: Some("this-boot".to_owned()),
            }),
        );

        assert_eq!(
            resolve(&mut facts, 30, &[child(20, 200)]),
            Resolution::NoMatch
        );
    }

    #[test]
    fn a_refused_platform_read_is_unresolvable() {
        let mut start_refused = Scripted {
            processes: [(30, Read::Refused)].into_iter().collect(),
            read_cap: 1,
            ..Scripted::default()
        };
        assert_eq!(
            resolve(&mut start_refused, 30, &[]),
            Resolution::Unresolvable
        );

        let mut job_refused = source(&[(30, 20, 300), (20, 10, 200)]);
        job_refused.jobs.insert((30, 20), Read::Refused);
        assert_eq!(
            resolve(&mut job_refused, 30, &[child(20, 200)]),
            Resolution::Unresolvable
        );
    }

    #[test]
    fn an_empty_candidate_walk_stops_after_the_readable_terminal_row() {
        let mut facts = source(&[(30, 20, 300), (20, 1, 200)]);
        facts.processes.insert(1, Read::Refused);

        assert_eq!(resolve(&mut facts, 30, &[]), Resolution::NoMatch);
        assert_eq!(facts.reads, 2, "PID 1 is not read after the terminal row");
    }

    #[test]
    fn empty_candidates_do_not_hide_an_unreadable_intermediate_row() {
        let mut facts = source(&[(30, 20, 300)]);
        facts.processes.insert(20, Read::Refused);

        assert_eq!(resolve(&mut facts, 30, &[]), Resolution::Unresolvable);
    }

    #[test]
    fn a_revisited_pid_terminates_without_a_fixed_hop_limit() {
        let mut facts = source(&[(30, 20, 300), (20, 30, 200)]);

        assert_eq!(resolve(&mut facts, 30, &[]), Resolution::NoMatch);
        assert_eq!(facts.reads, 2);
    }

    #[test]
    fn a_long_chain_reaches_its_last_full_key_and_rejects_a_low_cap_mutant() {
        const DEPTH: u32 = 64;
        const LOW_CAP: usize = 32;
        let rows: Vec<_> = (1..=DEPTH)
            .rev()
            .map(|pid| (pid, pid.saturating_sub(1), u64::from(pid) * 10))
            .collect();
        let mut complete = source(&rows);
        complete
            .jobs
            .insert((DEPTH, 1), Read::Found(Containment::NotAvailable));
        assert_eq!(
            resolve(&mut complete, DEPTH, &[child(1, 10)]),
            Resolution::Matched(0),
            "the full key at hop 64 remains reachable"
        );
        assert_eq!(complete.reads, DEPTH as usize);

        let mut capped = source(&rows);
        capped.stop_after = Some(LOW_CAP);
        capped
            .jobs
            .insert((DEPTH, 1), Read::Found(Containment::NotAvailable));
        assert_eq!(
            resolve(&mut capped, DEPTH, &[child(1, 10)]),
            Resolution::NoMatch,
            "control: an actual 32-hop cap loses the same final key"
        );
        assert_eq!(capped.reads, LOW_CAP + 1);
    }
}
