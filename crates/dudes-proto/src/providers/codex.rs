//! Codex CLI payloads: the lines `codex exec --json` writes to standard output during a managed
//! turn.
//!
//! Sources: <https://learn.chatgpt.com/docs/non-interactive-mode.md> (the `--json` event list and
//! its `turn.completed` sample), fetched 2026-08-27, and the vendor's exec event types at
//! `rust-v0.151.0` (`codex-rs/exec/src/exec_events.rs`), checked against captured runs of
//! codex-cli 0.158.0-alpha.2.1. Each line is one object tagged by `type`. Only the lines a managed
//! turn's reader needs are typed here; every other `type` is left to the reader to skip.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The field every line carries. Parse this first, then the line's own struct by `type`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Event {
    /// `thread.started`, `turn.started`, `item.started`, `item.updated`, `item.completed`,
    /// `turn.completed`, `turn.failed`, `error`, ….
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

/// `thread.started`: the first line of a run, naming the thread the vendor created or resumed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThreadStarted {
    /// The thread. `codex exec resume <id>` takes it back.
    #[serde(deserialize_with = "super::null_as_default")]
    pub thread_id: String,
}

/// `item.completed`: one thread item in its final state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ItemCompleted {
    /// The item.
    #[serde(deserialize_with = "super::null_as_default")]
    pub item: Item,
}

/// A thread item. The vendor flattens a payload of its own per `type` into the item; only the
/// fields an `agent_message` carries are typed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Item {
    /// `agent_message`, `reasoning`, `command_execution`, `file_change`, `mcp_tool_call`,
    /// `web_search`, `todo_list`, `error`, ….
    #[serde(rename = "type")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub kind: String,
    /// `agent_message`: the model's text.
    pub text: Option<String>,
}

/// `turn.completed`: the turn ended normally.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TurnCompleted {
    /// The thread's token counts so far. On a resumed thread this includes every earlier turn.
    pub usage: Option<Usage>,
}

/// `turn.failed`: the turn ended in an error.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TurnFailed {
    /// What failed.
    pub error: Option<ThreadError>,
}

/// The error a `turn.failed` line carries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThreadError {
    /// The vendor's message.
    #[serde(deserialize_with = "super::null_as_default")]
    pub message: String,
}

/// `turn.completed.usage`, in the vendor's words. `input_tokens` includes the cached and the
/// cache-written tokens, and `output_tokens` includes the reasoning tokens the vendor also reports
/// as `reasoning_output_tokens`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[allow(clippy::struct_field_names)] // the vendor's names, verbatim
pub struct Usage {
    /// Prompt tokens, cached or not.
    #[serde(deserialize_with = "super::null_as_default")]
    pub input_tokens: u64,
    /// Prompt tokens read from the cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub cached_input_tokens: u64,
    /// Prompt tokens written to the cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_write_input_tokens: u64,
    /// Completion tokens, reasoning included.
    #[serde(deserialize_with = "super::null_as_default")]
    pub output_tokens: u64,
}
