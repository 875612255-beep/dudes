//! The states of the five machines (agent, turn, goal, card, mail), the two modes and the
//! breaker ladder. Names are the fixed vocabulary; the transitions live in `dudes-policy` and
//! `dudes-supervisor`.

use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How the daemon drives an agent right now.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The default: one headless process per turn on a persistent vendor session;
    /// process exit is the turn end; nothing is ever typed.
    Managed,
    /// A human has the keyboard: the vendor's TUI runs in a daemon-owned PTY between
    /// `agent.attach` and `agent.detach`; hooks govern where the tier allows.
    Attached,
}

/// Where an agent is in its life.
///
/// `Paused` and `Halted` are control flags read at gate 1, not states.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// No turn in flight; the loop will decide.
    Idle,
    /// A managed turn is in flight.
    Running,
    /// An open `HumanQuestion` without a `HumanAnswer`.
    Waiting,
    /// The goal's predicate passed with `Observed` provenance.
    Done,
    /// Budget, breaker or hop cap refused admission.
    Exhausted,
    /// A human holds the keyboard; the loop issues nothing.
    Attached,
}

/// Where a managed turn is in its life.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    /// `TurnIssued` is fsynced; the process may or may not exist yet.
    Issued,
    /// `TurnSpawned` recorded the child's pid and start time.
    Spawned,
    /// `TurnEnded` closed it; the record says whether the end was `Observed` or `Inferred`.
    Ended,
    /// The daemon killed the child (halt, breaker `Stop`, retire, shutdown).
    Killed,
}

/// Where a goal is in its life.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// `GoalSet` appended; no turn issued for it yet.
    Set,
    /// At least one turn issued.
    InProgress,
    /// An `Observed` passing predicate result.
    Achieved,
    /// Cleared by the operator or retired with its agent.
    Abandoned,
}

/// Where a board card is in its life.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    /// Unclaimed.
    Todo,
    /// Claimed by an agent; the claimer is on the card.
    Claimed,
    /// Being worked on.
    Doing,
    /// Waiting on a review.
    Review,
    /// Blocked on something outside the card.
    Blocked,
    /// Finished.
    Done,
}

/// Where a mail item is in its delivery.
///
/// `Durable → Notified{1..5} → Acked | Bounced`. Five unacked notifications raise the recipient's
/// breaker one rung.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MailState {
    /// Journaled; not yet brought to the recipient's attention.
    Durable,
    /// The recipient has been told `n` times (a prompt line in managed mode, the wake sentinel
    /// in attached mode).
    Notified {
        /// Notification count, 1 to 5.
        n: u8,
    },
    /// The recipient moved the file to `inbox/.done/` or a turn reported it read.
    Acked,
    /// Undeliverable.
    Bounced {
        /// Why.
        why: String,
    },
}

impl MailState {
    /// The most times a recipient is notified before the breaker is involved.
    pub const MAX_NOTIFICATIONS: u8 = 5;
}

/// The rungs of the breaker ladder, in order. The ladder itself — one rung up per beat with a
/// signal, one rung down per healthy beat, with a bounded compaction exemption — is
/// policy and lives in `dudes-policy/breaker.rs`; this crate only names the
/// rungs. Gate 5 refuses admission at `Constrain` or above; `Stop` also kills a running turn.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BreakerRung {
    /// Nothing wrong.
    Healthy,
    /// Inject steering context on the next event.
    Steer,
    /// Refuse new turns.
    Constrain,
    /// Refuse new turns and kill the running one.
    Stop,
}

/// A signal that raises the breaker one rung on the next beat.
///
/// Adding a variant is a minor change, but
/// declaration order is journal-visible: derived `Ord` sorts the supervisor's per-agent
/// `BTreeSet` before it becomes `Record::BreakerBeat.signals`.
/// Therefore append variants; never insert one ahead of an existing variant.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BreakerSignal {
    /// The last turn end was `Inferred` (gate 2: `Hold` plus a breaker signal). No v1 emitter
    /// writes such an end — a managed end is `ChildExited` or the reaper's Observed `Vanished`,
    /// attached-mode quiescence is [`Self::PtySilence`] and a hook timeout is
    /// [`Self::HookTimeout`] — and the signal stays because gate 2 stays: `decide()` must not
    /// know which emitters exist.
    InferredTurnEnd,
    /// Attached-mode PTY silence (at most one rung per beat).
    PtySilence,
    /// The supervisor did not answer a hook within its deadline and the hook server failed open.
    HookTimeout,
    /// A standing steer hit the vendor's consecutive-block cap and blocking stopped at 7.
    StopCap,
    /// Five notifications for one mail item went unacknowledged.
    UnackedMail,
    /// A managed turn declared default isolation but its observed vendor init payload reported
    /// inherited plugins or servers.
    IsolationLeak,
    /// A managed turn's observed vendor init reported a billing credential source while no
    /// ceiling is set in any scope.
    ///
    /// The name states the conjunction on purpose. A billing source is not an anomaly on its own
    /// — it is ordinary for an operator who set a ceiling — so what is wrong is the pair, and a
    /// reader of a `Record::BreakerBeat` is told both halves without a second field to look up.
    /// Raised from observed *configuration* rather than observed *behaviour*, which is what makes
    /// it the first of its kind here.
    UncappedBillingSource,
    /// Repeated fast failures diagnosed from observed exits and duration. Alone raises only Steer.
    RapidFailure,
}

impl BreakerSignal {
    /// Every variant, in declaration order — the one list, so a test walks the enum without
    /// keeping a second copy of it in step. The runtime's subset of it is
    /// `dudes_supervisor::RuntimeSignal::ALL`, which is compared against this array rather than
    /// written out again — *partly*: that array is itself hand-written and unguarded, and its own
    /// doc comment says which omissions the comparison misses.
    ///
    /// **This array is hand-written; what makes it authoritative is a test, not this comment.**
    /// `breaker_signal_all_is_every_variant` re-derives the list from serde's own derive — the
    /// `unknown variant …, expected one of …` error carries the total list, generated from the
    /// enum definition — and asserts this array against it, in both membership and order. A
    /// variant added to the enum and not to `ALL`, or removed from `ALL` and not the enum,
    /// reddens there. Every other list of these variants in the workspace is derived from this
    /// one, so drift has one place to happen and one test watching it.
    ///
    /// Declaration order is part of what is pinned, because `BreakerSignal` derives `Ord` from
    /// it and `Record::BreakerBeat.signals` is sorted by a `BTreeSet` before it is journaled.
    pub const ALL: [Self; 8] = [
        Self::InferredTurnEnd,
        Self::PtySilence,
        Self::HookTimeout,
        Self::StopCap,
        Self::UnackedMail,
        Self::IsolationLeak,
        Self::UncappedBillingSource,
        Self::RapidFailure,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder is four rungs in this order; gate 5 compares against `Constrain`, so the
    /// order is part of the contract. Moving between rungs is `dudes-policy`'s job, not this
    /// crate's.
    #[test]
    fn breaker_rungs_are_ordered() {
        assert!(BreakerRung::Healthy < BreakerRung::Steer);
        assert!(BreakerRung::Steer < BreakerRung::Constrain);
        assert!(BreakerRung::Constrain < BreakerRung::Stop);
        assert_eq!(
            serde_json::to_string(&BreakerRung::Constrain).unwrap(),
            "\"constrain\""
        );
    }

    /// Mail state is internally tagged on `state`, so `jq '.state'` reads it and the count
    /// travels with `notified`.
    #[test]
    fn mail_state_is_tagged() {
        let json = serde_json::to_string(&MailState::Notified { n: 2 }).unwrap();
        assert_eq!(json, r#"{"state":"notified","n":2}"#);
        assert_eq!(
            serde_json::from_str::<MailState>(r#"{"state":"acked"}"#).unwrap(),
            MailState::Acked
        );
    }

    /// Fixed vocabulary: every status serialises to its `snake_case` name.
    #[test]
    fn statuses_are_snake_case() {
        assert_eq!(
            serde_json::to_string(&Mode::Managed).unwrap(),
            "\"managed\""
        );
        assert_eq!(
            serde_json::to_string(&AgentStatus::Exhausted).unwrap(),
            "\"exhausted\""
        );
        assert_eq!(
            serde_json::to_string(&GoalStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&BreakerSignal::HookTimeout).unwrap(),
            "\"hook_timeout\""
        );
        assert_eq!(
            serde_json::to_string(&BreakerSignal::IsolationLeak).unwrap(),
            "\"isolation_leak\""
        );
    }

    /// [`BreakerSignal::ALL`] is every variant, in declaration order — **the one place the
    /// enum's variant list is checked against anything**, so this test is what makes the array
    /// authoritative. Every other walk over these variants in the workspace goes through `ALL`
    /// and keeps no list of its own, which is why a red here is not local to this crate.
    ///
    /// Its consumers, so the next reader can see what a red here would let through — the set is
    /// `git grep -n 'BreakerSignal::ALL'`, not this comment, which is why they are named by test
    /// and not counted: `dudes-policy`'s `the_ladder_is_never_raised_by_what_a_turn_cost`
    /// (classification) and `any_signal()` (the proptest strategy);
    /// [`a_breaker_signal_carries_no_cost`](tests::a_breaker_signal_carries_no_cost) here
    /// (payloads); and in `dudes-supervisor`, `reachable_signals()` — the seam's image equation,
    /// which `the_breaker_seam_cannot_carry_an_inferred_turn_end` and
    /// `the_breaker_seam_carries_nothing_raised_by_what_a_turn_cost` both rest on.
    ///
    /// **The derivation.** serde's `unknown variant` error names every variant the derive
    /// generated a match arm for, in declaration order. That list comes from the enum
    /// definition, so it moves when the enum moves and cannot be edited into agreement without
    /// editing the enum. `ALL` is asserted equal to it as a sequence: membership *and* order,
    /// because `Ord` on this enum is declaration order and `Record::BreakerBeat.signals` is a
    /// sorted `BTreeSet`.
    ///
    /// *Control:* the same parse of a name that **is** declared must succeed, so the
    /// `expect_err` above is the unknown-variant arm firing and not `from_str` refusing
    /// everything — a probe that always errored would make the split below read a message it
    /// never checked the shape of.
    #[test]
    fn breaker_signal_all_is_every_variant() {
        let err = serde_json::from_str::<BreakerSignal>("\"nope\"")
            .expect_err("an unknown variant must not deserialise")
            .to_string();
        assert!(
            err.contains("unknown variant"),
            "the probe must reach serde's unknown-variant arm, but got: {err}"
        );
        assert_eq!(
            serde_json::from_str::<BreakerSignal>("\"hook_timeout\"").unwrap(),
            BreakerSignal::HookTimeout,
            "control: a declared name parses, so the refusal above is about `nope` and not \
             about this parser refusing every input"
        );

        let declared: Vec<&str> = err
            .split_once("expected one of ")
            .expect("serde names the variants it expected")
            .1
            .split(" at line")
            .next()
            .unwrap_or_default()
            .split(", ")
            .map(|name| name.trim_matches('`'))
            .collect();
        let walked: Vec<String> = BreakerSignal::ALL
            .iter()
            .map(|signal| {
                serde_json::to_string(signal)
                    .unwrap()
                    .trim_matches('"')
                    .to_owned()
            })
            .collect();
        assert_eq!(
            walked, declared,
            "BreakerSignal::ALL must be every variant, in declaration order; the derive \
             disagrees, so every walk over ALL in this workspace is partial"
        );
    }

    /// declaration order is journal-visible because a beat drains a
    /// `BTreeSet<BreakerSignal>`. The legacy prefix therefore never moves, and every later
    /// variant is appended after it.
    #[test]
    fn breaker_signal_legacy_order_is_stable_and_new_variants_append() {
        assert_eq!(
            &BreakerSignal::ALL[..5],
            &[
                BreakerSignal::InferredTurnEnd,
                BreakerSignal::PtySilence,
                BreakerSignal::HookTimeout,
                BreakerSignal::StopCap,
                BreakerSignal::UnackedMail,
            ]
        );
        assert_eq!(BreakerSignal::ALL[5], BreakerSignal::IsolationLeak);
    }
    /// The reserve must not be routed into the breaker. That is
    /// what keeps the record's reserve inside invariant 3 — a refusal it causes is `Exhausted`
    /// at gate 5 and never a `BreakerRung::Stop` — and nothing but this test and its siblings
    /// enforces it. This is one third of that check: the *structural* third. The ladder third — that
    /// nothing but a signal raises it, and no signal it can be handed is a cost — is
    /// `dudes-policy`'s `the_ladder_is_never_raised_by_what_a_turn_cost`; the *production* third,
    /// in the crate that holds the only construction site, is `dudes-supervisor`'s
    /// `the_breaker_seam_carries_nothing_raised_by_what_a_turn_cost`.
    ///
    /// **A payload is the route.** The reserve is `unknown_ends × per_turn_floor`; the ladder's
    /// input is this enum, and `Record::BreakerBeat.signals` is a `Vec<BreakerSignal>`. A count
    /// or a `Spend` reaches a beat only by riding on a variant, so every variant staying a bare
    /// name is the structural half of the prohibition — and it is checked here rather than
    /// asserted, because a variant that gained a field would still serialise, still journal, and
    /// still replay.
    ///
    /// `wire_name` is total over the enum, so a seventh variant is a **compile error in this test**
    /// before it is anything else. That is deliberate: whoever names a signal after what a turn
    /// cost has to come to this test, and to that sentence above it, to make the tree build.
    ///
    /// That compile error lands in `wire_name`, though, and not in [`BreakerSignal::ALL`] —
    /// which is why `ALL` is not hand-copied here. It is the crate's own array, and
    /// [`breaker_signal_all_is_every_variant`](tests::breaker_signal_all_is_every_variant)
    /// re-derives it from serde's derive. Without that, a variant added to `wire_name` but not
    /// to `ALL` leaves the walk below covering six sevenths of the enum and passing.
    ///
    /// **The rest of the coverage map**, so the next reader inherits it. Classification lives in
    /// `dudes-policy`'s `the_ladder_is_never_raised_by_what_a_turn_cost` for the ladder and in
    /// `dudes-supervisor`'s `the_breaker_seam_carries_nothing_raised_by_what_a_turn_cost` for the
    /// runtime seam; `the_breaker_seam_cannot_carry_an_inferred_turn_end` pins the seam's image
    /// to `BreakerSignal::ALL` minus the signals with no runtime emitter rather than to a list of
    /// its own. **That
    /// image equation does not catch every widening** — when a seventh variant is added to `ALL`
    /// *and* given a runtime emitter, both sides grow and it passes, leaving
    /// the seam's `Cost` classification as the only arm. `reachable_signals()` in
    /// `crates/dudes-supervisor/tests/breaker.rs` carries the table; this paragraph
    /// points at it rather than restating it, because a coverage map stated twice is how the two
    /// copies come to disagree. Neither this crate nor `dudes-policy` *produces* a
    /// `BreakerSignal` — the only non-test construction site is `RuntimeSignal::signal()`
    /// (`dudes-supervisor/src/input.rs:81-88`) — so what is checked here is the shape of the
    /// thing a producer would have to build.
    ///
    /// *Control:* `Payloaded` carries exactly the count that is forbidden and is put through the same
    /// assertion, where it fails to be a bare name — so the check discriminates a name from a
    /// payload rather than passing on whatever serde hands it.
    #[test]
    fn a_breaker_signal_carries_no_cost() {
        const fn wire_name(signal: BreakerSignal) -> &'static str {
            match signal {
                BreakerSignal::InferredTurnEnd => "inferred_turn_end",
                BreakerSignal::PtySilence => "pty_silence",
                BreakerSignal::HookTimeout => "hook_timeout",
                BreakerSignal::IsolationLeak => "isolation_leak",
                BreakerSignal::StopCap => "stop_cap",
                BreakerSignal::UnackedMail => "unacked_mail",
                BreakerSignal::UncappedBillingSource => "uncapped_billing_source",
                BreakerSignal::RapidFailure => "rapid_failure",
            }
        }
        #[derive(Debug, Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Payloaded {
            UnpricedTurnEnd { unknown_ends: u32 },
        }

        // `wire_name` is total over the enum but says nothing about `ALL`'s length, so the walk
        // below is total only because `breaker_signal_all_is_every_variant` pins `ALL` to the
        // derive. That is the one derivation; this test consumes it rather than repeating it.
        for signal in BreakerSignal::ALL {
            let json = serde_json::to_value(signal).unwrap();
            assert_eq!(
                json.as_str(),
                Some(wire_name(signal)),
                "{signal:?} must serialise to a bare name: a payload is how a cost figure \
                 reaches the breaker ladder"
            );
        }

        let control = Payloaded::UnpricedTurnEnd { unknown_ends: 1 };
        assert_eq!(
            serde_json::to_value(&control).unwrap().as_str(),
            None,
            "control: the assertion above rejects a variant carrying a count, so a green run \
             above is evidence and not an artefact of `as_str` accepting anything"
        );
    }
}
