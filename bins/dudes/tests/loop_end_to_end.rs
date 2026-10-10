//! Hire followed by set-goal starts a loop, and tail shows the resulting entries.
//!
//! # Three CLI runs, one daemon, and the child count is the assertion
//!
//! `dudes hire`, then `dudes goal`, then `dudes tail --from 0`. What makes this a test of *the loop*
//! rather than of three method calls is the pairing on the child count: **zero after the hire, one
//! after the goal**. A test that only asserted "a child exists" at the end would pass against a
//! daemon that launched one at `agent.hire`, which is not the loop starting — it is a hire spawning
//! a process for no goal.
//!
//! # What `tail` is asserted to show, and what it is not
//!
//! The records the two commands produced, in lsn order, deserialised as the contract's
//! `JournalAppendedParams`. That is the machine-readable promise: a caller parses the notification
//! type, never a line of prose.
//!
//! It is **not** asserted that `tail` shows everything the daemon will ever write. The daemon is
//! still running when the tail is taken, and asserting on the absence of a later record would be a
//! race with the loop rather than a property of it.
//!
//! # Why the tail terminates without a timeout
//!
//! `journal.tail`'s backfill is written **before** the response to the call that asked for it
//! (`dudes_rpc::methods::journal::tail`), so `dudes tail --from 0` reads notifications until the
//! response arrives and then exits. The backfill is bounded by the journal, so this is deterministic
//! rather than "wait a while and hope". `--follow` is the streaming case and is not used here.
#![cfg(unix)]

use std::sync::Arc;

use dudes::exit;
use dudes_proto::record::RecordKind;
use dudes_proto::rpc::agent::HireResult;
use dudes_proto::rpc::journal::JournalAppendedParams;
use dudes_types::Lsn;

mod common;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};

#[test]
fn hire_then_goal_starts_a_loop_and_tail_shows_the_entries() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let host = SharedHost::new();
    let mut running = start(&settings, host.clone(), Arc::new(OpenRunner::default()));
    let socket = settings.socket();
    let cli = || Cli::new(&socket, &settings.runtime_dir);

    // --- the control: nothing has been launched before the hire ---------------------------------
    assert_eq!(host.children(), 0, "control: no child before anything");

    // --- hire -----------------------------------------------------------------------------------
    let hired = cli()
        .arg("--json")
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;

    // **The second control, and the one that matters.** A hire on its own launches nothing: there
    // is no goal, so the loop has nothing to issue a turn for. Without this assertion a daemon that
    // spawned at `agent.hire` would satisfy every remaining line of this test.
    assert_eq!(
        host.children(),
        0,
        "a hire with no goal starts no turn, so it creates no child"
    );

    // --- the goal, which is what starts the loop -------------------------------------------------
    let set = cli()
        .arg("--json")
        .args(common::goal_args(&agent.to_string(), "make the tests pass"))
        .run();
    assert_eq!(set.code(), exit::SUCCESS, "{}", set.output());

    // The turn is journaled before it is spawned (invariant 2), so the record is what to wait on;
    // the child follows it.
    fixture.await_record("TurnIssued", |record| {
        matches!(record, dudes_proto::Record::TurnIssued { .. }).then_some(())
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while host.children() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "a TurnIssued reached the journal but no child was launched within 10s"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        host.children(),
        1,
        "the goal started exactly one turn, and therefore exactly one child"
    );

    // --- tail shows what the two commands produced ----------------------------------------------
    let tailed = cli().args(["--json", "tail", "--from", "0"]).run();
    assert_eq!(tailed.code(), exit::SUCCESS, "{}", tailed.output());

    let entries: Vec<JournalAppendedParams> = tailed
        .stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("`{line}` is not a journal.appended: {error}"))
        })
        .collect();
    assert!(
        !entries.is_empty(),
        "tail delivered nothing\n{}",
        tailed.output()
    );

    let kinds: Vec<RecordKind> = entries
        .iter()
        .map(|params| params.entry.record.kind())
        .collect();
    for wanted in [
        RecordKind::AgentHired,
        RecordKind::GoalSet,
        RecordKind::TurnIssued,
    ] {
        assert!(
            kinds.contains(&wanted),
            "tail did not show a {wanted:?}; it showed {kinds:?}"
        );
    }

    // In lsn order and starting at the beginning, which is what `--from 0` asked for. Without the
    // ordering assertion a subscriber that delivered the journal shuffled would pass the set
    // assertions above.
    let lsns: Vec<Lsn> = entries.iter().map(|params| params.lsn).collect();
    let mut sorted = lsns.clone();
    sorted.sort_unstable();
    assert_eq!(lsns, sorted, "the backfill is in lsn order");
    assert_eq!(
        kinds.first(),
        Some(&RecordKind::AgentHired),
        "and it starts at the beginning of the journal, because `--from 0` asked it to"
    );

    running.stop();
}

/// `tail` from a position later than the hire shows the goal and not the hire.
///
/// The control for the arm above: without it, a handler that ignored `from_lsn` and always
/// backfilled from zero would pass every assertion there.
#[test]
fn tail_honours_the_position_it_is_given() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let mut running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let socket = settings.socket();
    let cli = || Cli::new(&socket, &settings.runtime_dir);

    let hired = cli()
        .arg("--json")
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;

    let set = cli()
        .arg("--json")
        .args(common::goal_args(&agent.to_string(), "a second goal"))
        .run();
    assert_eq!(set.code(), exit::SUCCESS, "{}", set.output());
    fixture.await_record("GoalSet", |record| {
        matches!(record, dudes_proto::Record::GoalSet { .. }).then_some(())
    });

    // The `GoalSet`'s own position, read off the disk. **Not the hire's lsn plus one**: an `Lsn` is
    // a `(segment, byte offset)` pair packed into a `u64`, so arithmetic on it names a byte and not
    // the next record. Taking a record's own lsn is the only way to say "from here" that survives a
    // rotation.
    let at_goal = fixture
        .entries()
        .iter()
        .find(|(_, entry)| entry.record.kind() == RecordKind::GoalSet)
        .map(|(lsn, _)| *lsn)
        .expect("the goal is on the disk");

    let from_start = cli().args(["--json", "tail", "--from", "0"]).run();
    let from_later = cli()
        .args(["--json", "tail", "--from", &at_goal.0.to_string()])
        .run();

    let kinds = |run: &common::Run| -> Vec<RecordKind> {
        run.stdout
            .lines()
            .map(|line| {
                serde_json::from_str::<JournalAppendedParams>(line)
                    .unwrap_or_else(|error| panic!("`{line}`: {error}"))
                    .entry
                    .record
                    .kind()
            })
            .collect()
    };

    let early = kinds(&from_start);
    let late = kinds(&from_later);
    assert!(
        early.contains(&RecordKind::AgentHired),
        "control: from 0 shows the hire — {early:?}"
    );
    assert!(
        !late.contains(&RecordKind::AgentHired),
        "from after the hire does not — {late:?}"
    );
    assert!(
        late.contains(&RecordKind::GoalSet),
        "but it does show what came after — {late:?}"
    );

    running.stop();
}
