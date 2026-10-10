//! The real CLI replaces one paused goal and preserves its durable retry result.
#![cfg(unix)]

use std::sync::Arc;

use dudes::exit;
use dudes_proto::Record;
use dudes_proto::rpc::agent::HireResult;
use dudes_proto::rpc::goal::{GoalReplaceResult, GoalSetResult};
use serde_json::json;

mod common;
use common::{Cli, Fixture, OpenRunner, SharedHost, start};

#[test]
fn native_cli_replacement_is_atomic_targeted_and_idempotent() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let host = SharedHost::new();
    let mut running = start(&settings, host.clone(), Arc::new(OpenRunner::default()));
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let hired = cli
        .clone()
        .arg("--json")
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;
    let paused = cli.clone().args(["pause", &agent.to_string()]).run();
    assert_eq!(paused.code(), exit::SUCCESS, "{}", paused.output());
    let set = cli
        .clone()
        .arg("--json")
        .args(common::goal_args(&agent.to_string(), "original goal"))
        .run();
    assert_eq!(set.code(), exit::SUCCESS, "{}", set.output());
    let old = set.json::<GoalSetResult>().goal;
    let mut request = json!({
        "request_id":"11".repeat(32), "agent":agent, "expected_goal":old,
        "text":"replacement from an exact file",
        "predicate":{"command":{"argv":["/usr/bin/false","literal; $(not-a-shell)"],"timeout_s":7}},
        "check":{"id":"cli-check","label":"CLI check"}
    });
    let path = fixture.state.path().join("replacement.json");
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let replace = || {
        cli.clone()
            .args(["--json", "goal", "replace", "--request"])
            .arg(&path)
            .run()
    };
    let changed = replace();
    assert_eq!(changed.code(), exit::SUCCESS, "{}", changed.output());
    let result = changed.json::<GoalReplaceResult>();
    let GoalReplaceResult::Changed {
        goal,
        completion_check,
        ..
    } = &result
    else {
        panic!("changed")
    };
    assert_ne!(*goal, old);
    assert_eq!(completion_check.id, "cli-check");
    let durable = fixture.records();
    assert_eq!(
        durable
            .iter()
            .filter(|entry| matches!(
                entry,
                Record::GoalSet {
                    goal_replacement: Some(_),
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(!durable.iter().any(|entry| matches!(
        entry,
        Record::GoalCleared { .. } | Record::TurnIssued { .. }
    )));
    assert_eq!(host.children(), 0);
    let retried = replace();
    assert_eq!(retried.code(), exit::SUCCESS);
    assert_eq!(retried.json::<GoalReplaceResult>(), result);
    assert_eq!(fixture.records(), durable);
    request["request_id"] = json!("22".repeat(32));
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let stale = replace();
    assert_eq!(stale.code(), 3);
    assert!(matches!(
        stale.json::<GoalReplaceResult>(),
        GoalReplaceResult::Refused {
            reason: dudes_proto::rpc::goal::GoalReplaceRefusal::StaleGoal,
            ..
        }
    ));
    assert_eq!(fixture.records(), durable);
    let resumed = cli.args(["resume", &agent.to_string()]).run();
    assert_eq!(resumed.code(), exit::SUCCESS, "{}", resumed.output());
    fixture.await_record("replacement turn", |entry| {
        matches!(entry, Record::TurnIssued { goal: issued, .. } if *issued == *goal).then_some(())
    });
    running.stop();
}
