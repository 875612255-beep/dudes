//! Which predicate evidence may complete a goal.
//!
//! One pure rule over values, shared by the daemon's board runner, the machine's live
//! `PredicateDone` boundary and the replay fold. A cut that skipped it would
//! let a sibling agent's status finish a goal on that path alone: live, or after a restart that
//! replays a result an older runner journaled as passing.

use dudes_proto::{Actor, Evidence, Predicate};
use dudes_types::CardStatus;

/// Whether `evidence` may support a passing result for `predicate`.
///
/// A [`Predicate::BoardState`] pass needs [`Evidence::Card`] naming the requested card and status,
/// where that status can complete a goal and its author is an operator peer or the daemon. Any
/// other evidence fails it. Every other predicate kind keeps its own judgement, so this answers
/// `true` for it.
///
/// The two matches are the board-completion table with the retirement Todo row, written exhaustively so that a
/// new status or author fails to compile here until someone decides its row.
#[must_use]
pub fn predicate_evidence_eligible(predicate: &Predicate, evidence: &Evidence) -> bool {
    let Predicate::BoardState { card, status } = predicate else {
        return true;
    };
    let Evidence::Card {
        card: seen_card,
        status: seen_status,
        by,
    } = evidence
    else {
        return false;
    };
    let status_can_complete = match status {
        CardStatus::Doing | CardStatus::Review | CardStatus::Blocked | CardStatus::Done => true,
        // Claim records always name the claimer, so no author could ever make a Claimed row pass.
        // Retirement requeues the retired agent's unfinished cards to Todo as the daemon, so a
        // passing Todo row would let one agent's retirement finish another's goal.
        CardStatus::Claimed | CardStatus::Todo => false,
    };
    let author_can_complete = match by {
        Actor::Peer { .. } | Actor::System => true,
        Actor::Agent { .. } | Actor::BrowserSession { .. } => false,
    };
    seen_card == card && seen_status == status && status_can_complete && author_can_complete
}
