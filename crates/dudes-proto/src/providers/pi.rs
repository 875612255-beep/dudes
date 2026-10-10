//! Pi payloads: the JSON Lines a `pi --mode json` run writes to its standard output.
//!
//! Sources: Pi 0.85.1's shipped `docs/json.md`, its `dist/modes/print-mode.js` (the header line),
//! `dist/modes/json-event.js` (the reshaped `message_update`) and `dist/core/agent-session.js`
//! (`compaction_end`), and the `Usage` type of its `pi-ai` package, checked against captured runs of
//! Pi 0.85.1. Only the fields the adapter reads
//! are here. Everything is `#[serde(default)]`, so a field Pi drops reads as its default, and a
//! field it adds is ignored.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The first line of a run, `{"type":"session",…}`, which names the run's session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SessionHeader {
    /// The session id. Pi mints a version 7 UUID unless `--session-id` named one.
    #[serde(deserialize_with = "super::null_as_default")]
    pub id: String,
}

/// `message_start` and `message_end` when the message is the assistant's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MessageEvent {
    /// The whole message as it stands.
    #[serde(deserialize_with = "super::null_as_default")]
    pub message: AssistantMessage,
}

/// One assistant message, which is one provider call.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AssistantMessage {
    /// `assistant`.
    #[serde(deserialize_with = "super::null_as_default")]
    pub role: String,
    /// `text`, `thinking` and `toolCall` blocks.
    #[serde(deserialize_with = "super::null_as_default")]
    pub content: Vec<ContentBlock>,
    /// The model that answered.
    pub model: Option<String>,
    /// The call's usage so far. Pi starts every message at zero and fills in what the provider
    /// reports.
    pub usage: Option<Usage>,
    /// `stop`, `length`, `toolUse`, `error`, `aborted`, or `pending` while the message streams.
    #[serde(rename = "stopReason")]
    pub stop_reason: Option<String>,
    /// Why the call failed, when `stopReason` is `error` or `aborted`.
    #[serde(rename = "errorMessage")]
    pub error_message: Option<String>,
}

/// One content block of an assistant message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ContentBlock {
    /// The block type.
    #[serde(rename = "type")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub kind: String,
    /// `text`: the text.
    pub text: Option<String>,
}

/// `message_update`: an assistant message's usage so far. Pi replaces the streamed message on this
/// line with its usage (`json-event.js`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MessageUpdate {
    /// Cumulative for the message. Each line restates the whole figure so far.
    pub usage: Option<Usage>,
}

/// `compaction_end`: the end of an automatic compaction, which summarises the conversation through a
/// provider call of its own, outside the agent loop, so no message event carries that call.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CompactionEnd {
    /// What the compaction produced. Absent when it failed or was aborted.
    pub result: Option<CompactionResult>,
}

/// The result of a finished compaction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CompactionResult {
    /// The summary calls' usage, combined when Pi made two.
    pub usage: Option<Usage>,
}

/// Token counts for one provider call. `reasoning` is part of `output` and `cacheWrite1h` part of
/// `cacheWrite`, so neither is read. `cost` is not read either, because Pi computes it from its
/// local price catalog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Usage {
    /// Prompt tokens that were not served from a cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub input: u64,
    /// Completion tokens, reasoning included.
    #[serde(deserialize_with = "super::null_as_default")]
    pub output: u64,
    /// Prompt tokens read from the provider's cache.
    #[serde(rename = "cacheRead")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_read: u64,
    /// Prompt tokens written to the provider's cache.
    #[serde(rename = "cacheWrite")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_write: u64,
}
