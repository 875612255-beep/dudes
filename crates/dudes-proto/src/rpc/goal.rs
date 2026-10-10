//! `goal.*` params and results.

use dudes_types::{AgentId, GoalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::budget::GoalBudget;
use crate::goal::Predicate;

/// The id an enrolled workspace's check is recorded under on the goals it decides.
pub const ENROLLED_CHECK_ID: &str = "default";

/// `goal.set` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalSetParams {
    /// The agent that pursues it.
    pub agent: AgentId,
    /// The goal as the model reads it.
    pub text: String,
    /// How the daemon decides it is met.
    pub predicate: Predicate,
    /// Ceiling for the whole goal.
    pub budget: GoalBudget,
    /// Most managed turns the goal may consume.
    pub hop_cap: u32,
    /// The name of a command check, recorded on the goal with its revision. Absent records none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CompletionCheckDeclaration>,
    /// The operator declares that the check may already pass, so goal set does not run it first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub may_already_pass: bool,
}

/// `goal.set` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalSetResult {
    /// The new goal.
    pub goal: GoalId,
}

/// `goal.reassign` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalReassignParams {
    /// The goal.
    pub goal: GoalId,
    /// The new owner.
    pub to: AgentId,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

mod replacement;
pub use replacement::{
    CompletionCheckDeclaration, GoalReplaceParams, GoalReplaceRefusal, GoalReplaceResult,
    MAX_GOAL_REPLACE_BYTES,
};
