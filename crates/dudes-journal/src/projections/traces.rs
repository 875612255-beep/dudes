use dudes_proto::{
    AutomaticHold, Entry, ExitStatus, MAX_AGENT_TRACE_BYTES, MAX_AGENT_TRACES, MAX_TRACE_BYTES,
    MAX_TRACES, Record, TraceOutcome, TurnOutcome, TurnTraceView,
};
use dudes_types::{AgentId, TurnId};
use redb::{ReadableTable, Table};
use serde::{Deserialize, Serialize};

use super::{AgentRow, RowTable, TurnRow, get_row};
use crate::error::{Error, Result};

const WINDOW: &str = "window";
type TraceTable<'txn> = Table<'txn, &'static str, &'static [u8]>;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TraceRow {
    turn: TurnId,
    view: TurnTraceView,
}

fn rows(table: &impl ReadableTable<&'static str, &'static [u8]>) -> Result<Vec<TraceRow>> {
    table
        .get(WINDOW)?
        .map(|value| serde_json::from_slice(value.value()).map_err(|source| Error::Row { source }))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn read(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
) -> Result<Vec<TurnTraceView>> {
    Ok(rows(table)?.into_iter().map(|row| row.view).collect())
}

fn serialize(rows: &[TraceRow]) -> Result<Vec<u8>> {
    serde_json::to_vec(rows).map_err(|source| Error::Row { source })
}

fn public_bytes(rows: &[TraceRow]) -> Result<usize> {
    serde_json::to_vec(&rows.iter().map(|row| &row.view).collect::<Vec<_>>())
        .map(|bytes| bytes.len())
        .map_err(|source| Error::Row { source })
}

fn agent_bytes(rows: &[TraceRow], agent: AgentId) -> Result<usize> {
    serde_json::to_vec(
        &rows
            .iter()
            .filter(|row| row.view.agent_id == agent)
            .map(|row| &row.view)
            .collect::<Vec<_>>(),
    )
    .map(|bytes| bytes.len())
    .map_err(|source| Error::Row { source })
}

fn store(table: &mut TraceTable<'_>, rows: &[TraceRow]) -> Result<()> {
    if rows.is_empty() {
        table.remove(WINDOW)?;
    } else {
        table.insert(WINDOW, serialize(rows)?.as_slice())?;
    }
    Ok(())
}

fn outcome(outcome: Option<&TurnOutcome>, exit: ExitStatus) -> TraceOutcome {
    match outcome {
        Some(TurnOutcome::Killed { .. }) => TraceOutcome::Killed,
        Some(
            TurnOutcome::Failed { .. }
            | TurnOutcome::NoResult
            | TurnOutcome::Vanished
            | TurnOutcome::NotLaunched { .. }
            | TurnOutcome::LaunchRefused { .. },
        ) => TraceOutcome::Failed,
        Some(TurnOutcome::Completed | TurnOutcome::MaxTurns | TurnOutcome::BudgetExhausted)
        | None => {
            if exit.code.is_some_and(|code| code != 0) || exit.signal.is_some() {
                TraceOutcome::Failed
            } else {
                TraceOutcome::Ended
            }
        }
    }
}

pub(super) fn capture(
    table: &mut TraceTable<'_>,
    agents: &RowTable<'_>,
    turns: &RowTable<'_>,
    entry: &Entry,
    accepted_end: bool,
) -> Result<()> {
    match &entry.record {
        Record::AgentRetired { id, .. } => {
            let mut current = rows(table)?;
            let len = current.len();
            current.retain(|row| row.view.agent_id != *id);
            if current.len() != len {
                store(table, &current)?;
            }
        }
        Record::AgentHeld {
            agent,
            cause: AutomaticHold::RepeatedFastFailure { latest, .. },
            ..
        } => {
            let mut current = rows(table)?;
            if let Some(row) = current
                .iter_mut()
                .rev()
                .find(|row| row.turn == *latest && row.view.agent_id == *agent)
            {
                row.view.outcome = TraceOutcome::Held;
                store(table, &current)?;
            }
        }
        Record::TurnEnded {
            agent,
            turn,
            mode,
            elapsed_ms,
            usage,
            outcome: end_outcome,
            exit,
            ..
        } if accepted_end => {
            let Some(owner) = get_row::<AgentRow>(agents, &agent.to_bytes())? else {
                return Ok(());
            };
            let Some(turn_row) = get_row::<TurnRow>(turns, &turn.to_bytes())? else {
                return Ok(());
            };
            if turn_row.agent != *agent || turn_row.mode != *mode {
                return Ok(());
            }
            let view = TurnTraceView {
                agent_id: *agent,
                started_at: turn_row.spawned_at,
                duration_ms: *elapsed_ms,
                outcome: outcome(end_outcome.as_ref(), *exit),
                engine: owner.provider,
                tokens: usage.map(|value| value.total_tokens().count().to_string()),
                cost_usd_micros: usage
                    .and_then(|value| value.usd.map(|usd| usd.micros().to_string())),
            };
            let mut current = rows(table)?;
            current.retain(|row| row.turn != *turn);
            current.push(TraceRow { turn: *turn, view });
            while current
                .iter()
                .filter(|row| row.view.agent_id == *agent)
                .count()
                > MAX_AGENT_TRACES
                || agent_bytes(&current, *agent)? > MAX_AGENT_TRACE_BYTES
            {
                let oldest = current
                    .iter()
                    .position(|row| row.view.agent_id == *agent)
                    .expect("the appended agent has a trace");
                current.remove(oldest);
            }
            while current.len() > MAX_TRACES || public_bytes(&current)? > MAX_TRACE_BYTES {
                current.remove(0);
            }
            store(table, &current)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use dudes_proto::{ExitStatus, KillReason, TraceOutcome, TurnOutcome};

    use super::outcome;

    const CLEAN: ExitStatus = ExitStatus {
        code: Some(0),
        signal: None,
    };

    #[test]
    fn every_turn_outcome_maps_to_one_fixed_trace_outcome() {
        let reason = || "private reason".to_owned();
        for (end, expected) in [
            (Some(TurnOutcome::Completed), TraceOutcome::Ended),
            (Some(TurnOutcome::MaxTurns), TraceOutcome::Ended),
            (Some(TurnOutcome::BudgetExhausted), TraceOutcome::Ended),
            (None, TraceOutcome::Ended),
            (
                Some(TurnOutcome::Failed { reason: reason() }),
                TraceOutcome::Failed,
            ),
            (Some(TurnOutcome::NoResult), TraceOutcome::Failed),
            (Some(TurnOutcome::Vanished), TraceOutcome::Failed),
            (
                Some(TurnOutcome::NotLaunched { reason: reason() }),
                TraceOutcome::Failed,
            ),
            (
                Some(TurnOutcome::LaunchRefused { reason: reason() }),
                TraceOutcome::Failed,
            ),
            (
                Some(TurnOutcome::Killed {
                    by: KillReason::Halt,
                }),
                TraceOutcome::Killed,
            ),
        ] {
            assert_eq!(outcome(end.as_ref(), CLEAN), expected, "{end:?}");
        }
    }

    #[test]
    fn a_failed_exit_turns_an_ordinary_end_into_failed() {
        let signalled = ExitStatus {
            code: None,
            signal: Some(9),
        };
        for exit in [
            ExitStatus {
                code: Some(1),
                signal: None,
            },
            signalled,
        ] {
            for end in [
                Some(TurnOutcome::Completed),
                Some(TurnOutcome::MaxTurns),
                Some(TurnOutcome::BudgetExhausted),
                None,
            ] {
                assert_eq!(
                    outcome(end.as_ref(), exit),
                    TraceOutcome::Failed,
                    "{end:?} {exit:?}"
                );
            }
        }
        let killed = TurnOutcome::Killed {
            by: KillReason::Halt,
        };
        assert_eq!(outcome(Some(&killed), signalled), TraceOutcome::Killed);
    }
}
