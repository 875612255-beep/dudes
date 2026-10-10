//! `goal.set`.

use std::sync::Arc;

use dudes_proto::goal::Predicate;
use dudes_proto::rpc::goal::GoalSetParams;
use dudes_proto::rpc::{ErrorCode, Method, RpcError};
use serde_json::Value;

use crate::dispatch::{Call, error};
use crate::methods::{Control, encode, params, refuse_nul};

/// `goal.set`: give an agent a goal, its predicate, its ceiling and its hop cap.
///
/// A `Predicate::Command` carries `argv` as a typed list, and nothing on this path joins it into a
/// string: invariant 7 is a property of what is built, and the door is where a string would be
/// most tempting to accept.
///
/// A `text` holding U+0000 is invalid params: the goal text enters the agent's next
/// Managed task, which no launch can carry with U+0000 in it. It is refused before the goal
/// reaches the daemon, so no `GoalSet` is written and the agent keeps the goal it had.
pub fn set<C: Control>(
    control: Arc<C>,
) -> impl Fn(Call<'_>) -> Result<Value, RpcError> + Send + Sync + 'static {
    move |call| {
        let request: GoalSetParams = params(&call)?;
        refuse_nul(Method::GoalSet, &request.text)?;
        match &request.predicate {
            Predicate::PathHash { .. } => {
                return Err(error(
                    ErrorCode::Refused,
                    "goal.set: path_hash has no predicate runner",
                ));
            }
            Predicate::AgentVerify { .. } => {
                return Err(error(
                    ErrorCode::Refused,
                    "goal.set: agent_verify has no predicate runner",
                ));
            }
            Predicate::BoardState { .. } if request.check.is_some() => {
                return Err(error(
                    ErrorCode::InvalidParams,
                    "goal.set: only a command check can be named",
                ));
            }
            Predicate::Command { .. } | Predicate::BoardState { .. } => {}
        }
        let result = control.set_goal(request, &call.hang_up)?;
        encode(Method::GoalSet, &result)
    }
}

/// Replace only the strictly decoded original-frame request.
pub fn replace<C: Control>(
    control: Arc<C>,
) -> impl Fn(Call<'_>) -> Result<Value, RpcError> + Send + Sync + 'static {
    move |call| {
        let request = call.goal_replacement.ok_or_else(|| {
            error(
                ErrorCode::InvalidParams,
                "goal.replace requires original-frame validation",
            )
        })?;
        encode(Method::GoalReplace, &control.replace_goal(request.clone())?)
    }
}
