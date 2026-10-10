//! Same-unit reserve-floor validity over replayed state and one optional live candidate.

use std::fmt;

use dudes_proto::{AgentBudget, GoalBudget, Spend};
use dudes_supervisor::World;
use dudes_types::{Tokens, Usd};

/// A ceiling-bearing value not yet present in the replayed world.
#[derive(Clone, Copy)]
pub(crate) enum Candidate<'a> {
    None,
    Agent(&'a AgentBudget),
    Goal(&'a GoalBudget),
}

/// Units that carry a ceiling while their configured reserve column is zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Missing {
    dollars: bool,
    tokens: bool,
}

/// The remedy: each missing key, dollars first, and the `dudes config` command that sets it.
impl fmt::Display for Missing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match (self.dollars, self.tokens) {
            (true, true) => {
                "set [budget.per_turn_floor] usd and tokens above zero (dudes config set \
                 budget.per_turn_floor.usd <dollars> budget.per_turn_floor.tokens <count>)"
            }
            (true, false) => {
                "set [budget.per_turn_floor] usd above zero (dudes config set \
                 budget.per_turn_floor.usd <dollars>)"
            }
            (false, true) => {
                "set [budget.per_turn_floor] tokens above zero (dudes config set \
                 budget.per_turn_floor.tokens <count>)"
            }
            (false, false) => "",
        })
    }
}

/// Return every missing reserve unit in stable dollars-then-tokens order.
pub(crate) fn missing(
    world: &World,
    floor_usd_per_day: Option<Usd>,
    per_turn_floor: Spend,
    candidate: Candidate<'_>,
) -> Option<Missing> {
    let mut dollars = floor_usd_per_day.is_some();
    let mut tokens = false;

    for row in world.agents.values().filter(|row| !row.retired) {
        include_agent(&mut dollars, &mut tokens, &row.budget);
        if let Some(goal) = row.goal.and_then(|goal| world.goals.get(&goal))
            && !goal.cleared
        {
            include_goal(&mut dollars, &mut tokens, &goal.goal.budget);
        }
    }

    match candidate {
        Candidate::None => {}
        Candidate::Agent(budget) => include_agent(&mut dollars, &mut tokens, budget),
        Candidate::Goal(budget) => include_goal(&mut dollars, &mut tokens, budget),
    }

    // Configuration supplies one token reserve and stores it in both token columns. This check
    // therefore covers the same positive reserve admission consumes; if the columns ever diverge,
    // the predicate must follow the admission column in the same change.
    let missing = Missing {
        dollars: dollars && per_turn_floor.usd == Usd::ZERO,
        tokens: tokens && per_turn_floor.tokens == Tokens::ZERO,
    };
    (missing.dollars || missing.tokens).then_some(missing)
}

fn include_agent(dollars: &mut bool, tokens: &mut bool, budget: &AgentBudget) {
    *dollars |= budget.usd_per_day.is_some();
    *tokens |= budget.tokens_per_day.is_some();
}

fn include_goal(dollars: &mut bool, tokens: &mut bool, budget: &GoalBudget) {
    *dollars |= budget.usd.is_some();
    *tokens |= budget.tokens.is_some();
}
