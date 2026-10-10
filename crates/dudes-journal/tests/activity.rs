//! The activity projection exposes a bounded allowlist of journal facts.

use dudes_journal::Projections;
use dudes_proto::{
    ActivityEvent, ActivityView, Entry, LoopDecision, Record, RecordKind, record::samples,
};
use dudes_types::{CardId, Lsn, Timestamp};
use serde_json::{Value, json};

fn entry(record: Record) -> Entry {
    Entry {
        at: Timestamp::from_unix_millis(1_000),
        record,
    }
}

fn seed() -> Projections {
    let mut store = Projections::in_memory().unwrap();
    store
        .apply(Lsn(1), 0, &entry(samples::record(RecordKind::AgentHired)))
        .unwrap();
    store
        .apply(Lsn(2), 0, &entry(samples::record(RecordKind::CardCreated)))
        .unwrap();
    store
}

#[test]
fn every_record_kind_has_exactly_the_permitted_activity_fields() {
    let agent = json!({"id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "name": "Kim"});
    let card =
        json!({"id": "01ARZ3NDEKTSV4RRFFQ69G5FAZ", "title": "Write the parser", "status": "todo"});
    for kind in RecordKind::ALL {
        let mut store = seed();
        let mut record = samples::record(kind);
        if let Record::LoopDecided { decision, .. } = &mut record {
            *decision = LoopDecision::Done;
        }
        let before = store.activity().unwrap();
        store.apply(Lsn(3), 0, &entry(record.clone())).unwrap();
        let rows = store.activity().unwrap();
        let expected = match kind {
            RecordKind::TurnIssued => Some(json!({"kind":"turn_issued", "agent":agent})),
            RecordKind::TurnEnded => Some(json!({"kind":"turn_ended", "agent":agent})),
            RecordKind::GoalSet => Some(json!({"kind":"goal_set", "agent":agent})),
            RecordKind::LoopDecided => Some(json!({"kind":"goal_done", "agent":agent})),
            RecordKind::AgentRenamed => Some(
                json!({"kind":"agent_renamed", "agent":{"id":"01ARZ3NDEKTSV4RRFFQ69G5FAV", "name":"Kim renamed"}}),
            ),
            RecordKind::AgentRetired => Some(json!({"kind":"agent_retired", "agent":agent})),
            RecordKind::HumanQuestion => Some(json!({"kind":"human_question", "agent":agent})),
            RecordKind::HumanAnswer => Some(json!({"kind":"human_answer", "agent":agent})),
            RecordKind::Pause => Some(json!({"kind":"pause", "agent":agent})),
            RecordKind::Resume => Some(json!({"kind":"resume", "agent":agent})),
            RecordKind::CardCreated => Some(json!({"kind":"card_created", "card":card})),
            RecordKind::Claim => {
                let mut claimed = card.clone();
                claimed["status"] = json!("claimed");
                Some(json!({"kind":"card_claimed", "card":claimed, "agent":agent}))
            }
            RecordKind::CardTransition => {
                let Record::CardTransition { to, .. } = record else {
                    unreachable!()
                };
                let mut moved = card.clone();
                moved["status"] = json!(to);
                Some(json!({"kind":"card_transition", "card":moved}))
            }
            _ => None,
        };
        if let Some(mut expected) = expected {
            expected["lsn"] = json!("3");
            expected["at"] = json!(1_000);
            assert_eq!(
                serde_json::to_value(rows.last().unwrap()).unwrap(),
                expected,
                "{kind:?}"
            );
        } else {
            assert_eq!(rows, before, "{kind:?} must not enter or evict the window");
        }
    }
}

#[test]
fn a_passing_predicate_and_non_done_decisions_are_not_goal_completion_events() {
    let mut store = seed();
    let before = store.activity().unwrap();
    store
        .apply(
            Lsn(3),
            0,
            &entry(samples::record(RecordKind::PredicateResult)),
        )
        .unwrap();
    store
        .apply(Lsn(4), 0, &entry(samples::record(RecordKind::LoopDecided)))
        .unwrap();
    assert_eq!(store.activity().unwrap(), before);
}

#[test]
fn count_eviction_uses_lsn_order_and_ignored_records_never_evict() {
    let mut store = seed();
    let mut entries = Vec::new();
    for number in 3..=100_u64 {
        let record = if number % 2 == 0 {
            samples::record(RecordKind::Pause)
        } else {
            samples::record(RecordKind::Resume)
        };
        let mut row = entry(record);
        row.at = Timestamp::from_unix_millis(-i64::try_from(number).unwrap());
        store.apply(Lsn(number), 0, &row).unwrap();
        entries.push((Lsn(number), row));
    }
    let rows = store.activity().unwrap();
    assert_eq!(rows.len(), 64);
    assert_eq!(rows.first().unwrap().lsn, "37");
    assert_eq!(rows.last().unwrap().lsn, "100");
    assert_eq!(rows.last().unwrap().at, Timestamp::from_unix_millis(-100));
    for number in 101..=200 {
        store
            .apply(Lsn(number), 0, &entry(samples::record(RecordKind::Mail)))
            .unwrap();
    }
    assert_eq!(store.activity().unwrap(), rows);
    let mut replay = seed();
    replay
        .apply_batch(entries.iter().map(|(lsn, row)| (*lsn, 0, row)))
        .unwrap();
    assert_eq!(replay.activity().unwrap(), rows);
    assert_eq!(
        replay
            .export()
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("activity "))
            .collect::<Vec<_>>(),
        store
            .export()
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("activity "))
            .collect::<Vec<_>>()
    );
    assert!(
        !replay
            .apply(Lsn(100), 0, &entries.last().unwrap().1)
            .unwrap()
    );
    assert_eq!(replay.activity().unwrap(), rows);
}

#[test]
fn the_byte_cap_counts_array_punctuation_json_escaping_and_utf8_exactly() {
    let mut store = seed();
    let mut created = samples::record(RecordKind::CardCreated);
    let Record::CardCreated { card, title, .. } = &mut created else {
        unreachable!()
    };
    *card = CardId::from_parts(2, 1);
    *title = "\"\\\u{0001}€".into();
    let event = ActivityView {
        lsn: "3".into(),
        at: Timestamp::from_unix_millis(1_000),
        event: ActivityEvent::CardCreated {
            card: dudes_proto::ActivityCard {
                id: *card,
                title: title.clone(),
                status: dudes_types::CardStatus::Todo,
            },
        },
    };
    let missing = 32_768 - serde_json::to_vec(&[event]).unwrap().len();
    title.push_str(&"x".repeat(missing));
    store.apply(Lsn(3), 0, &entry(created.clone())).unwrap();
    let rows = store.activity().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(serde_json::to_vec(&rows).unwrap().len(), 32_768);
    let Record::CardCreated { title, .. } = &mut created else {
        unreachable!()
    };
    title.push('x');
    store.apply(Lsn(4), 0, &entry(created)).unwrap();
    assert_eq!(
        store.activity().unwrap(),
        rows,
        "an oversized event cannot erase the prior window"
    );
    store
        .apply(Lsn(5), 0, &entry(samples::record(RecordKind::Pause)))
        .unwrap();
    assert_eq!(store.activity().unwrap().len(), 1);
    assert_eq!(store.activity().unwrap()[0].lsn, "5");
}

fn plant(value: &mut Value, sentinel: &str) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                if [
                    "text",
                    "task",
                    "last_message",
                    "body",
                    "subject",
                    "cwd",
                    "body_path",
                    "path",
                    "input_digest",
                ]
                .contains(&key.as_str())
                    && field.is_string()
                {
                    *field = json!(sentinel);
                } else {
                    plant(field, sentinel);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                plant(value, sentinel);
            }
        }
        _ => {}
    }
}

#[test]
fn planted_private_payloads_never_cross_the_activity_projection() {
    const SECRET: &str = "private-activity-sentinel-DO-NOT-DISPLAY";
    let mut store = seed();
    for (index, kind) in RecordKind::ALL.into_iter().enumerate() {
        let mut value = serde_json::to_value(samples::record(kind)).unwrap();
        plant(&mut value, SECRET);
        if kind == RecordKind::AgentRetired {
            value["reason"] = json!(SECRET);
        }
        let record: Record = serde_json::from_value(value).unwrap();
        store
            .apply(Lsn(u64::try_from(index).unwrap() + 3), 0, &entry(record))
            .unwrap();
        let bytes = serde_json::to_string(&store.activity().unwrap()).unwrap();
        assert!(!bytes.contains(SECRET), "{kind:?}");
    }
    assert!(
        !store.activity().unwrap().is_empty(),
        "the safety check cannot pass with an empty feed"
    );
}

#[test]
fn permitted_labels_remain_whole_and_a_rejected_batch_cannot_advance_activity() {
    const LABEL: &str = "allowed-label-positive-control";
    let mut store = seed();
    let mut created = samples::record(RecordKind::CardCreated);
    let Record::CardCreated { title, .. } = &mut created else {
        unreachable!()
    };
    *title = LABEL.into();
    store.apply(Lsn(3), 0, &entry(created)).unwrap();
    assert!(
        serde_json::to_string(&store.activity().unwrap())
            .unwrap()
            .contains(LABEL)
    );
    let before = store.export().unwrap();
    let pause = entry(samples::record(RecordKind::Pause));
    let resume = entry(samples::record(RecordKind::Resume));
    assert!(
        store
            .apply_batch([(Lsn(5), 1, &pause), (Lsn(4), 2, &resume)])
            .is_err()
    );
    assert_eq!(store.export().unwrap(), before);
    assert_eq!(store.last_applied_lsn(), Lsn(3));
}

#[test]
fn rename_updates_current_name_and_only_future_activity_names() {
    let mut store = seed();
    let id = dudes_types::AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    store
        .apply(
            Lsn(3),
            0,
            &entry(Record::Pause {
                agent: id,
                browser: None,
            }),
        )
        .unwrap();
    let before = store.activity().unwrap();
    store
        .apply(
            Lsn(4),
            0,
            &entry(Record::AgentRenamed {
                id,
                name: "New name".to_owned(),
                browser_rename: None,
            }),
        )
        .unwrap();
    assert_eq!(store.agent(id).unwrap().unwrap().name, "New name");
    store
        .apply(
            Lsn(5),
            0,
            &entry(Record::Resume {
                agent: id,
                browser: None,
            }),
        )
        .unwrap();
    let rows = store.activity().unwrap();
    assert_eq!(&rows[..before.len()], &before);
    for row in &rows[before.len()..] {
        let wire = serde_json::to_value(row).unwrap();
        assert_eq!(wire["agent"]["name"], "New name");
    }
}
