//! The `budget_by_goal` and `budget_by_day` tables, and the read gate 5 admits against
//! (invariant 4, "budget is admission"; the turn outcome selects the bucket).
//!
//! Both tables are sums over `TurnEnded.usage`, in both modes: an attached turn spends real
//! money and is charged like any other. `budget_by_goal` is keyed by
//! goal and covers the goal's whole life, so it carries over a `GoalReassigned` for free — the
//! key never moves. `budget_by_day` is keyed by `(agent, UTC day of the entry's `at`)`, with a
//! floor row under [`AgentId::NIL`] holding every agent's spend for that day.
//!
//! # Why the views are a type of this crate and not `dudes_policy::BudgetState`
//!
//! `xtask/src/layering.rs` forbids `dudes-journal → dudes-policy`: the journal is the
//! synchronous writer and depends on the contract and nothing that acts. So [`SpendViews`] is
//! built here out of [`dudes_proto::Spend`], which both crates already depend on, and the
//! supervisor maps it into `BudgetState` (field for field, and onto its own `SpendView` too).
//! This is the boundary working, not a workaround for it: importing `dudes-policy` here to
//! return its type is what `cargo xtask layering` exists to catch.

use dudes_proto::{Entry, Record, Spend};
use dudes_types::{AgentId, GoalId, Tokens, Usage, Usd, UtcDay};
use redb::{ReadableTable, Table};
use serde::{Deserialize, Serialize};

use super::turns::EndedTurn;
use super::{AgentRow, RowTable, get_row, put_row};
use crate::error::{Error, Result};

/// The `budget_by_day` table: keyed by [`day_key`], with `serde_json` [`Spend`] rows.
pub(crate) type DayTable<'txn> = Table<'txn, &'static [u8; 20], &'static [u8]>;

/// The `session_totals` table: each confirmed session's last recorded `session_usage`, keyed by
/// the session id, with `serde_json` [`Usage`] rows. It charges nothing; it only tells a reset.
pub(crate) type SessionTable<'txn> = Table<'txn, &'static str, &'static [u8]>;

/// Spend in the three scopes gate 5 admits against, read in one call so a caller cannot assemble
/// two of them from one store state and the third from another.
///
/// Field for field `dudes_policy::BudgetState` and `dudes_supervisor::SpendView`; see the
/// module docs for why it is a separate type rather than either of those.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpendViews {
    /// Spend against the goal over its whole life, from `budget_by_goal`.
    pub goal: Spend,
    /// Unobserved ends against the goal over its whole life.
    pub goal_unknown_ends: u32,
    /// The agent's spend on `today`, from `budget_by_day`.
    pub agent_today: Spend,
    /// The agent's unobserved ends on `today`.
    pub agent_unknown_ends_today: u32,
    /// Every agent's spend on `today`, from the floor row of `budget_by_day`.
    pub floor_today: Spend,
    /// Every agent's unobserved ends on `today`.
    pub floor_unknown_ends_today: u32,
}

/// One budget projection row: observed spend and unobserved ends share one transaction value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct BudgetRow {
    pub(super) spend: Spend,
    pub(super) unknown_ends: u32,
}

/// The `budget_by_day` key: the agent's 16 ULID bytes, then the day as four big-endian bytes
/// with the sign bit flipped.
///
/// The flip is what makes the byte order the numeric order. redb compares keys as bytes, so a
/// plain two's-complement `to_be_bytes` would sort every pre-1970 day *after* every later one;
/// point lookups would still work and a range scan over one agent's days — what a "spend since"
/// read wants — would silently start in the wrong place. Correct on purpose beats correct by
/// accident for the case nobody tested.
fn day_key(agent: AgentId, day: UtcDay) -> [u8; 20] {
    let mut key = [0u8; 20];
    key[..16].copy_from_slice(&agent.to_bytes());
    key[16..]
        .copy_from_slice(&(day.days_since_epoch().cast_unsigned() ^ 0x8000_0000).to_be_bytes());
    key
}

/// Read one `budget_by_day` row; an absent key has no observed spend and no unobserved ends.
fn get_budget(
    table: &impl ReadableTable<&'static [u8; 20], &'static [u8]>,
    key: &[u8; 20],
) -> Result<BudgetRow> {
    let Some(guard) = table.get(key)? else {
        return Ok(BudgetRow::default());
    };
    serde_json::from_slice(guard.value()).map_err(|source| Error::Row { source })
}

/// Write one `budget_by_day` row.
fn put_budget(table: &mut DayTable<'_>, key: &[u8; 20], budget: BudgetRow) -> Result<()> {
    let bytes = serde_json::to_vec(&budget).map_err(|source| Error::Row { source })?;
    table.insert(key, bytes.as_slice())?;
    Ok(())
}

/// Field-wise saturating sum. Kept private: [`Spend`] itself has no arithmetic, and giving it
/// some is a change to the contract crate rather than something to reach across a boundary for.
fn add(a: Spend, b: Spend) -> Spend {
    Spend {
        usd: a.usd.saturating_add(b.usd),
        tokens: a.tokens.saturating_add(b.tokens),
        work_tokens: a.work_tokens.saturating_add(b.work_tokens),
    }
}

/// What one turn's reported usage costs the budget.
///
/// `Usage.usd` is `None` where the provider reported tokens but not dollars. Pricing tokens
/// needs a price table, which is configuration and therefore I/O — not this crate's, and not a
/// projection's at all. So the dollar column under-counts such a turn while neither token column
/// does: `tokens` counts every kind, and `work_tokens`, the figure a token ceiling compares,
/// counts all but prompt-cache reads. A token ceiling therefore still binds on the turn's work,
/// and a turn that only read its prompt cache spends none of it. How that turn *should* be
/// priced is a separate question; nothing here guesses a number.
fn spend_of(usage: Usage) -> Spend {
    Spend {
        usd: usage.usd.unwrap_or(Usd::ZERO),
        tokens: usage.total_tokens(),
        work_tokens: usage
            .input
            .saturating_add(usage.output)
            .saturating_add(usage.cache_creation),
    }
}

/// What a `TurnEnded` with no usage contributes to observed spend: nothing.
///
/// A turn the reaper adopted and then found gone (`TurnOutcome::Vanished`) reports no usage: the
/// process that would have reported it is not there to ask. A zero charge is not a claim that it
/// cost zero: [`apply`] increments `unknown_ends` beside this observed-spend value, and gate 5
/// prices that count from configuration.
///
/// [`apply`] also counts one unknown end for a turn whose running total is a reset of its
/// session's last recorded one, because the vendor's reset can drop spend no line reported.
/// That turn is still charged its usage.
///
/// Deliberately takes the whole [`EndedTurn`] rather than nothing, so a rule that prices by
/// agent, goal or mode does not need this signature widened first.
fn charge_unknown_usage(_ended: &EndedTurn) -> Spend {
    Spend {
        usd: Usd::ZERO,
        tokens: Tokens::ZERO,
        work_tokens: Tokens::ZERO,
    }
}

/// Fold one entry into the two budget tables.
///
/// `ended` comes from [`turns::apply`](super::turns::apply) and is `Some` only for a `TurnEnded`
/// the `turns` table accepted. That is not a convenience: `TurnEnded` carries no goal, so the
/// goal a turn served has to be read from its row, and passing the value rather than re-reading
/// the table is what stops these tables being folded before `turns` has seen the same entry.
/// See [`EndedTurn`].
///
/// A priced zero charge writes no row. An unpriced end does: its zero observed charge is paired
/// with an `unknown_ends` increment in the same row, so the projection cannot advance past the
/// record while losing the reserve input.
///
/// **Every accepted end is charged, including a second one for the same turn.** Rows follow
/// last-write-wins by `lsn`; a sum cannot, because undoing the first charge
/// would need it stored. No v1 emitter produces a second `TurnEnded`, and if one ever does the
/// error is an over-count — which refuses turns rather than admitting them over a ceiling, the
/// only direction invariant 4 can tolerate.
pub(crate) fn apply(
    by_goal: &mut RowTable<'_>,
    by_day: &mut DayTable<'_>,
    sessions: &mut SessionTable<'_>,
    agents: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
    entry: &Entry,
    ended: Option<&EndedTurn>,
) -> Result<()> {
    // Money moves on a turn end, folded below, and on the one other source this projection
    // charges: a memory-condense model call's own reported cost, folded by
    // `charge_condense`. Every other record — `Checkpoint` included — leaves here without
    // reaching either arm.
    if let Record::MemoryCondense {
        agent, model_usage, ..
    } = &entry.record
    {
        return charge_condense(by_goal, by_day, agents, entry, *agent, *model_usage);
    }
    if let Record::ConversationDispatchUncertain { agent, .. } = &entry.record {
        let day = entry.at.utc_day();
        let scopes = [*agent, AgentId::NIL];
        for scope in scopes
            .into_iter()
            .take(if *agent == AgentId::NIL { 1 } else { 2 })
        {
            let key = day_key(scope, day);
            let mut total = get_budget(by_day, &key)?;
            total.unknown_ends = total.unknown_ends.saturating_add(1);
            put_budget(by_day, &key, total)?;
        }
        return Ok(());
    }
    let Some(ended) = ended else { return Ok(()) };
    let reset = session_reset(sessions, ended)?;
    let (charge, unknown_ends) = match ended.usage {
        Some(usage) => (spend_of(usage), u32::from(reset)),
        None => (charge_unknown_usage(ended), 1),
    };
    if charge == Spend::default() && unknown_ends == 0 {
        return Ok(());
    }

    if let Some(goal) = goal_of(agents, ended)? {
        let key = goal.to_bytes();
        let mut total = get_row::<BudgetRow>(by_goal, &key)?.unwrap_or_default();
        total.spend = add(total.spend, charge);
        total.unknown_ends = total.unknown_ends.saturating_add(unknown_ends);
        put_row(by_goal, &key, &total)?;
    }

    // The agent's row and the floor's, in one pass. They collide only when the end names the NIL
    // agent — a malformed record, which a projection may not compound by charging the floor twice.
    let day = entry.at.utc_day();
    let both = [ended.agent, AgentId::NIL];
    let scopes = if ended.agent == AgentId::NIL {
        &both[..1]
    } else {
        &both[..]
    };
    for agent in scopes {
        let key = day_key(*agent, day);
        let mut total = get_budget(by_day, &key)?;
        total.spend = add(total.spend, charge);
        total.unknown_ends = total.unknown_ends.saturating_add(unknown_ends);
        put_budget(by_day, &key, total)?;
    }
    Ok(())
}

/// Charge one memory-condense model call's observed usage — or, when it reported none, one
/// unknown end at the operator's configured reserve, the same treatment [`charge_unknown_usage`]
/// gives a `TurnEnded` with no usage (`None` is not a claim the call was
/// free). `MemoryCondense` names no goal of its own, so — mirroring [`goal_of`]'s reading for an
/// attached turn — it charges whatever goal the agent currently holds, besides the agent's and
/// the floor's day.
fn charge_condense(
    by_goal: &mut RowTable<'_>,
    by_day: &mut DayTable<'_>,
    agents: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
    entry: &Entry,
    agent: AgentId,
    model_usage: Option<Usage>,
) -> Result<()> {
    let (charge, unknown_ends) = match model_usage {
        Some(usage) => (spend_of(usage), 0),
        None => (Spend::default(), 1),
    };
    if charge == Spend::default() && unknown_ends == 0 {
        return Ok(());
    }
    if let Some(goal) = get_row::<AgentRow>(agents, &agent.to_bytes())?.and_then(|row| row.goal) {
        let key = goal.to_bytes();
        let mut total = get_row::<BudgetRow>(by_goal, &key)?.unwrap_or_default();
        total.spend = add(total.spend, charge);
        total.unknown_ends = total.unknown_ends.saturating_add(unknown_ends);
        put_row(by_goal, &key, &total)?;
    }
    let day = entry.at.utc_day();
    let both = [agent, AgentId::NIL];
    let scopes = if agent == AgentId::NIL {
        &both[..1]
    } else {
        &both[..]
    };
    for scope_agent in scopes {
        let key = day_key(*scope_agent, day);
        let mut total = get_budget(by_day, &key)?;
        total.spend = add(total.spend, charge);
        total.unknown_ends = total.unknown_ends.saturating_add(unknown_ends);
        put_budget(by_day, &key, total)?;
    }
    Ok(())
}

/// Whether this end's running total fell below its confirmed session's last recorded one, and
/// record it as the session's last either way.
fn session_reset(sessions: &mut SessionTable<'_>, ended: &EndedTurn) -> Result<bool> {
    let (Some(session), Some(total)) = (&ended.confirmed_session, ended.session_usage) else {
        return Ok(false);
    };
    let last: Option<Usage> = match sessions.get(session.as_str())? {
        Some(guard) => {
            Some(serde_json::from_slice(guard.value()).map_err(|source| Error::Row { source })?)
        }
        None => None,
    };
    let bytes = serde_json::to_vec(&total).map_err(|source| Error::Row { source })?;
    sessions.insert(session.as_str(), bytes.as_slice())?;
    Ok(last.is_some_and(|last| total.checked_sub(last).is_none()))
}

/// Which goal a turn's spend is charged to.
///
/// A managed turn carries its goal on its row. An attached one does not — its row is minted by
/// the end and `TurnEnded` names no goal — so the spend is attributed to whatever goal its agent
/// holds now, from `AgentRow.goal`.
///
/// **The attached case is a choice.** Attached ends count toward budget, but `TurnEnded` does
/// not say which goal they belong to. The alternative — attributing nothing — would let a
/// pairing session burn a goal's ceiling where the budget gate never sees it, so attributing to
/// the agent's current goal is what keeps invariant 4 true. This function is where that choice
/// lives.
fn goal_of(
    agents: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
    ended: &EndedTurn,
) -> Result<Option<GoalId>> {
    if ended.mode == dudes_types::Mode::Managed {
        return Ok(ended.goal);
    }
    Ok(get_row::<AgentRow>(agents, &ended.agent.to_bytes())?.and_then(|row| row.goal))
}

/// Read the three scopes. `today` is the caller's — the supervisor holds the clock; this crate
/// reads none (the same day the caller then stamps on its `LoopDecided`,
/// so `dudes why` rebuilds the input from the journal alone).
pub(crate) fn views(
    by_goal: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
    by_day: &impl ReadableTable<&'static [u8; 20], &'static [u8]>,
    agent: AgentId,
    goal: Option<GoalId>,
    today: UtcDay,
) -> Result<SpendViews> {
    let goal = match goal {
        Some(goal) => get_row::<BudgetRow>(by_goal, &goal.to_bytes())?.unwrap_or_default(),
        None => BudgetRow::default(),
    };
    let agent_today = get_budget(by_day, &day_key(agent, today))?;
    let floor_today = get_budget(by_day, &day_key(AgentId::NIL, today))?;
    Ok(SpendViews {
        goal: goal.spend,
        goal_unknown_ends: goal.unknown_ends,
        agent_today: agent_today.spend,
        agent_unknown_ends_today: agent_today.unknown_ends,
        floor_today: floor_today.spend,
        floor_unknown_ends_today: floor_today.unknown_ends,
    })
}

pub(super) fn agent_total(
    table: &impl ReadableTable<&'static [u8; 20], &'static [u8]>,
    agent: AgentId,
) -> Result<Spend> {
    let mut first = [0u8; 20];
    first[..16].copy_from_slice(&agent.to_bytes());
    let mut last = first;
    last[16..].fill(u8::MAX);
    let mut total = Spend::default();
    for entry in table.range::<&[u8; 20]>(&first..=&last)? {
        let (_, value) = entry?;
        let row: BudgetRow =
            serde_json::from_slice(value.value()).map_err(|source| Error::Row { source })?;
        total = add(total, row.spend);
    }
    Ok(total)
}

/// Every `budget_by_day` row, decoded back into `(agent, day)`. Key order, which the sign-bit
/// flip makes `(agent, day)` numeric order.
pub(crate) fn day_rows(
    table: &impl ReadableTable<&'static [u8; 20], &'static [u8]>,
) -> Result<Vec<((AgentId, UtcDay), Spend)>> {
    let mut rows = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        let key = key.value();
        let mut id = [0u8; 16];
        id.copy_from_slice(&key[..16]);
        let mut day = [0u8; 4];
        day.copy_from_slice(&key[16..]);
        let day = (u32::from_be_bytes(day) ^ 0x8000_0000).cast_signed();
        rows.push((
            (AgentId::from_bytes(id), UtcDay::from_days_since_epoch(day)),
            serde_json::from_slice::<BudgetRow>(value.value())
                .map_err(|source| Error::Row { source })?
                .spend,
        ));
    }
    Ok(rows)
}

/// Every `budget_by_goal` row, with its goal. Key order.
pub(crate) fn goal_rows(
    table: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
) -> Result<Vec<(GoalId, Spend)>> {
    let mut rows = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        rows.push((
            GoalId::from_bytes(*key.value()),
            serde_json::from_slice::<BudgetRow>(value.value())
                .map_err(|source| Error::Row { source })?
                .spend,
        ));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The day component of a [`day_key`] sorts as the number does, in both signs. A plain
    /// two's-complement encoding fails the second assertion, which is the one a range scan
    /// over an agent's days would depend on.
    #[test]
    fn the_day_key_sorts_as_the_day_does() {
        let a = AgentId::from_parts(1, 1);
        let day = |d: i32| day_key(a, UtcDay::from_days_since_epoch(d));
        assert!(day(20_687) < day(20_688));
        assert!(day(-1) < day(0), "a pre-epoch day sorts before the epoch");
        assert!(day(i32::MIN) < day(i32::MAX));
        // And the agent is the major component: another agent's whole range is elsewhere.
        let b = AgentId::from_parts(1, 2);
        assert!(day(i32::MAX) < day_key(b, UtcDay::from_days_since_epoch(i32::MIN)));
    }

    /// `usd: None` means "the provider reported tokens but not dollars", not "it was free": the
    /// tokens are still charged to both token columns, so a token ceiling, which compares work
    /// tokens, binds even where the dollar one under-counts. This usage has no prompt-cache
    /// reads, so the two columns agree.
    #[test]
    fn usage_without_a_reported_cost_still_charges_tokens() {
        let usage = Usage {
            input: Tokens::new(30),
            output: Tokens::new(12),
            usd: None,
            ..Usage::ZERO
        };
        assert_eq!(
            spend_of(usage),
            Spend {
                usd: Usd::ZERO,
                tokens: Tokens::new(42),
                work_tokens: Tokens::new(42),
            }
        );
        // Control: the same usage with a cost carries it, so the zero above is the missing
        // `usd` and not a `spend_of` that always reports zero dollars.
        assert_eq!(
            spend_of(Usage {
                usd: Some(Usd::from_cents(7)),
                ..usage
            }),
            Spend {
                usd: Usd::from_cents(7),
                tokens: Tokens::new(42),
                work_tokens: Tokens::new(42),
            }
        );
    }

    /// The projection keeps the provider's reported cost and the display's all-kinds token total
    /// beside the work-token figure admission uses. Cache reads distinguish the two token columns.
    #[test]
    fn usage_keeps_all_kinds_tokens_and_vendor_cost_beside_work_tokens() {
        let charge = spend_of(Usage {
            input: Tokens::new(30),
            output: Tokens::new(12),
            cache_read: Tokens::new(1_000),
            cache_creation: Tokens::new(8),
            usd: Some(Usd::from_cents(7)),
        });
        assert_eq!(charge.usd, Usd::from_cents(7));
        assert_eq!(charge.tokens, Tokens::new(1_050));
        assert_eq!(charge.work_tokens, Tokens::new(50));
    }
}
