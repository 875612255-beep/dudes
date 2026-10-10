//! The control identity a Floor pause or resume is bound to, and the bytes of a
//! local pause, which that binding must leave unchanged.
//!
//! **On positions in this file.** Each assertion reads a record's position out of the harness
//! journal it was appended to, where the n-th entry is acknowledged at `Lsn(n)`, and compares it
//! with the value the same run's `World` folded. No position is carried from one log to another.

use dudes_proto::record::{BrowserControl, Hex32};
use dudes_proto::{Entry, Record};
use dudes_supervisor::testing::Harness;
use dudes_supervisor::{Command, Input};
use dudes_types::{AgentId, Lsn};

/// The position of the newest journaled record that `is` selects.
fn newest(harness: &Harness, is: impl Fn(&Record) -> bool) -> Lsn {
    let index = harness
        .journal
        .iter()
        .rposition(|entry| is(&entry.record))
        .expect("the record was journaled");
    Lsn(u64::try_from(index).expect("a small journal") + 1)
}

fn control_lsn(harness: &Harness, agent: AgentId) -> Option<Lsn> {
    harness.supervisor.world().agents[&agent].control_lsn
}

fn attribution(control_lsn: Option<Lsn>) -> BrowserControl {
    BrowserControl {
        request_id: Hex32::new([1; 32]),
        request_digest: Hex32::new([2; 32]),
        control_lsn,
        session_digest: Hex32::new([3; 32]),
    }
}

/// `Halt`, `Pause` and `Resume` each move the control identity to their own position, local or
/// browser-attributed; a record that is not one of the three leaves it where it was, and a replay
/// rebuilds the same value.
#[test]
fn control_identity_is_the_newest_pause_resume_or_halt() {
    let mut harness = Harness::new();
    let agent = harness.hire();
    assert_eq!(control_lsn(&harness, agent), None, "nothing journaled yet");

    harness.drive(Input::Command(Command::Halt { agent, text: None }));
    let halt = newest(&harness, |record| matches!(record, Record::Halt { .. }));
    assert_eq!(control_lsn(&harness, agent), Some(halt));

    harness.drive(Input::Command(Command::Pause {
        agent,
        browser: None,
    }));
    let pause = newest(&harness, |record| matches!(record, Record::Pause { .. }));
    assert_eq!(control_lsn(&harness, agent), Some(pause));

    harness.drive(Input::Command(Command::Resume {
        agent,
        browser: Some(attribution(Some(pause))),
    }));
    let resume = newest(&harness, |record| matches!(record, Record::Resume { .. }));
    assert_eq!(control_lsn(&harness, agent), Some(resume));
    let row = &harness.supervisor.world().agents[&agent];
    assert!(!row.paused && !row.halted, "{row:?}");

    harness.drive(Input::Command(Command::Ask {
        agent,
        text: "which branch?".into(),
    }));
    assert_eq!(
        control_lsn(&harness, agent),
        Some(resume),
        "a question is not a control record"
    );

    harness.restart();
    assert_eq!(control_lsn(&harness, agent), Some(resume), "replay agrees");
}

/// A `Pause` written before the browser attribution existed decodes with none, and still moves the
/// control identity: a page that saw it can present its position (the same lesson, applied
/// to control records).
#[test]
fn an_older_format_pause_sets_the_control_identity() {
    let mut harness = Harness::new();
    let agent = harness.hire();
    let older: Record = serde_json::from_value(serde_json::json!({ "t": "pause", "agent": agent }))
        .expect("an older-format pause decodes");
    assert!(matches!(older, Record::Pause { browser: None, .. }));
    let at = harness.journal.last().expect("the hire").at;
    harness.journal.push(Entry { at, record: older });
    let position = Lsn(u64::try_from(harness.journal.len()).expect("a small journal"));

    harness.restart();
    let row = &harness.supervisor.world().agents[&agent];
    assert!(row.paused);
    assert_eq!(row.control_lsn, Some(position));
}

/// The machine persists a local pause and resume with no `browser` key, so their serialized bytes
/// are the ones a daemon built before the attribution wrote.
#[test]
fn a_local_pause_serializes_to_todays_bytes() {
    let mut harness = Harness::new();
    let agent = harness.hire();
    harness.drive(Input::Command(Command::Pause {
        agent,
        browser: None,
    }));
    harness.drive(Input::Command(Command::Resume {
        agent,
        browser: None,
    }));
    for (tag, is) in [
        (
            "pause",
            (|record: &Record| matches!(record, Record::Pause { .. })) as fn(&Record) -> bool,
        ),
        ("resume", |record: &Record| {
            matches!(record, Record::Resume { .. })
        }),
    ] {
        let record = &harness
            .journal
            .iter()
            .rev()
            .find(|entry| is(&entry.record))
            .expect("persisted")
            .record;
        assert_eq!(
            serde_json::to_string(record).expect("record JSON"),
            format!(r#"{{"t":"{tag}","agent":"{agent}"}}"#)
        );
    }
}
