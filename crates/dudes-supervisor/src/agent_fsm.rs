//! The six agent states and the total function that moves between them.
//!
//! [`AgentStatus`] already names the states, so there is no new enum here — only the edges, and
//! the two things that are easy to get wrong:
//!
//! - **`Hold` is not a state.** Gates 1 and 2 return `Hold` and the status is unchanged; `Halted`,
//!   `Paused` and `Attached` are control flags read at gate 1, and only the last of them is also
//!   a state.
//! - **A decision never moves an agent whose keyboard a human holds.** Gate 1 returns `Hold` for
//!   an attached agent, so the guard here is belt-and-braces — but it is the guard that keeps an
//!   attached-mode `TurnEnded` (a hook `Stop`) from silently ending the attachment.
//!
//! Every edge is driven by a **record**, never by an input: the machine folds the entry and moves
//! the status from it, so replaying the same entries reaches the same statuses.

use dudes_proto::LoopDecision;
use dudes_types::AgentStatus;

/// What moved an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FsmEvent {
    /// `LoopDecided{Continue}` and the `TurnIssued` that follows it.
    Continued,
    /// `LoopDecided{Wait}`: an open `HumanQuestion` (gate 3).
    Waited,
    /// `LoopDecided{Done}`: the goal's predicate passed with `Observed` provenance (gate 4).
    Finished,
    /// `LoopDecided{Exhausted}`: budget, breaker or hop cap refused admission (gates 5 and 6).
    Exhausted,
    /// `LoopDecided{Hold}`: a control flag or a provenance doubt stands in the way.
    Held,
    /// `TurnEnded`, whatever ended it — a child exit or the reaper's Observed absence.
    TurnEnded,
    /// `HumanAnswer` cleared the open question.
    Answered,
    /// `Attached`: a human took the keyboard.
    Attached,
    /// `Detached`: the operator, the PTY exiting, or `by: System` at restart.
    Detached,
}

impl FsmEvent {
    /// A completed-note admission has no Finished edge.
    #[must_use]
    pub const fn of_note_admission(decision: &dudes_proto::NoteAdmission) -> Self {
        match decision {
            dudes_proto::NoteAdmission::Issue { .. } => Self::Continued,
            dudes_proto::NoteAdmission::Wait { .. } => Self::Waited,
            dudes_proto::NoteAdmission::Hold { .. } => Self::Held,
            dudes_proto::NoteAdmission::Exhausted { .. } => Self::Exhausted,
        }
    }
    /// Which event a journaled decision is.
    #[must_use]
    pub const fn of_decision(decision: &LoopDecision) -> Self {
        match decision {
            LoopDecision::Continue { .. } => Self::Continued,
            LoopDecision::Wait { .. } => Self::Waited,
            LoopDecision::Done => Self::Finished,
            LoopDecision::Exhausted { .. } => Self::Exhausted,
            LoopDecision::Hold { .. } => Self::Held,
        }
    }
}

/// The total transition function. Every `(status, event)` pair has an answer, so a state machine
/// that has fallen behind the record stream cannot get stuck in an unrepresentable place.
#[must_use]
pub const fn next(current: AgentStatus, event: FsmEvent) -> AgentStatus {
    match event {
        // Attach and detach win from every state: they are commands about the keyboard, not
        // decisions about the loop.
        FsmEvent::Attached => AgentStatus::Attached,
        FsmEvent::Detached => AgentStatus::Idle,
        // While a human holds the keyboard the loop issues nothing and ends nothing.
        _ if matches!(current, AgentStatus::Attached) => current,
        FsmEvent::Continued => AgentStatus::Running,
        FsmEvent::Waited => AgentStatus::Waiting,
        FsmEvent::Finished => AgentStatus::Done,
        FsmEvent::Exhausted => AgentStatus::Exhausted,
        FsmEvent::Held => current,
        FsmEvent::TurnEnded | FsmEvent::Answered => AgentStatus::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dudes_proto::{ExhaustedReason, HoldReason};

    /// The edges of the agent state machine, cell by cell. A turn end returns an agent to `Idle` so the
    /// loop re-decides; a human answer does the same for `Waiting`; `Done` and `Exhausted` are
    /// decisions, not terminal traps — a fresh goal decides `Continue` and they go `Running`.
    #[test]
    fn the_section_5_edges_hold() {
        assert_eq!(
            next(AgentStatus::Idle, FsmEvent::Continued),
            AgentStatus::Running
        );
        assert_eq!(
            next(AgentStatus::Running, FsmEvent::TurnEnded),
            AgentStatus::Idle
        );
        assert_eq!(
            next(AgentStatus::Idle, FsmEvent::Waited),
            AgentStatus::Waiting
        );
        assert_eq!(
            next(AgentStatus::Waiting, FsmEvent::Answered),
            AgentStatus::Idle
        );
        assert_eq!(
            next(AgentStatus::Idle, FsmEvent::Finished),
            AgentStatus::Done
        );
        assert_eq!(
            next(AgentStatus::Idle, FsmEvent::Exhausted),
            AgentStatus::Exhausted
        );
        assert_eq!(
            next(AgentStatus::Done, FsmEvent::Continued),
            AgentStatus::Running,
            "a new goal re-decides; Done is not a trap"
        );
        assert_eq!(
            next(AgentStatus::Exhausted, FsmEvent::Continued),
            AgentStatus::Running,
            "tomorrow's budget re-decides; Exhausted is not a trap"
        );
    }

    /// `Hold` is not a state: gates 1 and 2 return it and the status is exactly what it was.
    #[test]
    fn hold_changes_no_status() {
        for status in [
            AgentStatus::Idle,
            AgentStatus::Running,
            AgentStatus::Waiting,
            AgentStatus::Done,
            AgentStatus::Exhausted,
            AgentStatus::Attached,
        ] {
            assert_eq!(next(status, FsmEvent::Held), status);
        }
        assert_eq!(
            FsmEvent::of_decision(&LoopDecision::Hold {
                reason: HoldReason::Paused
            }),
            FsmEvent::Held
        );
    }

    /// Attach wins from every state and detach returns the keyboard to the loop; in between,
    /// nothing the loop decides moves the agent.
    #[test]
    fn attachment_wins_and_holds() {
        for status in [
            AgentStatus::Idle,
            AgentStatus::Running,
            AgentStatus::Waiting,
            AgentStatus::Done,
            AgentStatus::Exhausted,
        ] {
            assert_eq!(next(status, FsmEvent::Attached), AgentStatus::Attached);
        }
        for event in [
            FsmEvent::Continued,
            FsmEvent::Waited,
            FsmEvent::Finished,
            FsmEvent::Exhausted,
            FsmEvent::Held,
            FsmEvent::TurnEnded,
            FsmEvent::Answered,
        ] {
            assert_eq!(
                next(AgentStatus::Attached, event),
                AgentStatus::Attached,
                "{event:?} must not end an attachment"
            );
        }
        assert_eq!(
            next(AgentStatus::Attached, FsmEvent::Detached),
            AgentStatus::Idle
        );
    }

    /// Every decision maps to exactly one event, so a new `LoopDecision` variant fails to
    /// compile rather than silently doing nothing.
    #[test]
    fn every_decision_is_an_event() {
        let pairs = [
            (
                LoopDecision::Continue { task: "t".into() },
                FsmEvent::Continued,
            ),
            (
                LoopDecision::Wait {
                    question: "q".into(),
                },
                FsmEvent::Waited,
            ),
            (LoopDecision::Done, FsmEvent::Finished),
            (
                LoopDecision::Exhausted {
                    reason: ExhaustedReason::Hops,
                },
                FsmEvent::Exhausted,
            ),
            (
                LoopDecision::Hold {
                    reason: HoldReason::Halted,
                },
                FsmEvent::Held,
            ),
        ];
        for (decision, event) in pairs {
            assert_eq!(FsmEvent::of_decision(&decision), event, "{decision:?}");
        }
    }
}
