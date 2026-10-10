//! Answered-question disclosure is finite, allowlisted and rebuilt from the journal.

use dudes_journal::{Fsync, Projections, Reader, replay};
use dudes_proto::{Entry, Record, RecordKind, record::samples};
use dudes_types::{AgentId, Lsn, Timestamp};
use serde_json::json;

const SECRET: &str = "question-history-private-answer-sentinel";

fn entry(record: Record) -> Entry {
    Entry {
        at: Timestamp::UNIX_EPOCH,
        record,
    }
}
fn hire(id: AgentId) -> Record {
    let mut record = samples::record(RecordKind::AgentHired);
    let Record::AgentHired {
        id: agent,
        cwd,
        role,
        ..
    } = &mut record
    else {
        unreachable!()
    };
    *agent = id;
    *cwd = SECRET.into();
    *role = SECRET.into();
    record
}
fn answer(id: AgentId) -> Record {
    Record::HumanAnswer {
        agent: id,
        text: SECRET.into(),
        provenance: None,
        browser_answer: None,
    }
}
fn retire(id: AgentId) -> Record {
    Record::AgentRetired {
        id,
        reason: SECRET.into(),
        browser_retire: None,
    }
}
fn apply(store: &mut Projections, lsn: u64, record: Record) {
    store.apply(Lsn(lsn), 0, &entry(record)).unwrap();
}
fn answered(store: &mut Projections, agent: AgentId, question_lsn: u64, text: &str) {
    apply(
        store,
        question_lsn,
        Record::HumanQuestion {
            agent,
            text: text.into(),
        },
    );
    apply(store, question_lsn + 1, answer(agent));
}
fn seed() -> (Projections, AgentId) {
    let mut store = Projections::in_memory().unwrap();
    let id = AgentId::from_parts(1, 1);
    apply(&mut store, 1, hire(id));
    answered(&mut store, id, 2, "Which branch?");
    (store, id)
}

#[test]
fn history_has_exact_fields_and_omits_answer_and_private_agent_fields() {
    let (store, id) = seed();
    let rows = store.question_history().unwrap();
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        json!([
            {"agent_id":id,"question_lsn":"2","answer_lsn":"3","question":"Which branch?"}
        ])
    );
    assert!(!serde_json::to_string(&rows).unwrap().contains(SECRET));
    assert!(store.agent(id).unwrap().unwrap().question.is_none());
}

#[test]
fn unknown_duplicate_unpositioned_and_mismatched_answers_cannot_invent_history() {
    let (mut store, id) = seed();
    let first = store.question_history().unwrap();
    assert_eq!(first.len(), 1);
    apply(&mut store, 4, answer(AgentId::from_parts(1, 2)));
    apply(&mut store, 5, answer(id));
    let mut waiting = samples::record(RecordKind::LoopDecided);
    let Record::LoopDecided {
        agent, decision, ..
    } = &mut waiting
    else {
        unreachable!()
    };
    *agent = id;
    *decision = dudes_proto::LoopDecision::Wait {
        question: SECRET.into(),
    };
    apply(&mut store, 6, waiting);
    apply(&mut store, 7, answer(id));
    assert_eq!(store.question_history().unwrap(), first);
    apply(
        &mut store,
        8,
        Record::HumanQuestion {
            agent: id,
            text: "Current question".into(),
        },
    );
    let mut stale = samples::record(RecordKind::HumanAnswer);
    let Record::HumanAnswer {
        agent,
        browser_answer,
        ..
    } = &mut stale
    else {
        unreachable!()
    };
    *agent = id;
    *browser_answer = Some(dudes_proto::record::BrowserAnswer {
        question_lsn: Lsn(2),
        request_id: dudes_proto::record::Hex32::new([7; 32]),
        request_digest: dudes_proto::record::Hex32::new([8; 32]),
        session_digest: dudes_proto::record::Hex32::new([9; 32]),
    });
    apply(&mut store, 9, stale);
    assert_eq!(store.question_history().unwrap(), first);
    assert!(!store.apply(Lsn(3), 0, &entry(answer(id))).unwrap());
    assert_eq!(store.question_history().unwrap(), first);
}

#[test]
fn per_agent_and_global_counts_evict_oldest_answer_positions() {
    let (mut store, id) = seed();
    assert_eq!(store.question_history().unwrap().len(), 1);
    for i in 0..9 {
        answered(&mut store, id, 4 + i * 2, &format!("Ask {i}"));
    }
    let rows = store.question_history().unwrap();
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[0].question, "Ask 1");
    let mut position = 22;
    for i in 2..=10 {
        let other = AgentId::from_parts(1, i);
        apply(&mut store, position, hire(other));
        position += 1;
        for _ in 0..8 {
            answered(&mut store, other, position, "Small ask");
            position += 2;
        }
    }
    let rows = store.question_history().unwrap();
    assert_eq!(rows.len(), 64);
    assert!(rows.iter().all(|row| row.agent_id != id));
    assert!(
        rows.windows(2)
            .all(|pair| pair[0].answer_lsn.parse::<u64>().unwrap()
                < pair[1].answer_lsn.parse::<u64>().unwrap())
    );
}

#[test]
fn escaped_byte_limit_and_oversized_row_omission_preserve_prior_history() {
    let (mut store, id) = seed();
    let first = store.question_history().unwrap();
    assert_eq!(first.len(), 1);
    answered(&mut store, id, 4, &"\0".repeat(6000));
    assert_eq!(store.question_history().unwrap(), first);
    answered(&mut store, id, 6, &"雪".repeat(6000));
    answered(&mut store, id, 8, &"雪".repeat(6000));
    let rows = store.question_history().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].answer_lsn, "9");
    assert!(serde_json::to_vec(&rows).unwrap().len() <= 32_768);
}

#[test]
fn retirement_removes_only_the_retired_agents_history_and_export_is_complete() {
    let (mut store, id) = seed();
    let other = AgentId::from_parts(1, 2);
    apply(&mut store, 4, hire(other));
    answered(&mut store, other, 5, "Other question");
    assert_eq!(store.question_history().unwrap().len(), 2);
    apply(
        &mut store,
        7,
        Record::AgentRetired {
            id,
            reason: SECRET.into(),
            browser_retire: None,
        },
    );
    let rows = store.question_history().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_id, other);
    let export = store.export().unwrap();
    assert!(
        export
            .lines()
            .any(|line| line.starts_with("question_history ") && line.contains("Other question"))
    );
}

#[test]
fn replay_and_batch_projection_produce_the_same_answered_window() {
    let dir = tempfile::tempdir().unwrap();
    let id = AgentId::from_parts(1, 1);
    let entries = [
        entry(hire(id)),
        entry(Record::HumanQuestion {
            agent: id,
            text: "Replay ask".into(),
        }),
        entry(answer(id)),
    ];
    let (mut writer, _) = dudes_journal::open(
        dir.path(),
        dudes_journal::JournalOptions {
            fsync: Fsync::None,
            ..dudes_journal::JournalOptions::default()
        },
    )
    .unwrap();
    let positions: Vec<_> = entries
        .iter()
        .map(|row| writer.append_witnessed(row).unwrap())
        .collect();
    drop(writer);
    let mut projected = Projections::in_memory().unwrap();
    replay(&Reader::new(dir.path()), &mut projected).unwrap();
    let mut batched = Projections::in_memory().unwrap();
    batched
        .apply_batch(
            entries
                .iter()
                .zip(&positions)
                .map(|(entry, (lsn, crc))| (*lsn, *crc, entry)),
        )
        .unwrap();
    assert_eq!(projected.question_history().unwrap().len(), 1);
    assert_eq!(
        projected.question_history().unwrap(),
        batched.question_history().unwrap()
    );
    let path = dir.path().join("state.redb");
    let mut cached = Projections::open_with(&path, Fsync::None).unwrap();
    replay(&Reader::new(dir.path()), &mut cached).unwrap();
    drop(cached);
    {
        let database = redb::Database::create(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        transaction
            .open_table(redb::TableDefinition::<&str, u64>::new("meta"))
            .unwrap()
            .insert("schema_version", 23)
            .unwrap();
        transaction
            .delete_table(redb::TableDefinition::<&str, &[u8]>::new(
                "question_history",
            ))
            .unwrap();
        transaction.commit().unwrap();
    }
    let mut reopened = Projections::open_with(&path, Fsync::None).unwrap();
    assert_eq!(reopened.last_applied_lsn(), Lsn::ZERO);
    replay(&Reader::new(dir.path()), &mut reopened).unwrap();
    assert_eq!(
        reopened.question_history().unwrap(),
        projected.question_history().unwrap()
    );
}

#[test]
fn clearing_the_last_history_has_one_canonical_export_live_batch_replay_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("journal");
    let path = dir.path().join("state.redb");
    let id = AgentId::from_parts(1, 1);
    let other = AgentId::from_parts(1, 2);
    let records = [
        hire(id),
        Record::HumanQuestion {
            agent: id,
            text: "First question".into(),
        },
        answer(id),
        hire(other),
        Record::HumanQuestion {
            agent: other,
            text: "Other question".into(),
        },
        answer(other),
        retire(id),
        retire(other),
        retire(other),
        Record::HumanQuestion {
            agent: id,
            text: "Late question".into(),
        },
        answer(id),
        retire(AgentId::from_parts(1, 3)),
    ];
    let entries: Vec<_> = records.into_iter().map(entry).collect();
    let (mut writer, _) = dudes_journal::open(
        &journal,
        dudes_journal::JournalOptions {
            fsync: Fsync::None,
            ..dudes_journal::JournalOptions::default()
        },
    )
    .unwrap();
    let mut live = Projections::in_memory().unwrap();
    let mut disk = Projections::open_with(&path, Fsync::None).unwrap();
    let mut positions = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let (lsn, crc) = writer.append_witnessed(entry).unwrap();
        live.apply(lsn, crc, entry).unwrap();
        disk.apply(lsn, crc, entry).unwrap();
        positions.push((lsn, crc));
        if index == 5 {
            assert_eq!(live.question_history().unwrap().len(), 2);
        }
        if index == 6 {
            let history = live.question_history().unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].agent_id, other);
            assert_eq!(history[0].question, "Other question");
        }
    }
    drop(writer);
    let mut batch = Projections::in_memory().unwrap();
    batch
        .apply_batch(
            entries
                .iter()
                .zip(&positions)
                .map(|(entry, (lsn, crc))| (*lsn, *crc, entry)),
        )
        .unwrap();
    let mut scratch = Projections::in_memory().unwrap();
    replay(&Reader::new(&journal), &mut scratch).unwrap();
    let expected = live.export().unwrap();
    assert_eq!(disk.export().unwrap(), expected);
    assert_eq!(batch.export().unwrap(), expected);
    assert_eq!(scratch.export().unwrap(), expected);
    drop(disk);
    let mut reopened = Projections::open_with(&path, Fsync::None).unwrap();
    assert_eq!(
        replay(&Reader::new(&journal), &mut reopened)
            .unwrap()
            .applied,
        0
    );
    assert_eq!(reopened.export().unwrap(), expected);
    assert_eq!(reopened.question_history().unwrap(), []);
    assert!(
        !expected
            .lines()
            .any(|line| line.starts_with("question_history ")),
        "an empty history is an empty table, including duplicate, unknown and late records"
    );
}
