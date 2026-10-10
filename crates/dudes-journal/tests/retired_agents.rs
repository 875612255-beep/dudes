//! Retired-agent display facts remain bounded and rebuildable from the journal.

use dudes_journal::{Fsync, Projections, Reader, replay};
use dudes_proto::{Entry, Record, RecordKind, RetiredAgentView, record::samples};
use dudes_types::{AgentId, Lsn, Mode, Timestamp, Tokens, TurnId, Usage, Usd};
use serde_json::json;

const SECRET: &str = "archive-private-sentinel-never-render";

fn entry(record: Record, at: i64) -> Entry {
    Entry {
        at: Timestamp::from_unix_millis(at),
        record,
    }
}
fn hire(id: AgentId, name: &str) -> Record {
    let mut record = samples::record(RecordKind::AgentHired);
    let Record::AgentHired {
        id: agent,
        name: display,
        cwd,
        workspace,
        ..
    } = &mut record
    else {
        unreachable!()
    };
    *agent = id;
    *display = name.into();
    *cwd = SECRET.into();
    *workspace = Some(dudes_proto::record::RecordedWorkspace {
        id: SECRET.into(),
        label: "Parser workspace".into(),
    });
    record
}
fn retire(id: AgentId) -> Record {
    Record::AgentRetired {
        id,
        reason: SECRET.into(),
        browser_retire: None,
    }
}
fn issued(id: AgentId, turn: TurnId) -> Record {
    let mut record = samples::record(RecordKind::TurnIssued);
    let Record::TurnIssued {
        agent,
        turn: selected,
        task,
        ..
    } = &mut record
    else {
        unreachable!()
    };
    *agent = id;
    *selected = turn;
    *task = SECRET.into();
    record
}
fn ended(id: AgentId, turn: TurnId, input: u64, cost: Option<Usd>, mode: Mode) -> Record {
    let mut record = samples::record(RecordKind::TurnEnded);
    let Record::TurnEnded {
        agent,
        turn: selected,
        usage,
        last_message,
        mode: selected_mode,
        ..
    } = &mut record
    else {
        unreachable!()
    };
    *agent = id;
    *selected = turn;
    *last_message = Some(SECRET.into());
    *selected_mode = mode;
    *usage = Some(Usage {
        input: Tokens::new(input),
        usd: cost,
        ..Usage::ZERO
    });
    record
}
fn apply(store: &mut Projections, number: u64, record: Record, at: i64) {
    assert!(store.apply(Lsn(number), 0, &entry(record, at)).unwrap());
}

#[test]
fn retired_view_has_exact_allowlisted_fields_and_normalized_spend() {
    let id = AgentId::from_parts(1, 1);
    let turn = TurnId::from_parts(1, 1);
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, hire(id, "Ada"), 5_000);
    apply(&mut store, 2, samples::record(RecordKind::GoalSet), 4_000);
    apply(&mut store, 3, issued(id, turn), 3_000);
    apply(
        &mut store,
        4,
        ended(id, turn, 70, Some(Usd::from_micros(31_575)), Mode::Managed),
        -86_400_000,
    );
    apply(
        &mut store,
        5,
        ended(id, TurnId::from_parts(1, 2), 30, None, Mode::Attached),
        86_400_000,
    );
    apply(&mut store, 6, retire(id), 1_000);
    let rows = store.retired_agents().unwrap();
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        json!([{
            "id": id, "name": "Ada", "role": "backend engineer", "provider": "claude",
            "workspace_label": "Parser workspace", "hired_at": 5_000, "retired_at": 1_000,
            "turns": 2, "spend": {"usd": 31_575, "tokens": 100, "work_tokens": 100}
        }])
    );
    assert!(store.agent(id).unwrap().is_none());
    assert!(!serde_json::to_string(&rows).unwrap().contains(SECRET));
}

#[test]
fn first_effective_retire_wins_and_unknown_retire_changes_nothing() {
    let id = AgentId::from_parts(1, 1);
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, retire(id), 10);
    assert_eq!(store.retired_agents().unwrap(), []);
    apply(&mut store, 2, hire(id, "Ada"), 5_000);
    apply(&mut store, 3, retire(id), 1_000);
    let first = store.retired_agents().unwrap();
    apply(&mut store, 4, retire(id), 9_000);
    apply(&mut store, 5, retire(AgentId::from_parts(1, 2)), 10_000);
    assert_eq!(store.retired_agents().unwrap(), first);
    assert_eq!(first[0].hired_at, Timestamp::from_unix_millis(5_000));
    assert_eq!(first[0].retired_at, Timestamp::from_unix_millis(1_000));
    assert!(!store.apply(Lsn(3), 0, &entry(retire(id), 20_000)).unwrap());
    assert_eq!(store.retired_agents().unwrap(), first);
}

#[test]
fn late_accepted_ends_refresh_spend_without_recreating_evicted_agents() {
    let mut store = Projections::in_memory().unwrap();
    let id = AgentId::from_parts(1, 1);
    let turn = TurnId::from_parts(1, 1);
    apply(&mut store, 1, hire(id, "Ada"), 10);
    apply(&mut store, 2, issued(id, turn), 20);
    apply(&mut store, 3, retire(id), 30);
    apply(
        &mut store,
        4,
        ended(id, turn, 42, Some(Usd::from_micros(17)), Mode::Managed),
        40,
    );
    let rows = store.retired_agents().unwrap();
    assert_eq!(rows[0].spend.usd, Usd::from_micros(17));
    assert_eq!(rows[0].spend.tokens, Tokens::new(42));
    assert_eq!(rows[0].turns, 1);
    apply(
        &mut store,
        5,
        ended(
            id,
            TurnId::from_parts(99, 99),
            1_000,
            Some(Usd::from_dollars(1)),
            Mode::Managed,
        ),
        50,
    );
    assert_eq!(store.retired_agents().unwrap(), rows);
    for number in 2..=65_u64 {
        let other = AgentId::from_parts(1, u128::from(67 - number));
        apply(
            &mut store,
            number * 2 + 2,
            hire(other, "Other"),
            -i64::try_from(number).unwrap(),
        );
        apply(
            &mut store,
            number * 2 + 3,
            retire(other),
            -i64::try_from(number).unwrap(),
        );
    }
    let retained = store.retired_agents().unwrap();
    assert_eq!(retained.len(), 64);
    assert_eq!(retained[0].id, AgentId::from_parts(1, 65));
    assert_eq!(retained[63].id, AgentId::from_parts(1, 2));
    apply(
        &mut store,
        134,
        ended(id, turn, 9, None, Mode::Managed),
        100,
    );
    assert_eq!(store.retired_agents().unwrap(), retained);
}

#[test]
fn recorded_turns_include_unspawned_managed_turns_and_distinct_attached_ends() {
    let id = AgentId::from_parts(1, 1);
    let managed = TurnId::from_parts(1, 1);
    let attached = TurnId::from_parts(1, 2);
    let late = TurnId::from_parts(1, 3);
    let mut unspawned = Projections::in_memory().unwrap();
    apply(&mut unspawned, 1, hire(id, "Ada"), 10);
    apply(&mut unspawned, 2, issued(id, managed), 20);
    apply(&mut unspawned, 3, retire(id), 30);
    assert_eq!(unspawned.retired_agents().unwrap()[0].turns, 1);

    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, hire(id, "Ada"), 10);
    apply(&mut store, 2, issued(id, managed), 20);
    let spawn = |reissued| Record::TurnSpawned {
        agent: id,
        turn: managed,
        pid: 42,
        start_time: 1,
        boot: None,
        session: None,
        reissued,
        mcp_grant: None,
    };
    apply(&mut store, 3, spawn(false), 30);
    apply(&mut store, 4, spawn(true), 40);
    apply(
        &mut store,
        5,
        ended(id, managed, 10, Some(Usd::from_micros(11)), Mode::Managed),
        50,
    );
    apply(
        &mut store,
        6,
        ended(id, attached, 7, Some(Usd::from_micros(17)), Mode::Attached),
        60,
    );
    apply(
        &mut store,
        7,
        ended(id, attached, 7, Some(Usd::from_micros(17)), Mode::Attached),
        70,
    );
    apply(&mut store, 8, retire(id), 80);
    let row = &store.retired_agents().unwrap()[0];
    assert_eq!(row.turns, 2);
    assert_eq!(row.spend.tokens, Tokens::new(24));
    assert_eq!(row.spend.usd, Usd::from_micros(45));
    apply(&mut store, 9, ended(id, late, 5, None, Mode::Attached), 90);
    let row = &store.retired_agents().unwrap()[0];
    assert_eq!(row.turns, 3);
    assert_eq!(row.spend.tokens, Tokens::new(29));
    apply(
        &mut store,
        10,
        ended(id, late, 5, None, Mode::Attached),
        100,
    );
    apply(
        &mut store,
        11,
        ended(id, managed, 10, Some(Usd::from_micros(11)), Mode::Managed),
        110,
    );
    let row = &store.retired_agents().unwrap()[0];
    assert_eq!(row.turns, 3);
    assert_eq!(row.spend.tokens, Tokens::new(44));
    assert_eq!(row.spend.usd, Usd::from_micros(56));
}

#[test]
fn exact_byte_limit_counts_escaping_utf8_brackets_and_late_total_growth() {
    let id = AgentId::from_parts(1, 1);
    let mut probe = Projections::in_memory().unwrap();
    let prefix = "\"\\\u{0001}€";
    apply(&mut probe, 1, hire(id, prefix), 10);
    apply(&mut probe, 2, retire(id), 20);
    let overhead = serde_json::to_vec(&probe.retired_agents().unwrap())
        .unwrap()
        .len();
    let name = format!("{prefix}{}", "x".repeat(32_768 - overhead));
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, hire(id, &name), 10);
    apply(&mut store, 2, retire(id), 20);
    assert_eq!(
        serde_json::to_vec(&store.retired_agents().unwrap())
            .unwrap()
            .len(),
        32_768
    );
    apply(
        &mut store,
        3,
        ended(id, TurnId::from_parts(1, 1), 10, None, Mode::Attached),
        30,
    );
    assert_eq!(store.retired_agents().unwrap(), []);
    apply(
        &mut store,
        4,
        ended(id, TurnId::from_parts(1, 2), 20, None, Mode::Attached),
        40,
    );
    assert_eq!(store.retired_agents().unwrap(), []);
    let other = AgentId::from_parts(1, 2);
    apply(&mut store, 5, hire(other, "Kept"), 50);
    apply(&mut store, 6, retire(other), 60);
    let oversized = AgentId::from_parts(1, 3);
    apply(&mut store, 7, hire(oversized, &(name + "x")), 10);
    apply(&mut store, 8, retire(oversized), 20);
    assert_eq!(store.retired_agents().unwrap()[0].name, "Kept");
    assert_eq!(store.retired_agents().unwrap().len(), 1);
}

#[test]
fn byte_limit_rechecks_turn_count_growth_without_observed_spend() {
    let id = AgentId::from_parts(1, 1);
    let unknown_end = |number| {
        let mut record = ended(id, TurnId::from_parts(1, number), 0, None, Mode::Attached);
        let Record::TurnEnded { usage, .. } = &mut record else {
            unreachable!()
        };
        *usage = None;
        record
    };
    let seed = |name: &str| {
        let mut store = Projections::in_memory().unwrap();
        apply(&mut store, 1, hire(id, name), 10);
        for number in 1..=9_u64 {
            apply(&mut store, number + 1, unknown_end(u128::from(number)), 20);
        }
        apply(&mut store, 11, retire(id), 30);
        store
    };
    let prefix = "\"\\\u{0001}€";
    let probe = seed(prefix);
    let rows = probe.retired_agents().unwrap();
    assert_eq!(rows[0].turns, 9);
    assert_eq!(rows[0].spend, dudes_proto::Spend::default());
    let overhead = serde_json::to_vec(&rows).unwrap().len();
    let mut store = seed(&format!("{prefix}{}", "x".repeat(32_768 - overhead)));
    assert_eq!(
        serde_json::to_vec(&store.retired_agents().unwrap())
            .unwrap()
            .len(),
        32_768
    );
    apply(&mut store, 12, unknown_end(10), 40);
    assert_eq!(store.retired_agents().unwrap(), []);
    apply(&mut store, 13, unknown_end(11), 50);
    assert_eq!(store.retired_agents().unwrap(), []);
}

#[test]
fn retired_archive_replays_and_exports_identically_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let journal = directory.path().join("journal");
    let db = directory.path().join("state.redb");
    let id = AgentId::from_parts(1, 1);
    let mut store = Projections::open_with(&db, Fsync::None).unwrap();
    let records = [
        entry(hire(id, "Ada"), 5_000),
        entry(retire(id), 1_000),
        entry(
            ended(id, TurnId::from_parts(1, 1), 17, None, Mode::Attached),
            2_000,
        ),
    ];
    {
        let (mut writer, _) = dudes_journal::open(
            &journal,
            dudes_journal::JournalOptions {
                fsync: Fsync::None,
                ..dudes_journal::JournalOptions::default()
            },
        )
        .unwrap();
        for record in &records {
            let (lsn, crc) = writer.append_witnessed(record).unwrap();
            store.apply(lsn, crc, record).unwrap();
        }
    }
    let expected = store.export().unwrap();
    let rows: Vec<RetiredAgentView> = store.retired_agents().unwrap();
    assert_eq!(rows[0].spend.tokens, Tokens::new(17));
    assert_eq!(rows[0].turns, 1);
    drop(store);
    let mut reopened = Projections::open_with(&db, Fsync::None).unwrap();
    replay(&Reader::new(&journal), &mut reopened).unwrap();
    assert_eq!(reopened.export().unwrap(), expected);
    let mut fresh = Projections::in_memory().unwrap();
    replay(&Reader::new(&journal), &mut fresh).unwrap();
    assert_eq!(fresh.retired_agents().unwrap(), rows);
    assert_eq!(fresh.export().unwrap(), expected);
}
