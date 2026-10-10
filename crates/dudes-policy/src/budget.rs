//! Gate 5: budget is admission (invariant 4). A goal, agent or floor ceiling that is set is
//! checked before a turn is issued; a reached ceiling means not issued — not a warning. An unset
//! ceiling is no cap.

use dudes_proto::{AgentBudget, BudgetScope, Ceiling, GoalBudget, Spend};
use dudes_types::{Tokens, Usd};
use serde::Serialize;

/// Spend in each scope, as the projection holds it. The `*_today` values arrive already rolled
/// over to the current day; day boundaries are not policy's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BudgetState {
    /// Spend against the goal over its whole life.
    pub goal: Spend,
    /// Unobserved ends against the goal over its whole life.
    pub goal_unknown_ends: u32,
    /// The agent's spend today.
    pub agent_today: Spend,
    /// The agent's unobserved ends today.
    pub agent_unknown_ends_today: u32,
    /// The floor's spend today.
    pub floor_today: Spend,
    /// The floor's unobserved ends today.
    pub floor_unknown_ends_today: u32,
}

/// The ceilings of the three scopes.
///
/// Every one of them holds three states per unit:
/// **absent** (`None`) sets no ceiling in that unit and never binds; **zero** (`Some(Usd::ZERO)`,
/// `Some(Tokens::ZERO)`) is a ceiling of nothing, reached before the first dollar is spent, so
/// nothing is issued; **set** is refused once `spent >= n`. There is no "unlimited" ceiling —
/// nothing means unlimited as a *value*, because no ceiling is the absence of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Ceilings {
    /// The goal's, from `goal.set`.
    pub goal: GoalBudget,
    /// The agent's, from `agent.hire`.
    pub agent: AgentBudget,
    /// The floor's, from `[budget] floor_usd_per_day`; dollars only, and absent when the operator
    /// configured none.
    pub floor_usd_per_day: Option<Usd>,
}

/// The verdict of [`admit()`]. `Refused` carries what `Record::BudgetDecision` journals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// No ceiling is reached; the turn may be issued.
    Allowed,
    /// A ceiling is reached; the turn is not issued.
    Refused {
        /// The binding ceiling: the first reached in goal → agent → floor order.
        ceiling: Ceiling,
        /// Spend in that scope.
        spent: Spend,
        /// Unobserved ends in that scope.
        unknown_ends: u32,
        /// Configured price of each unobserved end.
        per_turn_floor: Spend,
    },
}

/// Check every ceiling; refuse on the first reached, in goal → agent → floor order.
///
/// "Reached" is `spent.usd >= cap` where the scope caps dollars, or `spent.work_tokens >= cap`
/// where it caps tokens (the goal and the agent may cap either; the floor caps dollars only). Integer
/// comparisons of values already journaled — no prediction of what the next turn would cost. A
/// `Some(Usd::ZERO)` ceiling is therefore reached at zero spend: "no budget", never "unlimited".
///
/// **An absent ceiling never binds, and an all-absent configuration is allowed.** That is the
/// honest reading of the three ceiling states and this doc does not dress it up: gate 6's hop cap
/// bounds every goal in turns, and the daemon refuses a call that sets no ceiling anywhere while
/// its own environment carries a billing credential a child would inherit. Neither of those is
/// this function's, and neither is invariant 4's — invariant 4 governs what happens when a ceiling
/// is *exceeded*, and a ceiling that does not exist cannot be.
#[must_use]
pub fn admit(state: &BudgetState, ceilings: &Ceilings, per_turn_floor: Spend) -> Admission {
    let scopes = [
        (
            Ceiling {
                scope: BudgetScope::Goal,
                usd: ceilings.goal.usd,
                tokens: ceilings.goal.tokens,
            },
            state.goal,
            state.goal_unknown_ends,
        ),
        (
            Ceiling {
                scope: BudgetScope::Agent,
                usd: ceilings.agent.usd_per_day,
                tokens: ceilings.agent.tokens_per_day,
            },
            state.agent_today,
            state.agent_unknown_ends_today,
        ),
        (
            Ceiling {
                scope: BudgetScope::Floor,
                usd: ceilings.floor_usd_per_day,
                tokens: None,
            },
            state.floor_today,
            state.floor_unknown_ends_today,
        ),
    ];
    scopes
        .into_iter()
        .find(|(ceiling, spent, unknown_ends)| {
            reached(ceiling, reserved(*spent, *unknown_ends, per_turn_floor))
        })
        .map_or(Admission::Allowed, |(ceiling, spent, unknown_ends)| {
            Admission::Refused {
                ceiling,
                spent,
                unknown_ends,
                per_turn_floor,
            }
        })
}

/// Observed spend plus the configured charge for journaled unobserved ends: the figure every
/// ceiling is compared with, and the one a browser goal change subtracts from the goal it
/// replaces.
#[must_use]
pub fn reserved(spent: Spend, unknown_ends: u32, per_turn_floor: Spend) -> Spend {
    let count = u64::from(unknown_ends);
    Spend {
        usd: spent.usd.saturating_add(Usd::from_micros(
            per_turn_floor.usd.micros().saturating_mul(count),
        )),
        tokens: spent.tokens.saturating_add(Tokens::new(
            per_turn_floor.tokens.count().saturating_mul(count),
        )),
        work_tokens: spent.work_tokens.saturating_add(Tokens::new(
            per_turn_floor.work_tokens.count().saturating_mul(count),
        )),
    }
}

/// Whether `spent` has reached `ceiling` in dollars, or in tokens, where the scope caps that unit.
///
/// Symmetric across the two units, and deliberately so: each arm binds only where its own ceiling
/// is present. An `unwrap_or(Usd::ZERO)` on the dollar arm would make **every absent** dollar
/// ceiling refuse at zero spend — the *zero* ceiling state applied to the *absent* one, which is the one
/// coercion that is forbidden by name.
#[must_use]
pub(crate) fn reached(ceiling: &Ceiling, spent: Spend) -> bool {
    ceiling.usd.is_some_and(|cap| spent.usd >= cap)
        || ceiling.tokens.is_some_and(|cap| spent.work_tokens >= cap)
}

/// Whether configured ceilings bound work billed by this provider. Zero is a configured cap.
#[must_use]
pub fn billing_ceiling_satisfied(
    provider: dudes_types::Provider,
    agent: &AgentBudget,
    goal: Option<&GoalBudget>,
    floor_usd_per_day: Option<Usd>,
) -> bool {
    let tokens = agent.tokens_per_day.is_some() || goal.is_some_and(|goal| goal.tokens.is_some());
    let dollars = agent.usd_per_day.is_some()
        || goal.is_some_and(|goal| goal.usd.is_some())
        || floor_usd_per_day.is_some();
    tokens
        || (!matches!(
            provider,
            dudes_types::Provider::Codex | dudes_types::Provider::Pi
        ) && dollars)
}

#[cfg(test)]
mod tests {
    use dudes_types::Tokens;

    use super::*;

    fn ceilings() -> Ceilings {
        Ceilings {
            goal: GoalBudget {
                usd: Some(Usd::from_dollars(10)),
                tokens: Some(Tokens::new(1_000)),
            },
            agent: AgentBudget {
                usd_per_day: Some(Usd::from_dollars(25)),
                tokens_per_day: None,
            },
            floor_usd_per_day: Some(Usd::from_dollars(50)),
        }
    }

    /// Every cell absent: the operator set no ceiling anywhere.
    fn no_ceilings() -> Ceilings {
        Ceilings {
            goal: GoalBudget {
                usd: None,
                tokens: None,
            },
            agent: AgentBudget {
                usd_per_day: None,
                tokens_per_day: None,
            },
            floor_usd_per_day: None,
        }
    }

    fn spend(usd_cents: u64, tokens: u64) -> Spend {
        Spend {
            usd: Usd::from_cents(usd_cents),
            tokens: Tokens::new(tokens),
            work_tokens: Tokens::new(tokens),
        }
    }

    fn spend_with_work(usd_cents: u64, tokens: u64, work_tokens: u64) -> Spend {
        Spend {
            usd: Usd::from_cents(usd_cents),
            tokens: Tokens::new(tokens),
            work_tokens: Tokens::new(work_tokens),
        }
    }

    /// Invariant 4: under every ceiling the turn is admitted.
    #[test]
    fn under_every_ceiling_is_allowed() {
        let state = BudgetState {
            goal: spend(999, 999),
            agent_today: spend(2_499, 5_000),
            floor_today: spend(4_999, 50_000),
            ..BudgetState::default()
        };
        assert_eq!(
            admit(&state, &ceilings(), Spend::default()),
            Admission::Allowed
        );
    }

    /// The *absent* ceiling state: a ceiling that does not exist never binds, in either
    /// unit, however large the spend.
    ///
    /// **This is the arm that goes red for `ceiling.usd.unwrap_or(Usd::ZERO)`** — coercing an absent ceiling to zero, which would refuse every turn — and for `is_some_and` written as `is_none_or`. Its opposite number is
    /// [`a_zero_ceiling_refuses_before_any_spend`] below, which goes red for the fix written the
    /// other way round, as "treat every ceiling as absent". Neither test alone is the rule; the
    /// pair is.
    #[test]
    fn an_absent_dollar_ceiling_never_binds() {
        let spent_hard = BudgetState {
            goal: spend(10_000_000, 10_000_000),
            agent_today: spend(10_000_000, 10_000_000),
            floor_today: spend(10_000_000, 10_000_000),
            ..BudgetState::default()
        };
        assert_eq!(
            admit(&spent_hard, &no_ceilings(), Spend::default()),
            Admission::Allowed
        );
        assert_eq!(
            admit(&BudgetState::default(), &no_ceilings(), Spend::default()),
            Admission::Allowed,
            "and at zero spend, which is where a zero-coercing read would refuse"
        );
    }

    /// An absent dollar ceiling does not mask the token cap beside it: each unit's arm binds on
    /// its own ceiling, and the refusal names the scope with no dollar figure to report.
    ///
    /// The sibling of [`an_absent_dollar_ceiling_never_binds`]: that one pins the dollar operand,
    /// this one pins that guarding the *token* arm on the dollar ceiling — or short-circuiting the
    /// whole expression on `ceiling.usd.is_none()` — is wrong.
    #[test]
    fn an_absent_dollar_ceiling_does_not_mask_a_token_cap() {
        let ceilings = Ceilings {
            goal: GoalBudget {
                usd: None,
                tokens: Some(Tokens::new(10)),
            },
            ..no_ceilings()
        };
        let state = BudgetState {
            goal: spend(0, 10),
            ..BudgetState::default()
        };
        assert_eq!(
            admit(&state, &ceilings, Spend::default()),
            Admission::Refused {
                ceiling: Ceiling {
                    scope: BudgetScope::Goal,
                    usd: None,
                    tokens: Some(Tokens::new(10)),
                },
                spent: spend(0, 10),
                unknown_ends: 0,
                per_turn_floor: Spend::default(),
            },
            "the journaled ceiling carries no dollar figure, because there is no dollar ceiling"
        );
        let state = BudgetState {
            goal: spend(0, 9),
            ..BudgetState::default()
        };
        assert_eq!(
            admit(&state, &ceilings, Spend::default()),
            Admission::Allowed,
            "control: one token below the cap is still allowed"
        );
    }

    /// Invariant 4 boundary: a `Usd::ZERO` ceiling is "no budget" — it refuses before any spend,
    /// with the binding scope's zero spend reported. Nothing about zero reads as unlimited, and
    /// nothing about it reads as absence either (the *zero* row).
    #[test]
    fn a_zero_ceiling_refuses_before_any_spend() {
        let ceilings = Ceilings {
            floor_usd_per_day: Some(Usd::ZERO),
            ..ceilings()
        };
        assert_eq!(
            admit(&BudgetState::default(), &ceilings, Spend::default()),
            Admission::Refused {
                ceiling: Ceiling {
                    scope: BudgetScope::Floor,
                    usd: Some(Usd::ZERO),
                    tokens: None,
                },
                spent: Spend::default(),
                unknown_ends: 0,
                per_turn_floor: Spend::default(),
            }
        );
    }

    /// Invariant 4: reaching a ceiling — equal, not only over — refuses, and the refusal names
    /// the binding scope with its spend.
    #[test]
    fn reaching_a_ceiling_refuses_with_the_binding_scope() {
        let state = BudgetState {
            goal: spend(0, 0),
            agent_today: spend(2_500, 0),
            floor_today: spend(0, 0),
            ..BudgetState::default()
        };
        assert_eq!(
            admit(&state, &ceilings(), Spend::default()),
            Admission::Refused {
                ceiling: Ceiling {
                    scope: BudgetScope::Agent,
                    usd: Some(Usd::from_dollars(25)),
                    tokens: None,
                },
                spent: spend(2_500, 0),
                unknown_ends: 0,
                per_turn_floor: Spend::default(),
            }
        );
    }

    /// Invariant 4: a token cap binds on its own, without the dollar ceiling being reached.
    #[test]
    fn a_token_cap_binds_alone() {
        let state = BudgetState {
            goal: spend(1, 1_000),
            ..BudgetState::default()
        };
        assert!(matches!(
            admit(&state, &ceilings(), Spend::default()),
            Admission::Refused {
                ceiling: Ceiling {
                    scope: BudgetScope::Goal,
                    ..
                },
                ..
            }
        ));
    }

    /// Cache reads stay in the all-kinds total but do not spend a work-token ceiling.
    #[test]
    fn cache_reads_alone_do_not_reach_a_token_ceiling() {
        let ceilings = Ceilings {
            goal: GoalBudget {
                usd: None,
                tokens: Some(Tokens::new(100)),
            },
            ..no_ceilings()
        };
        let state = BudgetState {
            goal: spend_with_work(0, 1_000, 50),
            ..BudgetState::default()
        };
        assert_eq!(
            admit(&state, &ceilings, Spend::default()),
            Admission::Allowed
        );
    }

    /// Invariant 4: when several ceilings are reached the goal's is reported first, then the
    /// agent's, then the floor's.
    #[test]
    fn the_first_reached_scope_in_order_binds() {
        let state = BudgetState {
            goal: spend(1_000, 0),
            agent_today: spend(2_500, 0),
            floor_today: spend(5_000, 0),
            ..BudgetState::default()
        };
        let Admission::Refused { ceiling, .. } = admit(&state, &ceilings(), Spend::default())
        else {
            panic!("three reached ceilings admitted");
        };
        assert_eq!(ceiling.scope, BudgetScope::Goal);
        let state = BudgetState {
            goal: spend(0, 0),
            ..state
        };
        let Admission::Refused { ceiling, .. } = admit(&state, &ceilings(), Spend::default())
        else {
            panic!("two reached ceilings admitted");
        };
        assert_eq!(ceiling.scope, BudgetScope::Agent);
    }

    /// the reserve prices only already-recorded unknown ends, uses
    /// the ceiling's own unit, and saturates before comparing.
    #[test]
    fn unknown_end_reserve_uses_existing_saturating_same_unit_arithmetic() {
        let token_ceiling = Ceilings {
            goal: GoalBudget {
                usd: None,
                tokens: Some(Tokens::new(10)),
            },
            ..no_ceilings()
        };
        let one_unknown = BudgetState {
            goal_unknown_ends: 1,
            ..BudgetState::default()
        };
        assert!(matches!(
            admit(&one_unknown, &token_ceiling, spend(500, 10),),
            Admission::Refused {
                ceiling: Ceiling {
                    scope: BudgetScope::Goal,
                    usd: None,
                    tokens: Some(_),
                },
                unknown_ends: 1,
                ..
            }
        ));
        assert_eq!(
            admit(&BudgetState::default(), &token_ceiling, spend(500, 10)),
            Admission::Allowed,
            "the floor is not a forecast for an unissued turn"
        );
        assert_eq!(
            admit(&one_unknown, &token_ceiling, spend(500, 0)),
            Admission::Allowed,
            "a dollar reserve never satisfies or reaches a token ceiling"
        );

        let dollar_ceiling = Ceilings {
            goal: GoalBudget {
                usd: Some(Usd::from_micros(u64::MAX)),
                tokens: None,
            },
            ..no_ceilings()
        };
        let overflow = BudgetState {
            goal: Spend {
                usd: Usd::from_micros(u64::MAX - 1),
                tokens: Tokens::ZERO,
                work_tokens: Tokens::ZERO,
            },
            goal_unknown_ends: u32::MAX,
            ..BudgetState::default()
        };
        assert!(matches!(
            admit(
                &overflow,
                &dollar_ceiling,
                Spend {
                    usd: Usd::from_micros(2),
                    tokens: Tokens::ZERO,
                    work_tokens: Tokens::ZERO,
                },
            ),
            Admission::Refused { .. }
        ));
    }

    /// An unobserved end reserves the same configured token count in the work-token column that
    /// admission compares. Charging only the all-kinds column would admit this state.
    #[test]
    fn unknown_end_reserve_charges_both_token_columns() {
        let token_ceiling = Ceilings {
            goal: GoalBudget {
                usd: None,
                tokens: Some(Tokens::new(10)),
            },
            ..no_ceilings()
        };
        let one_unknown = BudgetState {
            goal_unknown_ends: 1,
            ..BudgetState::default()
        };
        assert!(matches!(
            admit(&one_unknown, &token_ceiling, spend_with_work(0, 10, 10)),
            Admission::Refused {
                ceiling: Ceiling {
                    scope: BudgetScope::Goal,
                    tokens: Some(_),
                    ..
                },
                unknown_ends: 1,
                ..
            }
        ));
    }
}
