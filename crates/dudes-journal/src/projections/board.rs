use dudes_proto::{Actor, CardView, Entry, Record};
use dudes_types::{CardStatus, Lsn, Seq};
use serde::{Deserialize, Serialize};

use super::{RowTable, all_rows, get_row, put_row};
use crate::error::Result;

#[derive(Serialize, Deserialize)]
pub(super) struct BoardRow {
    #[serde(flatten)]
    pub(super) view: CardView,
    #[serde(default)]
    pub(super) claimed_lsn: Option<Lsn>,
}

pub(super) fn apply(table: &mut RowTable<'_>, lsn: Lsn, entry: &Entry) -> Result<()> {
    match &entry.record {
        Record::CardCreated {
            card,
            title,
            department,
            by,
            deps,
        } => put_row(
            table,
            &card.to_bytes(),
            &BoardRow {
                view: CardView {
                    id: *card,
                    title: title.clone(),
                    department: department.clone(),
                    status: CardStatus::Todo,
                    status_by: *by,
                    seq: Seq::ZERO,
                    claimed_by: None,
                    deps: deps.clone(),
                },
                claimed_lsn: None,
            },
        ),
        Record::Claim {
            card,
            agent,
            expected_seq,
        } => {
            let Some(mut row) = get_row::<BoardRow>(table, &card.to_bytes())? else {
                tracing::warn!(target: "journal", %lsn, %card, "projection.unknown_card");
                return Ok(());
            };
            row.view.status = CardStatus::Claimed;
            row.view.status_by = Actor::Agent { agent: *agent };
            row.view.seq = expected_seq.next();
            row.view.claimed_by = Some(*agent);
            row.claimed_lsn = Some(lsn);
            put_row(table, &card.to_bytes(), &row)
        }
        Record::CardTransition {
            card,
            to,
            by,
            expected_seq,
            ..
        } => {
            let Some(mut row) = get_row::<BoardRow>(table, &card.to_bytes())? else {
                tracing::warn!(target: "journal", %lsn, %card, "projection.unknown_card");
                return Ok(());
            };
            row.view.status = *to;
            row.view.status_by = *by;
            row.view.seq = expected_seq.next();
            put_row(table, &card.to_bytes(), &row)
        }
        Record::AgentRetired { id, .. } => {
            for mut card in all_rows::<BoardRow>(table)? {
                if card.view.claimed_by == Some(*id) {
                    card.view.claimed_by = None;
                    card.claimed_lsn = None;
                    card.view.seq = card.view.seq.next();
                    if card.view.status != CardStatus::Done {
                        card.view.status = CardStatus::Todo;
                        card.view.status_by = Actor::System;
                    }
                    put_row(table, &card.view.id.to_bytes(), &card)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
