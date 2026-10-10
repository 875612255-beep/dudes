//! Integration checks for the bounded Floor trace projection.

use dudes_journal::Projections;
use dudes_proto::{AutomaticHold, Entry, Record, RecordKind, TurnOutcome, record::samples};
use dudes_types::{Lsn, Timestamp, Tokens, TurnId, Usage, Usd};
use serde_json::json;

const SECRET: &str = "private-trace-sentinel-DO-NOT-DISPLAY";

fn sample(kind: RecordKind, at: i64) -> Entry {
    Entry {
        at: Timestamp::from_unix_millis(at),
        record: samples::record(kind),
    }
}

fn apply(store: &mut Projections, lsn: u64, entry: &Entry) {
    assert!(
        store
            .apply(Lsn(lsn), u32::try_from(lsn).unwrap(), entry)
            .unwrap()
    );
}

#[test]
fn a_turn_end_exposes_only_seven_permitted_fields_and_replay_rebuilds_it() {
    let mut issue = sample(RecordKind::TurnIssued, 1_000);
    let Record::TurnIssued { task, .. } = &mut issue.record else {
        unreachable!()
    };
    *task = SECRET.into();
    let mut end = sample(RecordKind::TurnEnded, 1_500);
    let Record::TurnEnded {
        elapsed_ms,
        last_message,
        ..
    } = &mut end.record
    else {
        unreachable!()
    };
    *elapsed_ms = Some(250);
    *last_message = Some(SECRET.into());
    let entries = [
        sample(RecordKind::AgentHired, 900),
        issue,
        sample(RecordKind::TurnSpawned, 1_100),
        end,
    ];
    let mut live = Projections::in_memory().unwrap();
    for (index, entry) in entries.iter().enumerate() {
        apply(&mut live, index as u64 + 1, entry);
    }
    let rows = live.traces().unwrap();
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        json!([{
            "agent_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "started_at": 1100,
            "duration_ms": 250,
            "outcome": "ended",
            "engine": "claude",
            "tokens": "100",
            "cost_usd_micros": "31575"
        }])
    );
    assert!(!serde_json::to_string(&rows).unwrap().contains(SECRET));
    let mut rebuilt = Projections::in_memory().unwrap();
    for (index, entry) in entries.iter().enumerate() {
        apply(&mut rebuilt, index as u64 + 1, entry);
    }
    assert_eq!(rebuilt.traces().unwrap(), rows);
    assert_eq!(rebuilt.export().unwrap(), live.export().unwrap());
}

#[test]
fn hold_updates_only_its_named_turn_and_private_causes_never_cross() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    apply(&mut store, 2, &sample(RecordKind::TurnIssued, 2));
    apply(&mut store, 3, &sample(RecordKind::TurnEnded, 3));
    let mut unrelated = sample(RecordKind::AgentHeld, 4);
    let Record::AgentHeld { cause, .. } = &mut unrelated.record else {
        unreachable!()
    };
    *cause = AutomaticHold::LaunchRefused {
        reason: SECRET.into(),
    };
    apply(&mut store, 4, &unrelated);
    assert_eq!(
        serde_json::to_value(store.traces().unwrap()).unwrap()[0]["outcome"],
        "ended"
    );
    let mut held = sample(RecordKind::AgentHeld, 5);
    let Record::AgentHeld { cause, .. } = &mut held.record else {
        unreachable!()
    };
    let Record::TurnIssued { turn, .. } = samples::record(RecordKind::TurnIssued) else {
        unreachable!()
    };
    *cause = AutomaticHold::RepeatedFastFailure {
        first: TurnId::from_parts(2, 1),
        latest: turn,
    };
    apply(&mut store, 5, &held);
    let rows = store.traces().unwrap();
    assert_eq!(serde_json::to_value(&rows).unwrap()[0]["outcome"], "held");
    assert!(!serde_json::to_string(&rows).unwrap().contains(SECRET));
}

#[test]
fn a_failed_turn_discloses_the_fixed_outcome_without_its_reason() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    apply(&mut store, 2, &sample(RecordKind::TurnIssued, 2));
    let mut end = sample(RecordKind::TurnEnded, 3);
    let Record::TurnEnded { outcome, .. } = &mut end.record else {
        unreachable!()
    };
    *outcome = Some(TurnOutcome::Failed {
        reason: SECRET.into(),
    });
    apply(&mut store, 3, &end);
    let bytes = serde_json::to_string(&store.traces().unwrap()).unwrap();
    assert!(bytes.contains("\"outcome\":\"failed\""));
    assert!(!bytes.contains(SECRET));
}

#[test]
fn attached_and_unknown_managed_ends_do_not_invent_issue_times() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    let mut unknown = sample(RecordKind::TurnEnded, 2);
    let Record::TurnEnded { elapsed_ms, .. } = &mut unknown.record else {
        unreachable!()
    };
    *elapsed_ms = Some(250);
    apply(&mut store, 2, &unknown);
    assert!(store.traces().unwrap().is_empty());
    let mut attached = unknown;
    let Record::TurnEnded { mode, .. } = &mut attached.record else {
        unreachable!()
    };
    *mode = dudes_types::Mode::Attached;
    apply(&mut store, 3, &attached);
    let rows = store.traces().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].started_at, None);
    assert_eq!(rows[0].duration_ms, Some(250));
}

#[test]
fn an_end_mismatched_to_its_issued_agent_or_mode_adds_no_trace() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    let other = dudes_types::AgentId::from_parts(1, 22);
    let mut hire = sample(RecordKind::AgentHired, 1);
    let Record::AgentHired { id, .. } = &mut hire.record else {
        unreachable!()
    };
    *id = other;
    apply(&mut store, 2, &hire);
    apply(&mut store, 3, &sample(RecordKind::TurnIssued, 3));
    let mut wrong_agent = sample(RecordKind::TurnEnded, 4);
    let Record::TurnEnded { agent, .. } = &mut wrong_agent.record else {
        unreachable!()
    };
    *agent = other;
    apply(&mut store, 4, &wrong_agent);
    assert!(store.traces().unwrap().is_empty());
    let mut wrong_mode = sample(RecordKind::TurnEnded, 5);
    let Record::TurnEnded { mode, .. } = &mut wrong_mode.record else {
        unreachable!()
    };
    *mode = dudes_types::Mode::Attached;
    apply(&mut store, 5, &wrong_mode);
    assert!(store.traces().unwrap().is_empty());
}

#[test]
fn repeated_end_replaces_its_turn_and_killed_outcome_is_fixed() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    let first = TurnId::from_parts(2, 1);
    let second = TurnId::from_parts(2, 2);
    for (issue_lsn, spawn_lsn, end_lsn, turn) in [(2, 3, 4, first), (5, 6, 7, second)] {
        let mut issue = sample(RecordKind::TurnIssued, i64::try_from(issue_lsn).unwrap());
        let Record::TurnIssued { turn: id, .. } = &mut issue.record else {
            unreachable!()
        };
        *id = turn;
        apply(&mut store, issue_lsn, &issue);
        let mut spawn = sample(RecordKind::TurnSpawned, i64::try_from(spawn_lsn).unwrap());
        let Record::TurnSpawned { turn: id, .. } = &mut spawn.record else {
            unreachable!()
        };
        *id = turn;
        apply(&mut store, spawn_lsn, &spawn);
        let mut end = sample(RecordKind::TurnEnded, i64::try_from(end_lsn).unwrap());
        let Record::TurnEnded { turn: id, .. } = &mut end.record else {
            unreachable!()
        };
        *id = turn;
        apply(&mut store, end_lsn, &end);
    }
    let mut repeated = sample(RecordKind::TurnEnded, 8);
    let Record::TurnEnded { turn, outcome, .. } = &mut repeated.record else {
        unreachable!()
    };
    *turn = first;
    *outcome = Some(TurnOutcome::Killed {
        by: dudes_proto::KillReason::Halt,
    });
    apply(&mut store, 8, &repeated);
    let rows = store.traces().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].started_at.unwrap().unix_millis(), 6);
    assert_eq!(rows[1].started_at.unwrap().unix_millis(), 3);
    assert_eq!(rows[1].outcome, dudes_proto::TraceOutcome::Killed);
}

#[test]
fn a_reissued_unspawned_turn_uses_its_later_spawn_observation() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    apply(&mut store, 2, &sample(RecordKind::TurnIssued, 2));
    let mut spawn = sample(RecordKind::TurnSpawned, 50);
    let Record::TurnSpawned { reissued, .. } = &mut spawn.record else {
        unreachable!()
    };
    *reissued = true;
    apply(&mut store, 3, &spawn);
    apply(&mut store, 4, &sample(RecordKind::TurnEnded, 52));
    assert_eq!(
        store.traces().unwrap()[0].started_at.unwrap().unix_millis(),
        50
    );
}

#[test]
fn recent_window_is_bounded_per_agent_and_in_total() {
    let mut store = Projections::in_memory().unwrap();
    let mut lsn = 1;
    for agent_number in 1..=16_u128 {
        let mut hire = sample(RecordKind::AgentHired, 1);
        let Record::AgentHired { id, provider, .. } = &mut hire.record else {
            unreachable!()
        };
        *id = dudes_types::AgentId::from_parts(1, agent_number);
        *provider = dudes_types::Provider::Antigravity;
        apply(&mut store, lsn, &hire);
        lsn += 1;
        for turn_number in 1..=9_u128 {
            let turn = TurnId::from_parts(2, agent_number * 100 + turn_number);
            let mut issue = sample(RecordKind::TurnIssued, 2);
            let Record::TurnIssued {
                agent, turn: id, ..
            } = &mut issue.record
            else {
                unreachable!()
            };
            *agent = dudes_types::AgentId::from_parts(1, agent_number);
            *id = turn;
            apply(&mut store, lsn, &issue);
            lsn += 1;
            let mut end = sample(RecordKind::TurnEnded, 3);
            let Record::TurnEnded {
                agent,
                turn: id,
                usage,
                elapsed_ms,
                ..
            } = &mut end.record
            else {
                unreachable!()
            };
            *agent = dudes_types::AgentId::from_parts(1, agent_number);
            *id = turn;
            *elapsed_ms = Some(u64::MAX);
            if let Some(usage) = usage {
                usage.input = Tokens::new(u64::MAX);
                usage.usd = Some(dudes_types::Usd::from_micros(u64::MAX));
            }
            apply(&mut store, lsn, &end);
            lsn += 1;
        }
    }
    let rows = store.traces().unwrap();
    assert!(rows.len() <= dudes_proto::MAX_TRACES);
    assert!(serde_json::to_vec(&rows).unwrap().len() <= dudes_proto::MAX_TRACE_BYTES);
    for agent in rows.iter().map(|row| row.agent_id) {
        let own: Vec<_> = rows.iter().filter(|row| row.agent_id == agent).collect();
        assert!(own.len() <= dudes_proto::MAX_AGENT_TRACES);
        assert!(serde_json::to_vec(&own).unwrap().len() <= dudes_proto::MAX_AGENT_TRACE_BYTES);
    }
    assert!(rows.len() < 90, "the unbounded mutant must fail");
    assert!(
        rows.len() < dudes_proto::MAX_TRACES,
        "large numeric strings must exercise the total byte cap"
    );
    assert!(
        rows.iter()
            .filter(|row| row.agent_id == dudes_types::AgentId::from_parts(1, 16))
            .count()
            < dudes_proto::MAX_AGENT_TRACES,
        "large numeric strings must exercise the per-agent byte cap"
    );
}

#[test]
fn rows_with_spawn_time_and_usage_keep_six_or_seven_turns_per_agent() {
    const SPAWNED: i64 = 1_790_899_200_000;
    for (elapsed, tokens, micros, row_bytes, kept) in [
        (8_000, 45_000, 90_000, 166, 7),
        (240_000, 2_500_000, 1_800_000, 172, 6),
    ] {
        let mut store = Projections::in_memory().unwrap();
        apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
        let mut lsn = 2;
        for turn_number in 1..=9_u32 {
            let turn = TurnId::from_parts(2, u128::from(turn_number));
            let spawned = SPAWNED + i64::from(turn_number) * 600_000;
            let mut issue = sample(RecordKind::TurnIssued, spawned - 1);
            let Record::TurnIssued { turn: id, .. } = &mut issue.record else {
                unreachable!()
            };
            *id = turn;
            let mut spawn = sample(RecordKind::TurnSpawned, spawned);
            let Record::TurnSpawned { turn: id, .. } = &mut spawn.record else {
                unreachable!()
            };
            *id = turn;
            let mut end = sample(RecordKind::TurnEnded, spawned + 1);
            let Record::TurnEnded {
                turn: id,
                elapsed_ms,
                usage,
                ..
            } = &mut end.record
            else {
                unreachable!()
            };
            *id = turn;
            *elapsed_ms = Some(elapsed);
            *usage = Some(Usage {
                input: Tokens::new(tokens),
                usd: Some(Usd::from_micros(micros)),
                ..Usage::ZERO
            });
            for entry in [issue, spawn, end] {
                apply(&mut store, lsn, &entry);
                lsn += 1;
            }
        }
        let rows = store.traces().unwrap();
        assert_eq!(serde_json::to_vec(&rows[0]).unwrap().len(), row_bytes);
        assert_eq!(rows.len(), kept);
        assert_eq!(
            rows.last().unwrap().started_at.unwrap().unix_millis(),
            SPAWNED + 9 * 600_000
        );
    }
}

#[test]
fn count_limits_keep_the_newest_eight_per_agent_and_sixty_four_overall() {
    let mut store = Projections::in_memory().unwrap();
    let mut lsn = 1;
    for agent_number in 1..=9_u128 {
        let agent = dudes_types::AgentId::from_parts(1, agent_number);
        let mut hire = sample(RecordKind::AgentHired, 1);
        let Record::AgentHired { id, .. } = &mut hire.record else {
            unreachable!()
        };
        *id = agent;
        apply(&mut store, lsn, &hire);
        lsn += 1;
        for turn_number in 1..=9_u128 {
            let turn = TurnId::from_parts(2, agent_number * 100 + turn_number);
            let mut issue = sample(RecordKind::TurnIssued, i64::try_from(turn_number).unwrap());
            let Record::TurnIssued {
                agent: id,
                turn: turn_id,
                ..
            } = &mut issue.record
            else {
                unreachable!()
            };
            *id = agent;
            *turn_id = turn;
            apply(&mut store, lsn, &issue);
            lsn += 1;
            let mut spawn = sample(RecordKind::TurnSpawned, i64::try_from(turn_number).unwrap());
            let Record::TurnSpawned {
                agent: id,
                turn: turn_id,
                ..
            } = &mut spawn.record
            else {
                unreachable!()
            };
            *id = agent;
            *turn_id = turn;
            apply(&mut store, lsn, &spawn);
            lsn += 1;
            let mut end = sample(
                RecordKind::TurnEnded,
                i64::try_from(turn_number).unwrap() + 1,
            );
            let Record::TurnEnded {
                agent: id,
                turn: turn_id,
                usage,
                ..
            } = &mut end.record
            else {
                unreachable!()
            };
            *id = agent;
            *turn_id = turn;
            *usage = None;
            apply(&mut store, lsn, &end);
            lsn += 1;
        }
    }
    let rows = store.traces().unwrap();
    assert_eq!(rows.len(), dudes_proto::MAX_TRACES);
    assert_eq!(
        rows.iter()
            .filter(|row| row.agent_id == dudes_types::AgentId::from_parts(1, 9))
            .count(),
        8
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.agent_id == dudes_types::AgentId::from_parts(1, 1))
            .count(),
        0
    );
    assert_eq!(rows.last().unwrap().started_at.unwrap().unix_millis(), 9);
}

#[test]
fn a_private_record_and_retirement_never_leave_a_stale_trace() {
    let mut store = Projections::in_memory().unwrap();
    apply(&mut store, 1, &sample(RecordKind::AgentHired, 1));
    apply(&mut store, 2, &sample(RecordKind::TurnIssued, 2));
    apply(&mut store, 3, &sample(RecordKind::TurnEnded, 3));
    let before = store.traces().unwrap();
    let mut mail = sample(RecordKind::Mail, 4);
    let Record::Mail { subject, .. } = &mut mail.record else {
        unreachable!()
    };
    *subject = SECRET.into();
    apply(&mut store, 4, &mail);
    assert_eq!(store.traces().unwrap(), before);
    apply(&mut store, 5, &sample(RecordKind::AgentRetired, 5));
    assert!(store.traces().unwrap().is_empty());
}
