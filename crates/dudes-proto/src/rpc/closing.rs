//! Role-specific extensions of `system.closingTime`; legacy calls retain strict empty results.

use std::path::PathBuf;

use dudes_types::{AgentId, Lsn, Timestamp, TurnId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::hook::HookToken;
use super::system::ShutdownReceipt;
use crate::ClosingPhase;

/// An explicit closing request, after structural caller authentication.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClosingParams {
    /// Operator announcement, returning the extended waiting contract.
    Announce,
    /// Operator status for one immutable announcement.
    Status {
        /// Announcement journal position.
        lsn: Lsn,
        /// Exclusive participant cursor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<AgentId>,
    },
    /// Agent acknowledgement of its own participation.
    Ack {
        /// Announcement journal position.
        lsn: Lsn,
        /// Existing per-call presentation; the owning agent is independently resolved.
        token: HookToken,
    },
}

/// Encode an own acknowledgement without copying or displaying the inherited token.
#[must_use]
pub fn closing_ack_params(token: Option<&HookToken>, lsn: Lsn) -> impl Serialize + '_ {
    #[derive(Serialize)]
    struct Ack<'a> {
        action: &'static str,
        lsn: Lsn,
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<&'a HookToken>,
    }
    Ack {
        action: "ack",
        lsn,
        token,
    }
}

/// Extended operator acknowledgement of a durable announcement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosingStarted {
    /// Immutable closure identity.
    pub announcement: Lsn,
    /// Original wall deadline; retries cannot extend it.
    pub deadline: Timestamp,
    /// Immutable participants, in structural identity order.
    pub participants: Vec<AgentId>,
    /// Unchanged addressed daemon run's teardown receipt.
    pub completion: ShutdownReceipt,
    /// Journal-derived final acknowledgement census.
    pub report: ClosingReportRef,
}

/// The protected runtime location and non-secret run identity of a final closing report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosingReportRef {
    /// Absolute path chosen by the daemon.
    pub path: PathBuf,
    /// The same run identity as the ordinary shutdown receipt.
    pub run: String,
}

/// One operator status page, with no task or memory content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosingStatus {
    /// Immutable announcement identity.
    pub announcement: Lsn,
    /// Latest journaled phase.
    pub phase: ClosingPhase,
    /// Original wall deadline.
    pub deadline: Timestamp,
    /// Complete acknowledgement count from the journal fold.
    pub acknowledged: usize,
    /// Complete participant count from the immutable snapshot.
    pub total: usize,
    /// At most 128 rows, in structural identity order.
    pub participants: Vec<ClosingParticipantStatus>,
    /// Exclusive cursor for another page, absent at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<AgentId>,
}

/// An operator-visible participant row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosingParticipantStatus {
    /// Structural identity captured at announcement.
    pub agent: AgentId,
    /// Snapshot label, bounded to 256 UTF-8 bytes.
    pub name: String,
    /// Whether its explicit acknowledgement is durable.
    pub acknowledged: bool,
    /// Its one saving turn, if issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saving_turn: Option<TurnId>,
    /// Why no acknowledgement can still arrive, when the participant is settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<ClosingSettledReason>,
}

/// Why a Managed participant with no open turn can never be asked to save work.
///
/// Each reason is read from facts the journal never reverses, so a settled participant cannot
/// become admissible again inside the closing window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClosingSettledReason {
    /// The agent had no goal at announcement and has none now.
    NoGoal,
    /// The goal captured at announcement was cleared.
    GoalCleared,
    /// The goal captured at announcement reached an Observed Done.
    GoalDone,
    /// A different goal replaced the captured one after announcement.
    GoalReplaced,
    /// The agent was retired.
    Retired,
}

impl ClosingSettledReason {
    /// Plain words for an operator.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NoGoal => "no goal",
            Self::GoalCleared => "goal cleared",
            Self::GoalDone => "goal done",
            Self::GoalReplaced => "goal replaced",
            Self::Retired => "retired",
        }
    }
}

/// A participant without an acknowledgement that closing time stopped waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClosingSettled {
    /// Structural identity captured at announcement.
    pub agent: AgentId,
    /// Why nothing could still acknowledge.
    pub reason: ClosingSettledReason,
}

/// Final journal-derived census for one addressed daemon run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClosingReport {
    /// Addressed daemon run identity.
    pub run: String,
    /// Immutable announcement identity.
    pub announcement: Lsn,
    /// Durable completion eligibility record.
    pub complete: Lsn,
    /// Immutable participants.
    pub participants: Vec<AgentId>,
    /// Participants with durable explicit acknowledgements.
    pub acknowledged: Vec<AgentId>,
    /// Participants without an acknowledgement when teardown became eligible.
    pub missing: Vec<AgentId>,
    /// The members of `missing` that closing time stopped waiting for, with the reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settled: Vec<ClosingSettled>,
}

/// Maximum accepted participant census.
pub const CLOSING_PARTICIPANT_LIMIT: usize = 4_096;
/// Maximum bytes of the final runtime report.
pub const CLOSING_REPORT_MAX_BYTES: usize = 1_048_576;
/// Fixed production saving interval, in seconds.
pub const CLOSING_GRACE_SECONDS: u64 = 360;

/// Explicit extension schema beside the unchanged legacy Empty-to-Empty method shape.
#[derive(JsonSchema)]
pub struct ClosingSchema {
    /// Operator announce/status and Agent own acknowledgement parameters.
    pub params: ClosingParams,
    /// Extended announcement result, selected only by explicit operator action.
    pub started: ClosingStarted,
    /// Extended operator status page.
    pub status: ClosingStatus,
    /// Bounded ephemeral final report representation.
    pub report: ClosingReport,
}
