//! Ceilings and spend. Budget is admission (invariant 4): every ceiling here that is set is
//! checked before a turn is issued, and exceeded means not issued. An unset ceiling is no cap.
//!
//! # A ceiling has three states per unit
//!
//! Every ceiling field below is an `Option`, and the three states it can hold are defined here
//! once rather than restated in each field's own doc:
//!
//! - **absent** — the key is omitted or `null`, either way `None`: *this scope sets no ceiling in
//!   this unit*, and its arm of gate 5 never binds;
//! - **zero** — the key is present and `0`: *a ceiling of nothing*, reached at zero spend, so the
//!   first turn is refused before it is issued;
//! - **set** — the key is present and `n > 0`: refused once `spent >= n`.
//!
//! **Absence is not zero and zero is not absence**, and no code path may coerce one into the
//! other: they are different instructions with opposite effects. There is no fourth state — nothing
//! means *unlimited* as a value, because "no ceiling" is the absence of one.
//!
//! Each field carries `#[serde(default, skip_serializing_if = "Option::is_none")]`, so a journal or
//! a call written before a field became optional folds unchanged into `Some`, and one written after
//! it omits the key rather than emitting a null.

use dudes_types::{Tokens, Usd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An agent's daily ceiling, set at `agent.hire`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentBudget {
    /// Dollars per day across every goal the agent works on, when the operator caps dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_per_day: Option<Usd>,
    /// Work tokens per day, when the operator caps tokens too: input, output and prompt-cache
    /// creation, the figure [`Spend::work_tokens`] carries. Prompt-cache reads do not spend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_day: Option<Tokens>,
}

/// A goal's ceiling, set at `goal.set`. Goal-scoped: it carries over a `GoalReassigned`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct GoalBudget {
    /// Dollars for the whole goal, when the operator caps dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<Usd>,
    /// Work tokens for the whole goal, when capped: input, output and prompt-cache creation, the
    /// figure [`Spend::work_tokens`] carries. Prompt-cache reads do not spend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
}

/// What has been spent so far in some scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Spend {
    /// Dollars.
    pub usd: Usd,
    /// Every token counted once, including prompt-cache reads.
    pub tokens: Tokens,
    /// Tokens attributable to work: uncached input, output and prompt-cache creation.
    pub work_tokens: Tokens,
}

impl Spend {
    /// The per-turn reserve, `[budget.per_turn_floor]`, in each unit the operator leaves unset:
    /// $0.25 and 20,000 tokens. It prices each turn end whose usage the daemon never saw, and
    /// binds only beside a set ceiling in its unit.
    pub const DEFAULT_PER_TURN_FLOOR: Self = Self {
        usd: Usd::from_micros(250_000),
        tokens: Tokens::new(20_000),
        work_tokens: Tokens::new(20_000),
    };
}

/// Which ceiling a budget decision was made against.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// The goal's [`GoalBudget`].
    Goal,
    /// The agent's [`AgentBudget`] for the current day.
    Agent,
    /// The floor-wide daily ceiling from configuration.
    Floor,
}

/// The binding ceiling of a budget decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Ceiling {
    /// Whose ceiling.
    pub scope: BudgetScope,
    /// Dollars, when that scope caps dollars.
    ///
    /// **Absent, not `Usd::ZERO`, when the refusal was not about dollars.** A refusal against a
    /// token-only ceiling has no dollar figure to journal, and writing a zero here would put
    /// *"a ceiling of nothing — refuses every turn"* onto a record describing a refusal that
    /// happened for another reason entirely: a false value, durable, in the log every projection
    /// is rebuilt from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<Usd>,
    /// Work tokens, when that scope caps tokens: the ceiling is compared with
    /// [`Spend::work_tokens`], not with the all-kinds [`Spend::tokens`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three ceiling states on the wire: absence, zero and a set value are three distinct readings
    /// of the same key, and the absent one omits the key rather than emitting a null.
    ///
    /// The zero row is in the same test as the absent row on purpose. An implementation that
    /// collapsed them — `#[serde(default)]` on a bare `Usd`, say — satisfies the absent row by
    /// decoding `{}` and fails here, because `{}` and `{"usd":0}` would read the same.
    ///
    /// The omission assertion is what makes the journal direction forward-only: a record written
    /// by this build carries no key at all where there is no ceiling, so a reader that folds
    /// `{}` into `None` folds it the same before and after.
    #[test]
    fn absence_zero_and_a_value_are_three_readings_of_one_ceiling_key() {
        let absent: GoalBudget = serde_json::from_str("{}").expect("an absent ceiling");
        assert_eq!(absent.usd, None, "an omitted key is no ceiling");
        assert_eq!(
            serde_json::from_str::<GoalBudget>(r#"{"usd":null}"#)
                .expect("an explicit null")
                .usd,
            None,
            "and so is an explicit null"
        );
        assert_eq!(
            serde_json::from_str::<GoalBudget>(r#"{"usd":0}"#)
                .expect("a zero ceiling")
                .usd,
            Some(Usd::ZERO),
            "a present zero is a ceiling of nothing, not the absence of one"
        );
        assert_eq!(
            serde_json::from_str::<GoalBudget>(r#"{"usd":7}"#)
                .expect("a set ceiling")
                .usd,
            Some(Usd::from_micros(7))
        );

        assert_eq!(
            serde_json::to_string(&absent).expect("serialises"),
            "{}",
            "an absent ceiling omits its key; a null would be a second spelling of absence"
        );
        assert_eq!(
            serde_json::to_string(&GoalBudget {
                usd: Some(Usd::ZERO),
                tokens: None,
            })
            .expect("serialises"),
            r#"{"usd":0}"#,
            "and a zero one is written, because it says something"
        );

        // The same three readings on the other two carriers, so the rule is the type's and not
        // one field's.
        assert_eq!(
            serde_json::from_str::<AgentBudget>("{}")
                .expect("an absent ceiling")
                .usd_per_day,
            None
        );
        assert_eq!(
            serde_json::from_str::<AgentBudget>(r#"{"usd_per_day":0}"#)
                .expect("a zero ceiling")
                .usd_per_day,
            Some(Usd::ZERO)
        );
        let token_only: Ceiling =
            serde_json::from_str(r#"{"scope":"goal","tokens":5}"#).expect("a token-only ceiling");
        assert_eq!(
            token_only.usd, None,
            "a refusal against a token cap journals no dollar figure"
        );
        assert_eq!(
            serde_json::to_string(&token_only).expect("serialises"),
            r#"{"scope":"goal","tokens":5}"#
        );
    }

    /// A negative vendor cost is rejected, never subtracted from spend: `Spend.usd` and every
    /// ceiling read only a non-negative integer, so a spend can only grow.
    #[test]
    fn spend_refuses_negative_usd() {
        assert!(serde_json::from_str::<Spend>(r#"{"usd":-1,"tokens":0}"#).is_err());
        assert!(serde_json::from_str::<GoalBudget>(r#"{"usd":-1}"#).is_err());
        assert!(serde_json::from_str::<AgentBudget>(r#"{"usd_per_day":-1}"#).is_err());
        let spend: Spend = serde_json::from_str(r#"{"usd":5,"tokens":1}"#).unwrap();
        assert_eq!(spend.usd, Usd::from_micros(5));
        assert_eq!(
            spend.work_tokens,
            Tokens::ZERO,
            "a record written before the work-token column remains readable"
        );
        let schema = schemars::schema_for!(Spend);
        assert_eq!(
            schema.as_value()["$defs"]["Usd"]["minimum"],
            0,
            "the wire schema refuses it too"
        );
    }

    #[test]
    fn spend_writes_both_token_figures() {
        let spend = Spend {
            usd: Usd::from_micros(5),
            tokens: Tokens::new(100),
            work_tokens: Tokens::new(20),
        };
        assert_eq!(
            serde_json::to_value(spend).unwrap(),
            serde_json::json!({"usd": 5, "tokens": 100, "work_tokens": 20})
        );
    }
}
