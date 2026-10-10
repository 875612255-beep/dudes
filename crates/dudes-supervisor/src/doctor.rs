//! What `system.doctor` asks the machine about adoption keys and session recovery.
//!
//! Diagnostic state, and only that. Adoption-key values are derived from the host and open turns;
//! unconfirmed-resume values are derived from the journal fold and survive replay. **Nothing may
//! decide on either** — a diagnostic view is an operator's view and nothing else.

use std::collections::BTreeSet;

use dudes_proto::rpc::system::{AgentHealth, OpenTurnHealth};
use dudes_types::{AgentId, Provider, TurnId};

use crate::host::{Clock, HostBoot, ProcessHost, Rng, key_is_interpretable};
use crate::machine::Supervisor;

/// `system.doctor`'s loop-owned view of open managed turns: issued turns with no recorded spawn,
/// plus the adoption keys of turns that did spawn.
///
/// The two travel together because neither is readable alone. A row marked
/// [`key_uninterpretable`](OpenTurnHealth::key_uninterpretable) says *this host* cannot interpret
/// that key, and which host that was is [`boot`](Self::boot); a boot identity with no rows says
/// nothing is stuck on it. Splitting them across two calls would let a caller pair a mark with the
/// wrong run.
///
/// Not a wire type: the handler puts [`boot`](Self::boot) beside `ping` and [`turns`](Self::turns)
/// into `DoctorResult`'s per-turn collection. It is the shape of what this crate can answer, and
/// `ping` is not part of that.
///
/// The mechanical half of the module doc's rule that nothing may decide on this: no file constructing an `Effect` under `dudes-supervisor`
/// or `dudes-policy` reads a `KeyHealth`, an `OpenTurnHealth`, a `key_health` or a
/// `key_uninterpretable`, nor the collision-free `issued_not_spawned_turns` field;
/// and `dudes-policy` — which could name the protocol rows, since it depends on `dudes-proto` —
/// names none of them at all. `key_uninterpretable` and `issued_not_spawned_turns` make the needle
/// durable because a reader deciding on either diagnostic must write the exact field token, where
/// `OpenTurnHealth` need never be named and `KeyHealth` and `key_health` never reach the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyHealth {
    /// The boot identity this run stamps every spawn with, or the reason it has none — as the host
    /// reports it, carried through unflattened so that a failed read stays distinguishable from a
    /// platform that has no boot identity to read.
    pub boot: HostBoot,
    /// One row per spawned, unclosed turn, keyed by `TurnId`: a projection of
    /// `World::open_children()`, never a join through the agents. A turn whose agent
    /// row is missing or retired is listed like any other, because that is the turn most worth
    /// seeing.
    pub turns: Vec<OpenTurnHealth>,
    /// One row per durable `TurnIssued` without a folded `TurnSpawned` or `TurnEnded`, projected
    /// directly from `World::issued_not_spawned()`. A row says only that no spawn is
    /// recorded; it does not claim that no process exists.
    pub issued_not_spawned_turns: Vec<IssuedNotSpawnedTurnHealth>,
}

/// A durable managed turn for which no spawn is recorded.
///
/// Exactly the journal-derived identity of the state. Process identity, liveness, task, session,
/// model selection and refusal reason are absent because none belongs to this diagnostic row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IssuedNotSpawnedTurnHealth {
    /// The durable turn.
    pub turn: TurnId,
    /// The agent named by `TurnIssued`; a label, never a join key.
    pub agent: AgentId,
}

impl<C: Clock, R: Rng> Supervisor<C, R> {
    /// Per-agent journal-derived diagnostics for `system.doctor`.
    #[must_use]
    pub fn agent_health(&self) -> Vec<AgentHealth> {
        self.world()
            .agents
            .iter()
            .filter(|(_, row)| !row.retired)
            .map(|(agent, _)| AgentHealth {
                agent: *agent,
                // Hook session starts are not journaled yet, so no replay-derived
                // lsn exists to report here.
                last_session_start: None,
                unconfirmed_resume: self.world().unconfirmed_resume(*agent),
                adopted_child: self.world().open_children().any(|(turn, row)| {
                    let (liveness, unknown_for_ms) = self.liveness_of(turn);
                    row.agent == *agent
                        && (liveness != dudes_types::Liveness::Unknown || unknown_for_ms.is_some())
                }),
            })
            .collect()
    }

    /// Every provider with an open turn: issued and not ended, spawned or not. `system.doctor`
    /// starts no sign-in check for these.
    #[must_use]
    pub fn providers_with_open_turns(&self) -> BTreeSet<Provider> {
        let world = self.world();
        world
            .issued_not_spawned()
            .chain(world.open_children())
            .filter_map(|(_, row)| world.agents.get(&row.agent).map(|agent| agent.provider))
            .collect()
    }

    /// What `system.doctor` answers about this run's adoption keys.
    ///
    /// Asks `host` for the boot identity it stamps spawns with and reports it, then
    /// lists every open turn — each marked when *this* host cannot interpret its recorded key at
    /// all, which is the liveness table's diagonal and the one `Liveness::Unknown` that no action on this
    /// machine resolves. A turn in that state stays open for the rest of the run, a standing kill
    /// against it becomes `KillDeferred{LivenessUnknown}`, and the loop makes no progress on its
    /// agent; the pair of facts this returns is what lets an operator see that before it bites
    /// instead of reconstructing it afterwards.
    ///
    /// **The mark is not read off the last liveness answer.** A host that could not read the
    /// process table this second also answers `Unknown` and recovers by itself, so a mark keyed on
    /// the answer would name a set that is not the stuck set. It is decided by the one predicate
    /// [`key_is_interpretable`] that `liveness()` itself consults, which is what keeps the two
    /// from drifting apart.
    ///
    /// Takes the host rather than a [`HostBoot`] threaded in beside it. Both come from the
    /// platform through the seam, so invariant 8 does not choose between them; what does is that
    /// one host in one call makes the identity reported on line 1 and the identity the mark on
    /// line 2 is compared against **the same read**, by construction. A second parameter would let
    /// those two be different values — the mispairing [`KeyHealth`]'s own doc rules out across two
    /// calls, reappearing inside one.
    #[must_use]
    pub fn key_health<H: ProcessHost + ?Sized>(&self, host: &H) -> KeyHealth {
        let boot = host.boot_identity();
        let host_boot = boot.as_host_boot();
        let issued_not_spawned_turns = self
            .world()
            .issued_not_spawned()
            .map(|(turn, row)| IssuedNotSpawnedTurnHealth {
                turn,
                agent: row.agent,
            })
            .collect();
        let turns = self
            .world()
            .open_children()
            .filter_map(|(turn, row)| {
                // An open child is a turn whose `TurnSpawned` is folded, and that record carries
                // the pid and the start value, so the key is present for every row this iterator
                // yields. `filter_map` rather than a default key: a key assembled from defaults is
                // the false `Gone` `AdoptionKey`'s missing equality exists to prevent, and a row
                // that cannot be built honestly is worth less than the one that is missing.
                let key = row.adoption_key()?;
                let (liveness, unknown_for_ms) = self.liveness_of(turn);
                Some(OpenTurnHealth {
                    turn,
                    agent: row.agent,
                    pid: key.pid,
                    start_time: key.start_time,
                    boot: key.boot.clone(),
                    // What the reaper last answered, paired with the age of its current Unknown
                    // run so the two fields cannot be projected from different map entries.
                    liveness,
                    unknown_for_ms,
                    key_uninterpretable: !key_is_interpretable(&key, host_boot),
                })
            })
            .collect();
        KeyHealth {
            boot,
            turns,
            issued_not_spawned_turns,
        }
    }
}
