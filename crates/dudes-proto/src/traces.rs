//! The finite, content-free turn history permitted on the authenticated Floor stream.

use dudes_types::{AgentId, Provider, Timestamp};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The count cap on one current agent's recent completed turns.
///
/// An agent keeps up to 8 turns, fewer when rows are long, because [`MAX_AGENT_TRACE_BYTES`] can
/// bind first. A row with a spawn time and usage is about 160 to 180 bytes, so such an agent keeps
/// 6 or 7 turns.
pub const MAX_AGENT_TRACES: usize = 8;
/// The count cap on recent completed turns across current agents.
///
/// The window keeps up to 64 turns, fewer when rows are long, because [`MAX_TRACE_BYTES`] can bind
/// first.
pub const MAX_TRACES: usize = 64;
/// Maximum serialized bytes of one agent's trace array.
pub const MAX_AGENT_TRACE_BYTES: usize = 1_200;
/// Maximum serialized bytes of the complete trace array.
pub const MAX_TRACE_BYTES: usize = 11_520;

/// One of the four content-free outcomes the Floor may show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    /// The turn ended without an observed failure.
    Ended,
    /// The provider, launch, or process reported a failure.
    Failed,
    /// The supervisor held the agent after this turn's rapid failure.
    Held,
    /// The daemon killed the child.
    Killed,
}

/// A recent completed turn, with no task, provider message, model key, or tool detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnTraceView {
    /// The current agent.
    pub agent_id: AgentId,
    /// The latest recorded spawn observation time. Absent without `TurnSpawned`.
    pub started_at: Option<Timestamp>,
    /// The monotonic duration recorded at the end, when available.
    pub duration_ms: Option<u64>,
    /// Fixed outcome with no vendor or host reason text.
    pub outcome: TraceOutcome,
    /// The agent's recorded engine.
    pub engine: Provider,
    /// Aggregate tokens charged to this turn by the journal, when available.
    pub tokens: Option<String>,
    /// Cost charged to this turn by the journal, in micro-dollars when available.
    pub cost_usd_micros: Option<String>,
}
