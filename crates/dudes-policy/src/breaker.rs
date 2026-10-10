//! The breaker ladder: `Healthy → Steer → Constrain → Stop`, one rung up per beat with a signal,
//! one rung down per healthy beat, with a bounded compaction exemption (`[breaker] hard_stop`
//! configures the top rung). Ordinary beats run on `Tick`. A repeated rapid
//! failure adds an immediate beat whose signal alone can reach only `Steer`.
//! Both are journaled as `Record::BreakerBeat`.

use dudes_types::{BreakerRung, BreakerSignal};

/// Signals discarded while the supervisor's bounded compaction state resolves true.
/// Only `PtySilence`: it is derived from silence, and compaction is silent by nature. Other
/// ordinary signals count during compaction; `RapidFailure` retains its separate `Steer` limit.
pub const COMPACTION_EXEMPT: &[BreakerSignal] = &[BreakerSignal::PtySilence];

/// How long a compaction-start event keeps its breaker exemption open.
pub const COMPACTION_GRACE_MS: i64 = 300_000;

/// How long the breaker exemption remains open after compaction ends.
pub const COMPACTION_TRAILING_GRACE_MS: i64 = 90_000;

/// The most beats on which compaction may suppress an exempt signal without evidence of life.
pub const COMPACTION_MAX_SUPPRESSED_BEATS: u8 = 10;

/// The ladder's configuration, from `[breaker]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakerConfig {
    /// Whether the ladder may reach `Stop`; without it the top rung is `Constrain`.
    pub hard_stop: bool,
}

impl Default for BreakerConfig {
    /// `hard_stop = true`, the configured default.
    ///
    /// Hand-written rather than derived, because `bool::default()` is `false` and a *derived*
    /// default would silently cap the ladder at `Constrain` — leaving `Stop`, and the kill it
    /// authorises, unreachable for any caller that took the default. The safe-looking direction
    /// is the one that disables the top rung, so it is not the one a `derive` gets to pick.
    fn default() -> Self {
        Self { hard_stop: true }
    }
}

impl BreakerConfig {
    /// The highest rung a beat may climb to.
    #[must_use]
    pub const fn cap(self) -> BreakerRung {
        if self.hard_stop {
            BreakerRung::Stop
        } else {
            BreakerRung::Constrain
        }
    }
}

/// An ordinary counting signal climbs one rung to the cap. With only `RapidFailure`, retain the
/// standing rung or raise `Healthy` to `Steer`; inferred rapid failure alone never reaches Stop.
/// Without either, descend one rung to `Healthy`. While `compacting`, [`COMPACTION_EXEMPT`]
/// signals do not count. No beat moves more than one rung.
#[must_use]
pub fn beat(
    rung: BreakerRung,
    signals: &[BreakerSignal],
    compacting: bool,
    config: BreakerConfig,
) -> BreakerRung {
    let counting = signals.iter().any(|signal| {
        *signal != BreakerSignal::RapidFailure
            && !(compacting && COMPACTION_EXEMPT.contains(signal))
    });
    if !counting && signals.contains(&BreakerSignal::RapidFailure) {
        return rung.max(BreakerRung::Steer);
    }
    if counting {
        if rung >= config.cap() { rung } else { up(rung) }
    } else {
        down(rung)
    }
}

/// One rung up; `Stop` stays.
const fn up(rung: BreakerRung) -> BreakerRung {
    match rung {
        BreakerRung::Healthy => BreakerRung::Steer,
        BreakerRung::Steer => BreakerRung::Constrain,
        BreakerRung::Constrain | BreakerRung::Stop => BreakerRung::Stop,
    }
}

/// One rung down; `Healthy` stays.
const fn down(rung: BreakerRung) -> BreakerRung {
    match rung {
        BreakerRung::Healthy | BreakerRung::Steer => BreakerRung::Healthy,
        BreakerRung::Constrain => BreakerRung::Steer,
        BreakerRung::Stop => BreakerRung::Constrain,
    }
}

#[cfg(test)]
mod tests {
    use BreakerRung::{Constrain, Healthy, Steer, Stop};
    use BreakerSignal::{HookTimeout, PtySilence};

    use super::*;

    const HARD: BreakerConfig = BreakerConfig { hard_stop: true };
    const SOFT: BreakerConfig = BreakerConfig { hard_stop: false };

    /// Invariant 3: an Inferred event may raise the rung, by exactly one per beat —
    /// the ladder climbs one rung per beat with a signal, up to `Stop` under `hard_stop`, so no
    /// single event carries it from `Healthy` to `Stop`.
    #[test]
    fn a_signal_climbs_one_rung_per_beat() {
        let mut rung = Healthy;
        let mut seen = vec![rung];
        for _ in 0..5 {
            rung = beat(rung, &[HookTimeout], false, HARD);
            seen.push(rung);
        }
        assert_eq!(seen, [Healthy, Steer, Constrain, Stop, Stop, Stop]);
    }

    /// a healthy beat descends exactly one rung, down to `Healthy` — recovery is as
    /// gradual as escalation, so one quiet beat does not erase a `Stop`.
    #[test]
    fn a_healthy_beat_descends_one_rung() {
        let mut rung = Stop;
        let mut seen = vec![rung];
        for _ in 0..4 {
            rung = beat(rung, &[], false, HARD);
            seen.push(rung);
        }
        assert_eq!(seen, [Stop, Constrain, Steer, Healthy, Healthy]);
    }

    /// Without `hard_stop` (`[breaker] hard_stop`) the ladder tops out at
    /// `Constrain`; a rung already at `Stop` stays `Stop` on a signal — the cap limits climbing,
    /// it does not lower a standing rung — and descends one rung on a quiet beat.
    #[test]
    fn without_hard_stop_the_cap_is_constrain() {
        assert_eq!(beat(Constrain, &[HookTimeout], false, SOFT), Constrain);
        assert_eq!(beat(Steer, &[HookTimeout], false, SOFT), Constrain);
        assert_eq!(beat(Stop, &[HookTimeout], false, SOFT), Stop);
        assert_eq!(beat(Stop, &[], false, SOFT), Constrain);
    }

    /// The configured default is `hard_stop = true`: a derived
    /// `Default` would be `false`, which caps the ladder at `Constrain` and makes `Stop` — and
    /// the kill it authorises — unreachable for a caller that took the default.
    #[test]
    fn the_default_config_reaches_stop() {
        assert!(BreakerConfig::default().hard_stop);
        assert_eq!(BreakerConfig::default().cap(), Stop);
        assert_eq!(
            beat(Constrain, &[HookTimeout], false, BreakerConfig::default()),
            Stop
        );
    }

    #[test]
    fn rapid_failure_alone_raises_steer_and_never_stop() {
        for rung in [Healthy, Steer, Constrain, Stop] {
            assert_eq!(
                beat(rung, &[BreakerSignal::RapidFailure], false, HARD),
                rung.max(Steer)
            );
            assert_eq!(
                beat(
                    rung,
                    &[BreakerSignal::RapidFailure, HookTimeout],
                    false,
                    HARD
                ),
                beat(rung, &[HookTimeout], false, HARD)
            );
        }
    }

    /// What raises a signal, in the one distinction that matters here. Test-local on purpose:
    /// it classifies the enum, it is not a second name for it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum RaisedBy {
        /// Something the daemon watched happen — how a turn ended, silence, a missed deadline, a
        /// cap, a notification nobody acknowledged.
        Conduct,
        /// What a turn cost, or the price put on an end that reported no cost at all. The reserve
        /// must not be routed into the breaker: it is forbidden on the breaker path.
        Cost,
    }

    /// Total over `BreakerSignal`: an eighth variant is a compile error here, which is the point.
    ///
    /// Every arm is `Conduct`, and the or-pattern is not laziness — `clippy::match_same_arms` is
    /// on, so seven arms saying one thing must be written as one. The per-variant reasons:
    /// `InferredTurnEnd` is raised by the **provenance** of an end, never by its usage —
    /// `LoopInput.last_turn_end` is an `Option<Provenance>` and carries no money at all;
    /// `PtySilence` by quiet on a PTY; `HookTimeout` by a deadline the supervisor missed;
    /// `IsolationLeak` by observed vendor init state; `StopCap` by a steer hitting the vendor's
    /// consecutive-block cap; `UnackedMail` by five unacknowledged notifications;
    /// `UncappedBillingSource` by a vendor-reported credential source beside four absent ceiling
    /// `Option`s — it reads no `usage`, prices nothing, compares no figure, and
    /// fires identically on a turn that costs a dollar and on one that costs nothing, which is
    /// what makes it conduct rather than cost. None is a figure anybody priced.
    const fn raised_by(signal: BreakerSignal) -> RaisedBy {
        match signal {
            BreakerSignal::InferredTurnEnd
            | BreakerSignal::PtySilence
            | BreakerSignal::HookTimeout
            | BreakerSignal::IsolationLeak
            | BreakerSignal::StopCap
            | BreakerSignal::UnackedMail
            | BreakerSignal::UncappedBillingSource
            | BreakerSignal::RapidFailure => RaisedBy::Conduct,
        }
    }

    /// The reserve must not be routed into the breaker. Invariant 3's argument
    /// for the reserve rests on that — an `Inferred` event may raise a rung but may
    /// never be the sole cause of `Stop`, and the reserve stays clear of invariant 3 only
    /// because its effect is `Exhausted` at gate 5 and it never touches `BreakerRung::Stop`.
    ///
    /// **What this crate can say, and what it cannot.** The property is about *production* — no
    /// `BreakerSignal` produced from a `TurnEnded` whose `usage` is `None`. Neither this crate nor
    /// `dudes-types` produces a `BreakerSignal` at all: the only non-test construction site in the
    /// workspace is `RuntimeSignal::signal()` (`dudes-supervisor/src/input.rs:81-88`). What is
    /// asserted here is therefore the strongest form of that property available where the ladder
    /// itself lives — that no signal the ladder can be handed *is* a cost, and that nothing else
    /// can climb. The production form is asserted where the construction site is, by
    /// `dudes-supervisor`'s `the_breaker_seam_carries_nothing_raised_by_what_a_turn_cost`; the
    /// three checks are siblings and none of them subsumes another.
    ///
    /// Two claims, both asserted below.
    ///
    /// 1. **Nothing but a signal raises the ladder.** Over every rung, both `compacting` values
    ///    and both configurations, an empty signal slice never climbs. `beat` mints nothing: its
    ///    other three parameters are a rung and two flags, none of which carries a count or a
    ///    `Spend`. So an unpriced end can only reach the ladder *as a `BreakerSignal`*.
    /// 2. **No `BreakerSignal` is raised by what a turn cost.** [`raised_by`] is total over the
    ///    enum, and every arm answers `Conduct`. `InferredTurnEnd` is the arm that makes this a
    ///    distinction rather than a truism: it *is* a turn end, and invariant 3 says it *may*
    ///    raise a rung — because what raises it is the end's provenance, not the end's price.
    ///
    /// Together: an end whose `usage` is `None` yields no signal, and nothing else can climb, so
    /// it cannot reach the breaker. The sibling structural half — that a variant cannot smuggle
    /// the count in as a field — is `dudes-types`' `a_breaker_signal_carries_no_cost`.
    ///
    /// **Where the coverage comes from, so the next reader inherits the map instead of
    /// re-deriving it.**
    ///
    /// - A seventh variant cannot arrive *silently*: [`raised_by`]'s match stops compiling. And
    ///   because that error says nothing about the list being walked, the list is not written
    ///   out here: it is `BreakerSignal::ALL`, which `dudes-types`'
    ///   `breaker_signal_all_is_every_variant` re-derives from serde's own derive. A
    ///   hand-maintained second copy could fall behind the enum and leave this walk covering
    ///   six sevenths of it; there is no second copy.
    /// - A seventh variant *wired to a runtime* is **not** reliably caught. `dudes-supervisor`'s `the_breaker_seam_cannot_carry_an_inferred_turn_end`
    ///   pins `RuntimeSignal::ALL`'s mapped image to `BreakerSignal::ALL` minus
    ///   `InferredTurnEnd` and `IsolationLeak` — an equation whose two sides **both grow** when
    ///   the new signal is added to `BreakerSignal::ALL`, so that wiring passes.
    /// - The *production* form of claim 2 — that the crate holding the only construction site
    ///   builds no cost-raised signal — is `dudes-supervisor`'s
    ///   `the_breaker_seam_carries_nothing_raised_by_what_a_turn_cost`, which classifies that
    ///   same image with its own total match. It is the **only** thing that catches such a
    ///   wiring, and only when the new variant is classified `Cost`. This crate cannot state it;
    ///   see below. Do not re-derive the coverage from this list — `reachable_signals()` in
    ///   `crates/dudes-supervisor/tests/breaker.rs` carries the measured table.
    /// - So what is left uncovered is **two** things, not one. A variant mislabelled against its
    ///   own documentation in **both** classifiers — here and at the seam, since neither reads
    ///   the other's table — needs intent, and no test reads intent. A variant wired and
    ///   *honestly* labelled `Conduct` needs none, and is invisible to the whole workspace.
    ///
    /// **Claim 2 cannot fail on today's six-variant enum. That is what a tripwire is, and it is
    /// stated rather than dressed up.** No control is invented for it: a "control" that does not
    /// run the real classifier would assert a capability it does not establish, which would
    /// be the same defect one level down. The evidence that claim 2 *can* fail is this
    /// mutation — a seventh variant classified `Cost` reddens it by name.
    ///
    /// *Claim 1 does have a live control*, inside its loop: the same call with one `HookTimeout`
    /// must climb from every rung below the cap, so "never climbs" is a property of the empty
    /// slice and not of `beat` being inert.
    #[test]
    fn the_ladder_is_never_raised_by_what_a_turn_cost() {
        // The walked list is `BreakerSignal::ALL` — `dudes-types`' one array, pinned to serde's
        // derive by `breaker_signal_all_is_every_variant`. A seventh variant added to `raised_by`
        // (which stops the build, so it does get fixed) reaches this walk without anyone
        // remembering to widen a list here, because there is no list here to widen.

        // 1. Nothing but a signal raises the ladder.
        for rung in [Healthy, Steer, Constrain, Stop] {
            for compacting in [false, true] {
                for config in [HARD, SOFT] {
                    assert!(
                        beat(rung, &[], compacting, config) <= rung,
                        "no signal must mean no climb, but {rung:?} rose with \
                         compacting={compacting} and {config:?}"
                    );
                    // Control: the identical call with one signal *does* climb below the cap, so
                    // the assertion above is about the empty slice and not about `beat` being inert.
                    if rung < config.cap() {
                        assert!(
                            beat(rung, &[HookTimeout], compacting, config) > rung,
                            "control: a signal must climb from {rung:?} under {config:?}"
                        );
                    }
                }
            }
        }

        // 2. No signal that can reach `beat` is raised by what a turn cost.
        let cost_raised: Vec<BreakerSignal> = BreakerSignal::ALL
            .into_iter()
            .filter(|&signal| raised_by(signal) == RaisedBy::Cost)
            .collect();
        assert!(
            cost_raised.is_empty(),
            "{cost_raised:?} raised by what a turn cost; routing the reserve \
             into the breaker is forbidden, so change the breaker design before this table"
        );
    }

    /// Compaction exempt: PTY silence during compaction is a
    /// healthy beat; any other signal still counts, and silence outside compaction counts.
    /// Invariant 3: silence is Inferred evidence — it may raise the rung, one per beat, and the
    /// rung is an input to gate 5, never a verdict on its own.
    #[test]
    fn compaction_exempts_pty_silence_only() {
        assert_eq!(beat(Steer, &[PtySilence], true, HARD), Healthy);
        assert_eq!(beat(Steer, &[PtySilence], false, HARD), Constrain);
        assert_eq!(
            beat(Steer, &[PtySilence, HookTimeout], true, HARD),
            Constrain
        );
        assert_eq!(COMPACTION_EXEMPT, [PtySilence]);
    }

    /// The five-minute compaction window gives a real compaction time to finish. This pin states the
    /// human duration the millisecond value represents, so a change cannot look like a harmless
    /// unit conversion or an implementation-detail adjustment.
    #[test]
    fn compaction_grace_is_five_minutes() {
        assert_eq!(COMPACTION_GRACE_MS, 5 * 60 * 1_000);
    }

    /// A completed compaction gets ninety seconds of trailing grace for its next-beat
    /// output burst. The duration is part of that distinct contract, not a fraction derived from
    /// the start grace.
    #[test]
    fn compaction_trailing_grace_is_ninety_seconds() {
        assert_eq!(COMPACTION_TRAILING_GRACE_MS, 90 * 1_000);
    }

    /// The bound is ten discarded silence beats, after which the ladder must count
    /// silence again. This numeric pin keeps the security ceiling separate from tests that vary
    /// a test-only ceiling to prove the state machine's relative behavior.
    #[test]
    fn compaction_suppression_ceiling_is_ten_beats() {
        assert_eq!(COMPACTION_MAX_SUPPRESSED_BEATS, 10);
    }
}
