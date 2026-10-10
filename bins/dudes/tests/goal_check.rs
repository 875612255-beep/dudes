//! `dudes goal` refuses a completion check the daemon cannot run, so a goal is never set that every
//! turn could only run out on.
#![cfg(unix)]

use std::sync::Arc;

use dudes::exit;
use dudes_proto::goal::Predicate;
use dudes_proto::record::Record;
use dudes_proto::rpc::agent::HireResult;

mod common;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};

fn goal_with(cli: &Cli, agent: &str, check: &[&str]) -> common::Run {
    cli.clone()
        .args([
            "goal",
            "--agent",
            agent,
            "--text",
            "write tim.txt",
            "--hop-cap",
            "2",
            "--",
        ])
        .args(check)
        .run()
}

fn checks_set(fixture: &Fixture) -> Vec<Vec<String>> {
    fixture
        .records()
        .into_iter()
        .filter_map(|record| match record {
            Record::GoalSet { goal, .. } => match goal.predicate {
                Predicate::Command { argv, .. } => Some(argv),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn a_check_the_daemon_cannot_run_is_refused_at_goal_set() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let repo = tempfile::tempdir().expect("the agent's repository");
    let hired = cli
        .clone()
        .args(["--json"])
        .args(common::hire_args(repo.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent.to_string();

    let missing = goal_with(&cli, &agent, &["no-such-check-program", "tim.txt"]);
    assert_eq!(missing.code(), exit::OTHER, "{}", missing.output());
    assert_eq!(
        missing.stderr,
        "dudes: goal.set: the completion check cannot run: `no-such-check-program` is not on the \
         daemon's PATH (-32602)\n"
    );

    let quoted = goal_with(&cli, &agent, &["test -f tim.txt"]);
    assert_eq!(quoted.code(), exit::OTHER, "{}", quoted.output());
    assert_eq!(
        quoted.stderr,
        "dudes: goal.set: the completion check cannot run: `test -f tim.txt` is not on the \
         daemon's PATH; give each word as its own argument, not one quoted string: test -f \
         tim.txt (-32602)\n"
    );

    let absent = goal_with(&cli, &agent, &["/no/such/check", "tim.txt"]);
    assert_eq!(absent.code(), exit::OTHER, "{}", absent.output());
    assert_eq!(
        absent.stderr,
        "dudes: goal.set: the completion check cannot run: `/no/such/check`: No such file or \
         directory (os error 2) (-32602)\n"
    );

    let runnable = goal_with(&cli, &agent, &["test", "-f", "tim.txt"]);
    assert_eq!(runnable.code(), exit::SUCCESS, "{}", runnable.output());
    assert_eq!(checks_set(&fixture), [["test", "-f", "tim.txt"]]);
}
