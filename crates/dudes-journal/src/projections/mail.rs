use std::collections::{BTreeMap, BTreeSet};

use dudes_proto::mail_wake::{MailRef, MailWakeMail};
use dudes_proto::{
    Entry, MAX_MESSAGES, MAX_MESSAGES_BYTES, MAX_THREADS, MAX_THREADS_BYTES, MailFrom, MailTo,
    MessageState, MessageView, Record, ThreadView,
};
use dudes_types::{AgentId, Lsn, MailId, MailState, ThreadId, Timestamp};
use redb::{ReadableTable, Table};
use serde::{Deserialize, Serialize};

use super::{RowTable, get_row, put_row};
use crate::error::{Error, Result};

/// Internal mail metadata and delivery state. Browser readers receive only [`MessageView`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MailRow {
    /// The item, repeated so the canonical export exposes a key/row mismatch.
    id: MailId,
    from: MailFrom,
    at: Timestamp,
    lsn: Lsn,
    /// The recipient fixed on the original `Mail` record.
    to: MailTo,
    /// The highest-lsn delivery state folded for this item.
    state: MailState,
    #[serde(default)]
    state_lsn: Lsn,
    #[serde(default)]
    thread: Option<ThreadId>,
    #[serde(default)]
    members: Vec<AgentId>,
}

impl MailRow {
    pub(super) fn wake_fact(&self) -> Option<(AgentId, MailWakeMail)> {
        let MailTo::Agent { agent } = self.to else {
            return None;
        };
        Some((
            agent,
            MailWakeMail::from_state(
                MailRef {
                    id: self.id,
                    lsn: self.lsn,
                },
                &self.state,
                if self.state_lsn == Lsn::ZERO {
                    self.lsn
                } else {
                    self.state_lsn
                },
            ),
        ))
    }
}

/// Apply one entry to the mail table. An unknown state change is corrupt ordering at the producer
/// edge, but projections follow the other row folds: warn and skip rather than inventing a row
/// whose recipient the state-only record cannot supply.
pub(super) fn apply(
    table: &mut RowTable<'_>,
    pending: &mut Table<'_, &'static [u8; 32], &'static [u8]>,
    messages: &mut Table<'_, &'static str, &'static [u8]>,
    lsn: Lsn,
    entry: &Entry,
) -> Result<()> {
    match &entry.record {
        Record::Mail {
            id,
            from,
            to,
            state,
            thread,
            members,
            ..
        } => {
            if get_row::<MailRow>(table, &id.to_bytes())?.is_some() {
                return Ok(());
            }
            let row = MailRow {
                id: *id,
                from: *from,
                at: entry.at,
                lsn,
                to: *to,
                state: state.clone(),
                state_lsn: lsn,
                thread: *thread,
                members: members.clone(),
            };
            put_row(table, &id.to_bytes(), &row)?;
            index(pending, &row)?;
            update_messages(messages, &row)
        }
        Record::MailState { id, state } => {
            let key = id.to_bytes();
            let Some(mut row) = get_row::<MailRow>(table, &key)? else {
                tracing::warn!(
                    target: "journal",
                    lsn = %lsn,
                    mail = %id,
                    kind = ?entry.record.kind(),
                    "projection.unknown_mail"
                );
                return Ok(());
            };
            row.state = state.clone();
            row.state_lsn = lsn;
            put_row(table, &key, &row)?;
            index(pending, &row)?;
            update_messages(messages, &row)
        }
        _ => Ok(()),
    }
}

pub(super) fn recipient_key(agent: AgentId, mail: [u8; 16]) -> [u8; 32] {
    let mut key = [0; 32];
    key[..16].copy_from_slice(&agent.to_bytes());
    key[16..].copy_from_slice(&mail);
    key
}

fn index(table: &mut Table<'_, &'static [u8; 32], &'static [u8]>, row: &MailRow) -> Result<()> {
    if let Some((agent, mail)) = row.wake_fact() {
        let key = recipient_key(agent, row.id.to_bytes());
        if mail.count.is_some() {
            table.insert(&key, b"null".as_slice())?;
        } else {
            table.remove(&key)?;
        }
    }
    Ok(())
}

/// One canonical cut and its complete content-free mail window and thread graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailView {
    /// The last applied journal position read with both tables.
    pub lsn: Lsn,
    /// The final bounded window in original creation order.
    pub messages: Vec<MessageView>,
    /// Fixed membership for exactly the threads referenced by the window.
    pub threads: Vec<ThreadView>,
}

pub(super) fn view(
    table: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
    window: &impl ReadableTable<&'static str, &'static [u8]>,
    lsn: Lsn,
) -> Result<MailView> {
    let messages = messages(window)?;
    let mut threads = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut previous = 0;
    if messages.len() > MAX_MESSAGES || encoded_len(&messages)? > MAX_MESSAGES_BYTES {
        return Err(Error::MailViewCorrupt { mail: None });
    }
    for message in &messages {
        let corrupt = || Error::MailViewCorrupt {
            mail: Some(message.id),
        };
        let creation = message.lsn.parse::<u64>().map_err(|_| corrupt())?;
        if creation == 0
            || creation <= previous
            || creation > lsn.0
            || creation.to_string() != message.lsn
            || !ids.insert(message.id)
        {
            return Err(corrupt());
        }
        previous = creation;
        let row = get_row::<MailRow>(table, &message.id.to_bytes())?.ok_or_else(corrupt)?;
        if row.id != message.id
            || row.from
                != (MailFrom::Agent {
                    agent: message.from,
                })
            || row.to != (MailTo::Agent { agent: message.to })
            || row.at != message.at
            || row.lsn.0 != creation
            || row.thread != message.thread
            || message_state(&row.state) != message.state
        {
            return Err(corrupt());
        }
        let Some(thread) = row.thread else {
            if !row.members.is_empty() {
                return Err(corrupt());
            }
            continue;
        };
        if !(2..=8).contains(&row.members.len())
            || row.members.windows(2).any(|pair| pair[0] >= pair[1])
            || message.from == message.to
            || !row.members.contains(&message.from)
            || !row.members.contains(&message.to)
        {
            return Err(corrupt());
        }
        if let Some(members) = threads.get(&thread) {
            if members != &row.members {
                return Err(corrupt());
            }
        } else {
            threads.insert(thread, row.members);
        }
    }
    let threads: Vec<_> = threads
        .into_iter()
        .map(|(id, members)| ThreadView { id, members })
        .collect();
    if threads.len() > MAX_THREADS || encoded_len(&threads)? > MAX_THREADS_BYTES {
        return Err(Error::MailViewCorrupt { mail: None });
    }
    Ok(MailView {
        lsn,
        messages,
        threads,
    })
}

fn encoded_len(value: &impl Serialize) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|source| Error::Row { source })
}

fn message_state(state: &MailState) -> MessageState {
    match state {
        MailState::Durable => MessageState::Durable,
        MailState::Notified { .. } => MessageState::Notified,
        MailState::Acked => MessageState::Acked,
        MailState::Bounced { .. } => MessageState::Bounced,
    }
}

const WINDOW: &str = "window";

pub(super) fn messages(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
) -> Result<Vec<MessageView>> {
    table
        .get(WINDOW)?
        .map(|value| serde_json::from_slice(value.value()).map_err(|source| Error::Row { source }))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn update_messages(
    table: &mut Table<'_, &'static str, &'static [u8]>,
    row: &MailRow,
) -> Result<()> {
    let (MailFrom::Agent { agent: from }, MailTo::Agent { agent: to }) = (row.from, row.to) else {
        return Ok(());
    };
    let state = message_state(&row.state);
    let mut window = MessageWindow::new();
    for view in messages(table)? {
        if view.id != row.id {
            let lsn = view.lsn.parse().map_err(|source| Error::Row {
                source: <serde_json::Error as serde::de::Error>::custom(source),
            })?;
            window.insert(Lsn(lsn), view)?;
        }
    }
    window.insert(
        row.lsn,
        MessageView {
            id: row.id,
            from,
            to,
            at: row.at,
            lsn: row.lsn.0.to_string(),
            state,
            thread: row.thread,
        },
    )?;
    let rows: Vec<_> = window.rows.into_iter().map(|(_, row, _)| row).collect();
    let bytes = serde_json::to_vec(&rows).map_err(|source| Error::Row { source })?;
    table.insert(WINDOW, bytes.as_slice())?;
    Ok(())
}

struct MessageWindow<T> {
    rows: Vec<(Lsn, T, usize)>,
    bytes: usize,
}

impl<T: Serialize> MessageWindow<T> {
    fn new() -> Self {
        Self {
            rows: Vec::with_capacity(MAX_MESSAGES + 1),
            bytes: 2,
        }
    }

    fn insert(&mut self, lsn: Lsn, row: T) -> Result<()> {
        let bytes = serde_json::to_vec(&row)
            .map_err(|source| Error::Row { source })?
            .len();
        if bytes + 2 > MAX_MESSAGES_BYTES {
            return Ok(());
        }
        let index = self
            .rows
            .binary_search_by_key(&lsn, |(lsn, _, _)| *lsn)
            .unwrap_or_else(|index| index);
        self.bytes += bytes + usize::from(!self.rows.is_empty());
        self.rows.insert(index, (lsn, row, bytes));
        while self.rows.len() > MAX_MESSAGES || self.bytes > MAX_MESSAGES_BYTES {
            let (_, _, removed) = self.rows.remove(0);
            self.bytes -= removed + usize::from(!self.rows.is_empty());
        }
        Ok(())
    }
}

/// Count agent-addressed mail whose latest state remains unread.
pub(super) fn unread_by_agent(
    table: &impl ReadableTable<&'static [u8; 16], &'static [u8]>,
    agent: AgentId,
) -> Result<u32> {
    let mut unread = 0u64;
    for entry in table.iter()? {
        let (_, value) = entry?;
        let row: MailRow =
            serde_json::from_slice(value.value()).map_err(|source| Error::Row { source })?;
        if row.to == (MailTo::Agent { agent })
            && matches!(row.state, MailState::Durable | MailState::Notified { .. })
        {
            // A redb table has a `u64` row count and this iterator yields each key once, so the
            // accumulator cannot overflow before the table itself does.
            unread += 1;
        }
    }
    bounded_unread_count(agent, unread)
}

/// Narrow an exact table count to the public view without disguising range as row corruption.
fn bounded_unread_count(agent: AgentId, count: u64) -> Result<u32> {
    u32::try_from(count).map_err(|_| Error::UnreadCountRange { agent, count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_window_counts_exact_escaped_array_bytes_and_omits_an_oversize_row() {
        let mut window = MessageWindow::new();
        let escaped = "\"\\\u{0001}€".repeat(1_000);
        let row_bytes = serde_json::to_vec(&escaped).unwrap().len();
        assert_eq!(row_bytes, 13_002);
        for lsn in (1..=20).rev() {
            window.insert(Lsn(lsn), escaped.clone()).unwrap();
            assert!(window.rows.len() <= 128);
            let rows: Vec<_> = window.rows.iter().map(|(_, row, _)| row).collect();
            assert_eq!(window.bytes, serde_json::to_vec(&rows).unwrap().len());
            assert!(window.bytes <= 65_536);
        }
        assert_eq!(
            window
                .rows
                .iter()
                .map(|(lsn, _, _)| lsn.0)
                .collect::<Vec<_>>(),
            vec![16, 17, 18, 19, 20]
        );
        let oversized = "\u{0001}".repeat(11_000);
        assert!(serde_json::to_vec(&vec![&oversized]).unwrap().len() > 65_536);
        window.insert(Lsn(21), oversized).unwrap();
        assert_eq!(window.rows.last().unwrap().0, Lsn(20));
        assert_eq!(window.rows.len(), 5);
    }

    #[test]
    fn messages_window_keeps_the_newest_rows_under_continuing_out_of_order_arrivals() {
        let mut window = MessageWindow::new();
        for lsn in (1..=1_000).rev() {
            window.insert(Lsn(lsn), lsn).unwrap();
            assert!(window.rows.len() <= 128);
        }
        assert_eq!(window.rows.len(), 128);
        assert_eq!(window.rows.first().unwrap().0, Lsn(873));
        assert_eq!(window.rows.last().unwrap().0, Lsn(1_000));
    }

    #[test]
    fn unread_count_range_is_not_row_corruption() {
        let agent = AgentId::from_parts(1, 1);
        assert_eq!(
            bounded_unread_count(agent, u64::from(u32::MAX)).unwrap(),
            u32::MAX
        );
        let error = bounded_unread_count(agent, u64::from(u32::MAX) + 1).unwrap_err();
        assert!(
            matches!(
                error,
                Error::UnreadCountRange {
                    agent: found,
                    count,
                } if found == agent && count == u64::from(u32::MAX) + 1
            ),
            "the first unrepresentable exact count has its own diagnosis: {error:?}"
        );
    }
}
