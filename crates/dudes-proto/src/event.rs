//! The normalised hook event: one vocabulary for ten providers.
//!
//! An adapter's `translate` (attached mode, from a hook payload) or `parse_stream_line` (managed
//! mode, from the child's stdout) produces an [`Event`]; the supervisor consumes it as
//! `Input::Hook`. The vendor's JSON travels alongside in `raw` so nothing is lost in translation.

use dudes_types::{AgentId, Provenance, Timestamp, Usage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Something a provider told the daemon, normalised.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    /// The agent whose process produced it (from the hook token or the child table, never from
    /// the payload).
    pub agent: AgentId,
    /// When the daemon received it.
    pub at: Timestamp,
    /// `Observed` for anything the daemon's own process host or hook server delivered;
    /// `Inferred` for anything derived from silence or self-report.
    pub provenance: Provenance,
    /// What happened.
    #[serde(flatten)]
    pub kind: EventKind,
    /// The vendor's JSON, verbatim, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// The event vocabulary. Per-provider names map onto these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// The vendor started or resumed a session. Informational: nothing waits on it, it raises
    /// no rung and never causes `Done`, `Stop` or `Wait`; the vendor's source, session id and
    /// model are in `raw`.
    SessionStart,
    /// The vendor ended a session. Fires on error exits where `TurnEnd` does not (Claude Code
    /// 2.1.239, `--max-turns` / `--max-budget-usd` runs); in managed mode the process exit is
    /// still the turn boundary.
    SessionEnd,
    /// A prompt was submitted to the model.
    PromptSubmit,
    /// A tool is about to run; the reply may deny it.
    PreTool {
        /// The vendor's tool name.
        name: String,
        /// The vendor's tool input, verbatim.
        #[serde(default)]
        input: Value,
    },
    /// A tool ran.
    PostTool {
        /// The vendor's tool name.
        name: String,
        /// Whether the vendor reported success.
        ok: bool,
    },
    /// The model finished a turn (a vendor `Stop`). Attached mode only: in managed mode the
    /// process exit is the `Observed` turn end and the stream's final message is carried on
    /// `TurnEnded`.
    TurnEnd {
        /// The model's final message, when the vendor provides it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_message: Option<String>,
    },
    /// A subagent finished.
    SubagentEnd,
    /// The vendor raised a notification (permission prompt, idle prompt, …).
    Notification {
        /// The vendor's text.
        text: String,
    },
    /// Context compaction started.
    CompactStart,
    /// Context compaction ended.
    CompactEnd,
    /// Token and cost usage the vendor reported.
    Usage {
        /// The counts.
        usage: Usage,
    },
    /// **This variant has no producer and no consumer**.
    ///
    /// An agent asks by calling `agent.ask`, which becomes `Command::Ask` and journals
    /// `Record::HumanQuestion` directly; the supervisor's hook path no longer reads this kind,
    /// and no adapter emits one. The agent-ask path refuses the only thing that could feed it — a
    /// classification of the agent's own output — so a branch here would be a second, weaker
    /// emitter beside the strong one, which is how the strong one gets bypassed.
    ///
    /// It stays declared because the variant is schema surface (`schemas/` is generated from this
    /// crate) and removing it would change the generated schema.
    HumanQuestion {
        /// The question.
        text: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> AgentId {
        AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap()
    }

    /// The event kind is flattened and tagged on `kind`, so a line in the log reads
    /// `{"agent":…,"kind":"pre_tool","name":"Bash",…}` and round-trips.
    #[test]
    fn event_round_trips_with_flattened_kind() {
        let event = Event {
            agent: agent(),
            at: Timestamp::from_unix_millis(1),
            provenance: Provenance::Observed,
            kind: EventKind::PreTool {
                name: "Bash".into(),
                input: serde_json::json!({"command": "echo hi"}),
            },
            raw: Some(serde_json::json!({"hook_event_name": "PreToolUse"})),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "pre_tool");
        assert_eq!(json["name"], "Bash");
        assert_eq!(json["provenance"], "observed");
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }

    /// Every kind round-trips with its `snake_case` tag; the sample count is checked against
    /// the schema so a new variant without a sample fails here.
    #[test]
    fn every_kind_round_trips() {
        let samples = [
            EventKind::SessionStart,
            EventKind::SessionEnd,
            EventKind::PromptSubmit,
            EventKind::PreTool {
                name: "Bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            EventKind::PostTool {
                name: "Bash".into(),
                ok: false,
            },
            EventKind::TurnEnd {
                last_message: Some("done".into()),
            },
            EventKind::SubagentEnd,
            EventKind::Notification {
                text: "permission needed".into(),
            },
            EventKind::CompactStart,
            EventKind::CompactEnd,
            EventKind::Usage {
                usage: Usage {
                    input: dudes_types::Tokens::new(1),
                    usd: Some(dudes_types::Usd::from_micros(5)),
                    ..Usage::ZERO
                },
            },
            EventKind::HumanQuestion {
                text: "which branch?".into(),
            },
        ];
        let schema = schemars::schema_for!(EventKind);
        let variants = schema.as_value()["oneOf"].as_array().unwrap().len();
        assert_eq!(variants, samples.len(), "an EventKind has no sample");
        let expected_tags = [
            "session_start",
            "session_end",
            "prompt_submit",
            "pre_tool",
            "post_tool",
            "turn_end",
            "subagent_end",
            "notification",
            "compact_start",
            "compact_end",
            "usage",
            "human_question",
        ];
        for (kind, tag) in samples.into_iter().zip(expected_tags) {
            let json = serde_json::to_value(&kind).unwrap();
            assert_eq!(json["kind"], tag);
            assert_eq!(serde_json::from_value::<EventKind>(json).unwrap(), kind);
            let event = Event {
                agent: agent(),
                at: Timestamp::from_unix_millis(7),
                provenance: Provenance::Observed,
                kind: kind.clone(),
                raw: None,
            };
            let text = serde_json::to_string(&event).unwrap();
            assert_eq!(
                serde_json::from_str::<Event>(&text).unwrap(),
                event,
                "{tag}"
            );
        }
    }

    /// A unit kind serialises to just its tag, and `raw` is optional on the way in.
    #[test]
    fn unit_kind_and_missing_raw() {
        let json = serde_json::json!({
            "agent": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "at": 0,
            "provenance": "inferred",
            "kind": "session_start"
        });
        let event: Event = serde_json::from_value(json).unwrap();
        assert_eq!(event.kind, EventKind::SessionStart);
        assert_eq!(event.raw, None);
    }
}
