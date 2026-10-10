use dudes_proto::{
    AnsweredQuestionView, Entry, MAX_AGENT_QUESTION_HISTORY, MAX_QUESTION_HISTORY,
    MAX_QUESTION_HISTORY_BYTES, Record,
};
use dudes_types::Lsn;
use redb::{ReadableTable, Table};

use super::{AgentRow, RowTable, get_row};
use crate::error::{Error, Result};

const WINDOW: &str = "window";
type HistoryTable<'txn> = Table<'txn, &'static str, &'static [u8]>;

pub(super) fn read(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
) -> Result<Vec<AnsweredQuestionView>> {
    table
        .get(WINDOW)?
        .map(|value| serde_json::from_slice(value.value()).map_err(|source| Error::Row { source }))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn capture(
    table: &mut HistoryTable<'_>,
    agents: &RowTable<'_>,
    lsn: Lsn,
    entry: &Entry,
) -> Result<()> {
    if let Record::AgentRetired { id, .. } = &entry.record {
        let mut rows = read(table)?;
        let count = rows.len();
        rows.retain(|row| row.agent_id != *id);
        if rows.len() != count {
            if rows.is_empty() {
                table.remove(WINDOW)?;
            } else {
                table.insert(WINDOW, serialize(&rows)?.as_slice())?;
            }
        }
        return Ok(());
    }
    let (agent, browser_answer) = match &entry.record {
        Record::HumanAnswer {
            agent,
            browser_answer,
            ..
        } => (agent, browser_answer.as_ref()),
        Record::ConversationAnswerQueued {
            agent, authority, ..
        } => (
            agent,
            match authority {
                dudes_proto::ConversationAnswerAuthority::Browser { browser } => Some(browser),
                dudes_proto::ConversationAnswerAuthority::Operator { .. } => None,
            },
        ),
        _ => return Ok(()),
    };
    let Some(current) = get_row::<AgentRow>(agents, &agent.to_bytes())? else {
        return Ok(());
    };
    let (Some(question), Some(question_lsn)) = (current.question, current.question_lsn) else {
        return Ok(());
    };
    if question_lsn == Lsn::ZERO
        || question_lsn >= lsn
        || browser_answer
            .as_ref()
            .is_some_and(|answer| answer.question_lsn != question_lsn)
    {
        return Ok(());
    }
    let row = AnsweredQuestionView {
        agent_id: *agent,
        question_lsn: question_lsn.0.to_string(),
        answer_lsn: lsn.0.to_string(),
        question,
    };
    if serialize(std::slice::from_ref(&row))?.len() > MAX_QUESTION_HISTORY_BYTES {
        return Ok(());
    }
    let mut rows = read(table)?;
    rows.push(row);
    while rows.iter().filter(|row| row.agent_id == *agent).count() > MAX_AGENT_QUESTION_HISTORY {
        let oldest = rows
            .iter()
            .position(|row| row.agent_id == *agent)
            .expect("the appended agent has history");
        rows.remove(oldest);
    }
    let mut bytes = serialize(&rows)?;
    while rows.len() > MAX_QUESTION_HISTORY || bytes.len() > MAX_QUESTION_HISTORY_BYTES {
        rows.remove(0);
        bytes = serialize(&rows)?;
    }
    table.insert(WINDOW, bytes.as_slice())?;
    Ok(())
}

fn serialize(rows: &[AnsweredQuestionView]) -> Result<Vec<u8>> {
    serde_json::to_vec(rows).map_err(|source| Error::Row { source })
}
