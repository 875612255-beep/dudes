//! Cleanup uncertainty is run-local, so every daemon exit leaves the same
//! issued-only journal prefix for replay.
//!
//! # What "a crash between the append and the launch" is, on the disk
//!
//! `Writer::append` returns only after the fsync, and the machine parks the `Spawn` behind that
//! acknowledgement. So the state a crash in that window leaves behind is exactly: a `TurnIssued`
//! durable, and no `TurnSpawned`. That is the whole of it — the rest is what the next start does
//! with it, which is the replay rule that an un-spawned turn is re-issued exactly once.
//!
//! # How the two exit arms construct that state
//!
//! The first run's host reports `CleanupUnconfirmed`. The fake models the runtime result only;
//! native `dudes-host` tests own the retained-process half. The runtime queues no input, leaving a
//! durable `TurnIssued` with no `TurnSpawned` or `TurnEnded`. One arm stops normally. The other
//! starts this test binary as a helper daemon, waits until a real retained host row proves the
//! uncertain launch is in flight, and kills the whole helper process without its exit path. Each
//! second run replays the issued-only prefix.
//!
//! # The control
//!
//! The same fixture without the crash also yields exactly one child:
//! [`the_same_fixture_without_the_refusal_yields_exactly_one_child`].
//! Without it, a daemon that launched nothing at all would pass the arm above.
use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use duded::startup::Witness;
use duded::{ExitCause, boot};
use dudes_proto::Record;
use dudes_providers::launch::{EnvVar, LaunchSpec};

mod common;

use common::{Fixture, ScriptedRunner, TestClock, UnconfirmedHost, seams};

const ABRUPT_ROLE: &str = "DUDES_ABRUPT_UNCERTAIN_ROLE";
const ABRUPT_STATE: &str = "DUDES_ABRUPT_UNCERTAIN_STATE";
const ABRUPT_RUNTIME: &str = "DUDES_ABRUPT_UNCERTAIN_RUNTIME";
const ABRUPT_READY: &str = "DUDES_ABRUPT_UNCERTAIN_READY";
const ABRUPT_READY_PAYLOAD: &str = "spawns=1 owned=1";

fn helper_env(role: &str) -> Vec<EnvVar> {
    let mut env: Vec<EnvVar> = [
        "PATH",
        "HOME",
        "TMPDIR",
        "SystemRoot",
        "windir",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "SystemDrive",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| EnvVar::new(name, value)))
    .collect();
    env.push(EnvVar::new(ABRUPT_ROLE, role));
    env
}

fn abrupt_agent_spec(cwd: &Path) -> LaunchSpec {
    LaunchSpec {
        argv: vec![
            OsString::from(std::env::current_exe().expect("current test executable")),
            "--exact".into(),
            "abrupt_uncertain_daemon_helper".into(),
            "--quiet".into(),
        ],
        env: helper_env("agent"),
        cwd: cwd.to_path_buf(),
        stdin: dudes_providers::launch::LaunchInput::WriteThenClose(vec![b'x'; 1 << 20]),
    }
}

/// The outer helper runs a complete daemon; its inner helper exits to produce the real EPIPE.
#[test]
fn abrupt_uncertain_daemon_helper() {
    let Ok(role) = std::env::var(ABRUPT_ROLE) else {
        return;
    };
    if role == "agent" {
        std::process::exit(0);
    }
    assert_eq!(role, "daemon");

    let state = std::path::PathBuf::from(std::env::var_os(ABRUPT_STATE).expect("state path"));
    let runtime = std::path::PathBuf::from(std::env::var_os(ABRUPT_RUNTIME).expect("runtime path"));
    let ready = std::path::PathBuf::from(std::env::var_os(ABRUPT_READY).expect("ready path"));
    let host = common::RetainedFailureHost::new(abrupt_agent_spec(&state));
    let settings = common::settings_at(&state, &runtime);
    let daemon = boot(
        &settings,
        seams(
            host.clone(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::ticking(1_787_394_600_000, 1)),
            Witness::default(),
        ),
    )
    .expect("helper daemon boots");
    let running = common::start(daemon);
    let agent = common::hire(&running.handle, &state);
    common::set_goal(
        &running.handle,
        common::goal_params(agent, &[common::true_program()]),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while host.spawns() != 1 || host.owned() != 1 {
        assert!(
            Instant::now() < deadline,
            "the helper daemon did not retain exactly one uncertain launch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(&ready, ABRUPT_READY_PAYLOAD).expect("publish abrupt readiness");
    loop {
        std::thread::park();
    }
}

#[allow(
    clippy::disallowed_types,
    reason = "the test harness starts the helper daemon process that it kills abruptly"
)]
fn abrupt_daemon_command(fixture: &Fixture, ready: &Path) -> std::process::Command {
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args(["--exact", "abrupt_uncertain_daemon_helper", "--quiet"])
        .env_clear();
    for var in helper_env("daemon") {
        command.env(var.name(), var.expose_value());
    }
    command
        .env(ABRUPT_STATE, fixture.state.path())
        .env(ABRUPT_RUNTIME, fixture.runtime.path())
        .env(ABRUPT_READY, ready)
        .current_dir(fixture.state.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

fn complete_abrupt_readiness(ready: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(ready) {
        Ok(payload) => Ok((payload == ABRUPT_READY_PAYLOAD).then_some(payload)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn await_abrupt_ready_core(
    deadline: Instant,
    mut read: impl FnMut() -> std::io::Result<Option<String>>,
    mut child_status: impl FnMut() -> std::io::Result<Option<std::process::ExitStatus>>,
    mut sleep: impl FnMut(Duration),
) -> String {
    loop {
        if let Some(payload) = read().expect("read readiness") {
            return payload;
        }
        if let Some(status) = child_status().expect("poll helper daemon") {
            panic!("the helper daemon exited before readiness: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "the helper daemon did not publish readiness"
        );
        sleep(Duration::from_millis(10));
    }
}

fn await_abrupt_ready(child: &mut Child, ready: &Path) -> String {
    await_abrupt_ready_core(
        Instant::now() + Duration::from_secs(20),
        || complete_abrupt_readiness(ready),
        || child.try_wait(),
        std::thread::sleep,
    )
}

fn abrupt_exit_arm() {
    let fixture = Fixture::new();
    let ready = fixture.state.path().join("abrupt-uncertain.ready");
    let mut child = abrupt_daemon_command(&fixture, &ready)
        .spawn()
        .expect("start helper daemon");
    let readiness = await_abrupt_ready(&mut child, &ready);
    assert_eq!(
        readiness, ABRUPT_READY_PAYLOAD,
        "the abrupt exit must intersect one real retained uncertain launch"
    );

    child.kill().expect("abruptly terminate helper daemon");
    let status = child.wait().expect("reap helper daemon");
    assert!(!status.success(), "the abrupt helper exit ran cleanly");

    let prefix = fixture.records();
    assert_eq!(
        count(&prefix, |record| matches!(
            record,
            Record::TurnIssued { .. }
        )),
        1
    );
    assert_eq!(
        count(&prefix, |record| matches!(
            record,
            Record::TurnSpawned { .. }
        )),
        0
    );
    assert_eq!(
        count(&prefix, |record| matches!(record, Record::TurnEnded { .. })),
        0
    );

    let restarted = UnconfirmedHost::new(false);
    let daemon = boot(
        &fixture.settings(),
        seams(
            restarted.clone(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::ticking(1_787_394_700_000, 1)),
            Witness::default(),
        ),
    )
    .expect("restart after abrupt uncertain exit");
    let mut running = common::start(daemon);
    fixture.await_record("reissued TurnSpawned", |record| {
        matches!(record, Record::TurnSpawned { .. }).then_some(())
    });
    running.stop(ExitCause::Ended);

    assert_eq!(restarted.children(), 1, "replay reissued exactly once");
    let records = fixture.records();
    let turns: std::collections::BTreeSet<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::TurnIssued { turn, .. } | Record::TurnSpawned { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(turns.len(), 1, "the abrupt replay kept the turn id");
    assert_eq!(
        count(&records, |record| matches!(
            record,
            Record::TurnSpawned { .. }
        )),
        1
    );
}

#[test]
#[allow(
    clippy::disallowed_types,
    reason = "the test starts and reaps a parked helper through the production waiter"
)]
fn abrupt_ready_waits_for_complete_payload() {
    let dir = tempfile::tempdir().expect("temporary readiness directory");
    let ready = dir.path().join("abrupt-uncertain.ready");
    let mut states = [
        ("absent", None),
        ("empty", Some("")),
        ("partial", Some("spawns=1")),
        ("wrong", Some("spawns=1 owned=0")),
        ("complete", Some(ABRUPT_READY_PAYLOAD)),
    ]
    .into_iter();
    let mut ledger = Vec::new();
    let mut polls = 0;
    let payload = await_abrupt_ready_core(
        Instant::now() + Duration::from_secs(20),
        || {
            let (name, state) = states
                .next()
                .expect("the readiness script must publish completion before it exhausts");
            match state {
                Some(state) => std::fs::write(&ready, state)?,
                None => match std::fs::remove_file(&ready) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                },
            }
            let classified = complete_abrupt_readiness(&ready)?;
            ledger.push((name, classified.clone()));
            Ok(classified)
        },
        || {
            polls += 1;
            Ok(None)
        },
        |_| {},
    );
    assert_eq!(payload, ABRUPT_READY_PAYLOAD);
    assert_eq!(
        ledger,
        vec![
            ("absent", None),
            ("empty", None),
            ("partial", None),
            ("wrong", None),
            ("complete", Some(ABRUPT_READY_PAYLOAD.to_owned())),
        ],
        "the waiter must observe every incomplete publication before completion"
    );
    assert_eq!(
        polls, 4,
        "the child is polled exactly once after each incomplete readiness read"
    );

    let ready = dir.path().join("exited-before-ready");
    let fixture = Fixture::new();
    let mut command = abrupt_daemon_command(&fixture, &ready);
    command.env_remove(ABRUPT_ROLE);
    let mut child = command.spawn().expect("start early-exit helper");
    let panic = catch_unwind(AssertUnwindSafe(|| await_abrupt_ready(&mut child, &ready)))
        .expect_err("an exited helper cannot satisfy readiness");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("non-string readiness panic");
    assert!(
        message.contains("helper daemon exited before readiness"),
        "the waiter must reject an early helper exit: {message}"
    );
}

/// Run one daemon over `fixture` with `host`, hire an agent, set it a goal, and wait for the turn
/// to be issued.
fn one_run(fixture: &Fixture, host: UnconfirmedHost, start_millis: i64) {
    let daemon = boot(
        &fixture.settings(),
        seams(
            host,
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::ticking(start_millis, 1)),
            Witness::default(),
        ),
    )
    .expect("boot");
    let mut running = common::start(daemon);
    let agent = common::hire(&running.handle, fixture.state.path());
    common::set_goal(
        &running.handle,
        common::goal_params(agent, &[common::true_program()]),
    );
    fixture.await_turn();
    running.stop(ExitCause::Ended);
}

/// Invariant 2: the turn is re-issued on replay, exactly once, and exactly one child exists.
#[test]
fn cleanup_unconfirmed_is_run_local_across_any_daemon_exit() {
    let fixture = Fixture::new();

    // Run one: the turn is journaled and the launch never happens.
    let crashed = UnconfirmedHost::new(true);
    one_run(&fixture, crashed.clone(), 1_787_394_600_000);

    let after_first = fixture.records();
    assert_eq!(
        count(&after_first, |record| matches!(
            record,
            Record::TurnIssued { .. }
        )),
        1,
        "the turn was journaled"
    );
    assert_eq!(
        count(&after_first, |record| matches!(
            record,
            Record::TurnSpawned { .. }
        )),
        0,
        "and no launch was recorded: this is the crash window's state, on the disk"
    );
    assert_eq!(
        count(&after_first, |record| matches!(
            record,
            Record::TurnEnded { .. }
        )),
        0,
        "uncertainty is not converted into a durable end"
    );
    assert_eq!(
        crashed.children(),
        0,
        "and no child exists — the 'not two' half starts from zero"
    );

    // Run two: the same state directory, replayed.
    let restarted = UnconfirmedHost::new(false);
    let daemon = boot(
        &fixture.settings(),
        seams(
            restarted.clone(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::ticking(1_787_394_700_000, 1)),
            Witness::default(),
        ),
    )
    .expect("the same state directory boots again");
    let mut running = common::start(daemon);
    fixture.await_record("TurnSpawned", |record| {
        matches!(record, Record::TurnSpawned { .. }).then_some(())
    });
    running.stop(ExitCause::Ended);

    assert_eq!(
        restarted.children(),
        1,
        "exactly one child: replay re-issued the un-spawned turn once, and once only"
    );

    let records = fixture.records();
    let spawned = count(&records, |record| {
        matches!(record, Record::TurnSpawned { .. })
    });
    assert_eq!(spawned, 1, "and exactly one launch is recorded: {spawned}");

    // The turn is the *same* turn, under the same id: a re-issue, not a second turn.
    let turns: std::collections::BTreeSet<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::TurnIssued { turn, .. } | Record::TurnSpawned { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(
        turns.len(),
        1,
        "the re-issue kept the turn's identity: {turns:?}"
    );

    abrupt_exit_arm();
}

/// The control: the same fixture *without* the refusal also yields exactly one.
///
/// Without this arm a daemon that launched nothing at all would satisfy the test above.
#[test]
fn the_same_fixture_without_the_refusal_yields_exactly_one_child() {
    let fixture = Fixture::new();
    let host = UnconfirmedHost::new(false);
    one_run(&fixture, host.clone(), 1_787_394_600_000);

    assert_eq!(host.children(), 1, "one launch, one child");
    let records = fixture.records();
    assert_eq!(
        count(&records, |record| matches!(
            record,
            Record::TurnSpawned { .. }
        )),
        1,
        "and one recorded launch"
    );
}

/// How many records satisfy `f`.
fn count(records: &[Record], f: impl Fn(&Record) -> bool) -> usize {
    records.iter().filter(|record| f(record)).count()
}
