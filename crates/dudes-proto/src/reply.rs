//! What the daemon answers a hook with, before the adapter encodes it for the vendor.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The daemon's answer to a hook event. Built by policy, encoded per vendor by the adapter.
///
/// On the wire the decision is flattened: `{"decision":"deny","reason":"…","context":"…"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookReply {
    /// What to do.
    #[serde(flatten)]
    pub decision: HookDecision,
    /// One context string per event (roster and steer merged), injected as the vendor's
    /// `additionalContext` or equivalent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl HookReply {
    /// Allow with no context — also what the hook route returns on a supervisor timeout.
    pub const ALLOW: Self = Self {
        decision: HookDecision::Allow,
        context: None,
    };

    /// Allow, carrying context.
    #[must_use]
    pub fn allow_with(context: impl Into<String>) -> Self {
        Self {
            decision: HookDecision::Allow,
            context: Some(context.into()),
        }
    }
}

/// The decision inside a [`HookReply`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum HookDecision {
    /// Let the vendor proceed.
    Allow,
    /// Refuse the tool call (pre-tool events) or the prompt.
    Deny {
        /// Shown to the model as the reason.
        reason: String,
    },
    /// At a stop boundary: keep the model working on `task`. Attached mode only, and capped
    /// at seven consecutive blocks on providers that cap at eight.
    Block {
        /// The next unit of work.
        task: String,
    },
    /// Stop processing entirely (`continue: false` on Claude-shaped vendors).
    Halt,
}

/// A [`HookReply`] encoded for one vendor: a JSON body, an exit code, or both.
///
/// The `dudes-hook` shim prints `body` to stdout and exits with `exit_code`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VendorReply {
    /// The vendor-shaped JSON, or nothing (an empty `{}` body is not the same as `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Exit code for the shim. `0` for success; `2` is the blocking error on Claude-shaped
    /// vendors.
    #[serde(default)]
    pub exit_code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision is flattened onto the reply and tagged on `decision`, so an adapter and a
    /// test fixture both read `reply.decision` as a string.
    #[test]
    fn decision_is_flattened() {
        let reply = HookReply {
            decision: HookDecision::Deny {
                reason: "budget".into(),
            },
            context: Some("ctx".into()),
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"decision": "deny", "reason": "budget", "context": "ctx"})
        );
        assert_eq!(serde_json::from_value::<HookReply>(json).unwrap(), reply);
        assert_eq!(
            serde_json::to_value(HookReply::ALLOW).unwrap(),
            serde_json::json!({"decision": "allow"})
        );
    }

    /// Every decision round-trips: `block` carries the next task, `halt` carries nothing, and
    /// the sample count matches the schema.
    #[test]
    fn every_decision_round_trips() {
        let samples = [
            (
                HookDecision::Allow,
                serde_json::json!({"decision": "allow"}),
            ),
            (
                HookDecision::Deny {
                    reason: "budget".into(),
                },
                serde_json::json!({"decision": "deny", "reason": "budget"}),
            ),
            (
                HookDecision::Block {
                    task: "run the tests".into(),
                },
                serde_json::json!({"decision": "block", "task": "run the tests"}),
            ),
            (HookDecision::Halt, serde_json::json!({"decision": "halt"})),
        ];
        let schema = schemars::schema_for!(HookDecision);
        let variants = schema.as_value()["oneOf"].as_array().unwrap().len();
        assert_eq!(variants, samples.len(), "a HookDecision has no sample");
        for (decision, expected) in samples {
            assert_eq!(serde_json::to_value(&decision).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<HookDecision>(expected.clone()).unwrap(),
                decision
            );
            let reply = HookReply {
                decision: decision.clone(),
                context: Some("ctx".into()),
            };
            let text = serde_json::to_string(&reply).unwrap();
            assert_eq!(serde_json::from_str::<HookReply>(&text).unwrap(), reply);
        }
        assert_eq!(
            serde_json::to_value(HookReply::allow_with("roster")).unwrap(),
            serde_json::json!({"decision": "allow", "context": "roster"})
        );
    }
}
