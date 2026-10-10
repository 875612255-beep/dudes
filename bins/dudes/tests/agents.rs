//! `dudes agents` lists the daemon's hired, unretired agents over `agent.list`.
#![cfg(unix)]

use std::sync::Arc;

use dudes::exit;
use dudes_proto::AgentView;
use dudes_proto::rpc::agent::HireResult;

mod common;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};

#[test]
fn agents_lists_each_hired_agent_and_says_when_there_is_none() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);

    let none = cli.clone().args(["agents"]).run();
    assert_eq!(none.code(), exit::SUCCESS, "{}", none.output());
    assert_eq!(
        none.stdout,
        "no agents are hired; hire one with `dudes hire` or on the Floor\n"
    );

    let hired = cli
        .clone()
        .args(["--json"])
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;

    let listed = cli.clone().args(["agents"]).run();
    assert_eq!(listed.code(), exit::SUCCESS, "{}", listed.output());
    assert_eq!(
        listed.stdout,
        format!("{agent}  Hector  infra  Claude  Idle\n")
    );

    let machine = cli.args(["--json", "agents"]).run();
    assert_eq!(machine.code(), exit::SUCCESS, "{}", machine.output());
    let views = machine.json::<Vec<AgentView>>();
    assert_eq!(
        views
            .iter()
            .map(|view| (view.id, view.name.as_str(), view.department.as_str()))
            .collect::<Vec<_>>(),
        [(agent, "Hector", "infra")]
    );
}

#[test]
fn agents_needs_the_operator_credential() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let elsewhere = tempfile::tempdir().expect("a runtime directory with no credential");

    let refused = Cli::new(&settings.socket(), elsewhere.path())
        .args(["agents"])
        .run();
    assert_eq!(refused.code(), exit::REFUSED, "{}", refused.output());
    assert!(refused.stdout.is_empty(), "{}", refused.output());
}
