use std::fmt;
use std::marker::PhantomData;

use dudes_types::{AgentId, GoalId};
use schemars::JsonSchema;
use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

use crate::goal::{CompletionCheckRef, Predicate};
use crate::record::Hex32;

/// Maximum original request-file or complete RPC-frame bytes, excluding the RPC terminator.
pub const MAX_GOAL_REPLACE_BYTES: usize = 131_072;

/// Public-safe declaration bound to a literal command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletionCheckDeclaration {
    /// Identifier matching `[a-z][a-z0-9_-]*`, at most 64 bytes.
    pub id: String,
    /// ASCII-trimmed public-safe label, at most 128 UTF-8 bytes and without controls.
    pub label: String,
}

/// One reviewed, allowance-preserving replacement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalReplaceParams {
    /// Identity retained for retries of these exact values.
    pub request_id: Hex32,
    /// Target agent.
    pub agent: AgentId,
    /// Current goal expected by the operator.
    pub expected_goal: GoalId,
    /// ASCII-trimmed goal text, from 1 to 2,048 UTF-8 bytes and without NUL.
    pub text: String,
    /// Literal command. Other predicate kinds are not admitted by this method.
    #[schemars(with = "ReplacementPredicate")]
    pub predicate: Predicate,
    /// Safe check identity and description.
    pub check: CompletionCheckDeclaration,
    /// The operator declares that the check may already pass, so the replacement does not run it
    /// first. It enters the check's revision, so it binds the request identity too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub may_already_pass: bool,
}

/// A replacement's durable or retryable outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GoalReplaceResult {
    /// The replacement is durable and indexed, including a historical retry.
    Changed {
        /// Original request identity.
        request_id: Hex32,
        /// Target agent.
        agent: AgentId,
        /// Fresh goal created by the original request.
        goal: GoalId,
        /// Recorded check identity.
        completion_check: CompletionCheckRef,
    },
    /// This same request is in flight or its append has uncertain durability.
    Pending {
        /// Identity to retain for retry.
        request_id: Hex32,
    },
    /// This attempt appended no replacement.
    Refused {
        /// Original request identity.
        request_id: Hex32,
        /// First applicable refusal.
        reason: GoalReplaceRefusal,
    },
}

/// No-effect replacement refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalReplaceRefusal {
    /// The identity already names different values.
    RequestConflict,
    /// Journal state cannot safely admit a new request.
    Unavailable,
    /// The target is absent or retired.
    AgentAbsent,
    /// The target has a standing halt.
    AgentHalted,
    /// The expected goal is no longer current.
    StaleGoal,
    /// An issued turn or any target-owned command check is unsettled.
    TurnOpen,
    /// The old goal has no remaining capped allowance or hops.
    GoalExhausted,
    /// The check could not run before the first turn: its directory or program is missing, or
    /// the program is inside the tree.
    CheckCannotRun,
    /// The check already passes before any work, so it cannot show when this goal is done.
    CheckAlreadyPasses,
    /// The check's run before the first turn timed out or was ended by a signal.
    CheckDidNotFinish,
}

fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("one JSON object")
        }
        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(Object(PhantomData))
}

fn trim(text: &str) -> &str {
    text.trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

impl CompletionCheckDeclaration {
    /// A declaration from an id and a label, with the label ASCII-trimmed as the wire decoder
    /// trims it.
    ///
    /// # Errors
    ///
    /// As [`Self::validate`].
    pub fn new(id: &str, label: &str) -> Result<Self, &'static str> {
        let check = Self {
            id: id.to_owned(),
            label: trim(label).to_owned(),
        };
        check.validate()?;
        Ok(check)
    }

    /// Whether `id` is a check id: 1 to 64 bytes of `[a-z][a-z0-9_-]*`.
    #[must_use]
    pub fn valid_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 64
            && id.as_bytes()[0].is_ascii_lowercase()
            && id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
    }

    /// Validate the normalized declaration, including declarations read from a journal.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !Self::valid_id(&self.id) {
            return Err("invalid completion check id");
        }
        if self.label.is_empty()
            || self.label.len() > 128
            || trim(&self.label) != self.label
            || self.label.chars().any(char::is_control)
        {
            return Err("invalid completion check label");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CompletionCheckDeclaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            id: String,
            label: String,
        }
        let raw: Fields = object(deserializer)?;
        Self::new(&raw.id, &raw.label).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplacementPredicate {
    #[serde(deserialize_with = "object")]
    command: ReplacementCommand,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplacementCommand {
    argv: Vec<String>,
    timeout_s: u64,
}

fn command<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Predicate, D::Error> {
    let raw: ReplacementPredicate = object(deserializer)?;
    Ok(Predicate::Command {
        argv: raw.command.argv,
        timeout_s: raw.command.timeout_s,
    })
}

impl GoalReplaceParams {
    /// Decode an original request file once, preserving duplicate-key detection.
    pub fn decode_strict(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        if bytes.len() > MAX_GOAL_REPLACE_BYTES {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "goal replacement request exceeds 128 KiB",
            ));
        }
        serde_json::from_slice(bytes)
    }

    /// Validate normalized content at a typed or journal boundary.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.text.is_empty()
            || self.text.len() > 2_048
            || trim(&self.text) != self.text
            || self.text.contains('\0')
        {
            return Err("invalid goal replacement text");
        }
        self.check.validate()?;
        let Predicate::Command { argv, timeout_s } = &self.predicate else {
            return Err("goal replacement requires a command predicate");
        };
        if argv.is_empty()
            || argv.len() > 64
            || argv[0].is_empty()
            || *timeout_s == 0
            || argv.iter().any(|arg| arg.contains('\0'))
            || argv
                .iter()
                .try_fold(0usize, |sum, arg| sum.checked_add(arg.len()))
                .is_none_or(|sum| sum > 16_384)
        {
            return Err("invalid goal replacement command");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for GoalReplaceParams {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            request_id: Hex32,
            agent: AgentId,
            expected_goal: GoalId,
            text: String,
            #[serde(deserialize_with = "command")]
            predicate: Predicate,
            check: CompletionCheckDeclaration,
            #[serde(default)]
            may_already_pass: bool,
        }
        let raw: Fields = object(deserializer)?;
        let request = Self {
            request_id: raw.request_id,
            agent: raw.agent,
            expected_goal: raw.expected_goal,
            text: trim(&raw.text).to_owned(),
            predicate: raw.predicate,
            check: raw.check,
            may_already_pass: raw.may_already_pass,
        };
        request.validate().map_err(serde::de::Error::custom)?;
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> Value {
        json!({"request_id":"0".repeat(64),"agent":"01ARZ3NDEKTSV4RRFFQ69G5FAV","expected_goal":"01ARZ3NDEKTSV4RRFFQ69G5FAW","text":" ship ","predicate":{"command":{"argv":["cargo","test"],"timeout_s":600}},"check":{"id":"tests","label":" Tests "}})
    }
    use serde_json::Value;

    /// `goal.set` without a named check keeps its bytes, and a named one is decoded as the
    /// replacement's declaration is: trimmed, then validated.
    #[test]
    fn goal_set_names_its_check_only_when_given_one() {
        use crate::rpc::goal::GoalSetParams;
        let plain = json!({"agent":"01ARZ3NDEKTSV4RRFFQ69G5FAV","text":"ship","predicate":{"command":{"argv":["true"],"timeout_s":1}},"budget":{},"hop_cap":3});
        let params: GoalSetParams = serde_json::from_value(plain.clone()).unwrap();
        assert_eq!(params.check, None);
        assert_eq!(serde_json::to_value(&params).unwrap(), plain);
        let mut named = plain.clone();
        named["check"] = json!({"id": "tests", "label": " Tests "});
        let params: GoalSetParams = serde_json::from_value(named).unwrap();
        assert_eq!(
            params.check,
            Some(CompletionCheckDeclaration::new("tests", "Tests").unwrap())
        );
        for check in [
            json!({"id": "Tests", "label": "Tests"}),
            json!({"id": "tests", "label": "   "}),
            json!({"id": "tests", "label": "Tests", "argv": ["true"]}),
        ] {
            let mut bad = plain.clone();
            bad["check"] = check;
            assert!(serde_json::from_value::<GoalSetParams>(bad).is_err());
        }
    }

    #[test]
    fn replacement_schema_and_decoder_agree_on_command_shape() {
        let schema = schemars::schema_for!(GoalReplaceParams).to_value();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let good = request();
        assert!(validator.is_valid(&good));
        assert!(GoalReplaceParams::decode_strict(&serde_json::to_vec(&good).unwrap()).is_ok());
        for predicate in [
            json!({"board_state":{"card":"01ARZ3NDEKTSV4RRFFQ69G5FAV","status":"done"}}),
            json!({"path_hash":{"path":"file","blake3":"a"}}),
            json!({"agent_verify":{"prompt":"review","budget":1}}),
            json!({"command":{"argv":["true"],"timeout_s":1,"extra":0}}),
        ] {
            let mut bad = good.clone();
            bad["predicate"] = predicate;
            assert!(!validator.is_valid(&bad));
            assert!(GoalReplaceParams::decode_strict(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
    }

    #[test]
    fn strict_decode_normalizes_text_and_label_without_changing_argv() {
        let parsed =
            GoalReplaceParams::decode_strict(&serde_json::to_vec(&request()).unwrap()).unwrap();
        assert_eq!(parsed.text, "ship");
        assert_eq!(parsed.check.label, "Tests");
        assert_eq!(
            parsed.predicate,
            Predicate::Command {
                argv: vec!["cargo".into(), "test".into()],
                timeout_s: 600
            }
        );
    }

    #[test]
    fn strict_decode_rejects_unknown_duplicate_and_array_shapes() {
        let valid = serde_json::to_string(&request()).unwrap();
        let mutations = [
            valid.replacen('{', "{\"budget\":{},", 1),
            valid.replace("\"text\":", "\"te\\u0078t\":\"other\",\"text\":"),
            valid.replace("\"check\":{", "\"check\":{\"id\":\"other\","),
            valid.replace("\"command\":{", "\"command\":{\"argv\":[\"other\"],"),
            valid.replace("\"predicate\":{", "\"predicate\":{\"command\":{},"),
            valid.replace("\"timeout_s\":600", "\"timeout_s\":600,\"timeout_s\":1"),
            valid.replace(
                "{\"id\":\"tests\",\"label\":\" Tests \"}",
                "[\"tests\",\"Tests\"]",
            ),
            valid.replace(
                "{\"argv\":[\"cargo\",\"test\"],\"timeout_s\":600}",
                "[[\"cargo\"],600]",
            ),
            valid.replace("\"command\"", "\"board_state\""),
        ];
        assert!(GoalReplaceParams::decode_strict(valid.as_bytes()).is_ok());
        for mutation in mutations {
            assert!(
                GoalReplaceParams::decode_strict(mutation.as_bytes()).is_err(),
                "{mutation}"
            );
        }
        assert!(GoalReplaceParams::decode_strict(b"[]").is_err());
    }

    #[test]
    fn strict_decode_enforces_each_content_bound() {
        let mut maximal = request();
        maximal["text"] = json!("é".repeat(1024));
        maximal["check"]["id"] = json!("a".repeat(64));
        maximal["check"]["label"] = json!("é".repeat(64));
        maximal["predicate"]["command"]["argv"] = json!(vec!["a".repeat(256); 64]);
        assert!(GoalReplaceParams::decode_strict(&serde_json::to_vec(&maximal).unwrap()).is_ok());
        for (pointer, value) in [
            ("/text", json!("a".repeat(2049))),
            ("/text", json!("a\0b")),
            ("/request_id", json!("A".repeat(64))),
            ("/agent", json!("01arz3ndektsv4rrffq69g5fav")),
            ("/check/id", json!("a".repeat(65))),
            ("/check/label", json!("a".repeat(129))),
            ("/check/label", json!("line\nline")),
            ("/predicate/command/argv", json!(vec!["a"; 65])),
            ("/predicate/command/argv", json!([""])),
            ("/predicate/command/argv", json!(["a\0b"])),
            ("/predicate/command/argv", json!(["a".repeat(16385)])),
            ("/predicate/command/timeout_s", json!(0)),
        ] {
            let mut bad = maximal.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(
                GoalReplaceParams::decode_strict(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{pointer}"
            );
        }
        let mut bytes = serde_json::to_vec(&maximal).unwrap();
        bytes.resize(MAX_GOAL_REPLACE_BYTES, b' ');
        assert!(GoalReplaceParams::decode_strict(&bytes).is_ok());
        bytes.push(b' ');
        assert!(GoalReplaceParams::decode_strict(&bytes).is_err());
    }
}
