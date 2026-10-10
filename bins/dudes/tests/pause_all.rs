//! The CLI calls `loop.pause` once for each eligible agent in the daemon's current roster.
#![cfg(unix)]

use std::ffi::OsString;
use std::sync::Arc;

use duded::ports::Ask;
use dudes::exit;
use dudes_proto::AgentView;
use dudes_proto::rpc::agent::{HireResult, RetireParams};

mod common;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};

fn hire(cli: &Cli, cwd: &std::path::Path, name: &str) -> dudes_types::AgentId {
    let mut args = common::hire_args(cwd);
    args[2] = OsString::from(name);
    let hired = cli.clone().arg("--json").args(args).run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    hired.json::<HireResult>().agent
}

#[test]
fn pause_all_skips_an_existing_pause_and_reaches_the_other_agents() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);

    let ada = hire(&cli, fixture.state.path(), "Ada");
    let bob = hire(&cli, fixture.state.path(), "Bob");
    let cy = hire(&cli, fixture.state.path(), "Cy");
    let dee = hire(&cli, fixture.state.path(), "Dee");
    let eve = hire(&cli, fixture.state.path(), "Eve");
    running
        .handle
        .ask(Ask::Pause(bob))
        .expect("pause Bob first");
    running.handle.ask(Ask::Halt(dee)).expect("halt Dee first");
    running
        .handle
        .ask(Ask::Retire(Box::new(RetireParams {
            agent: eve,
            reason: "finished before Pause all".to_owned(),
        })))
        .expect("retire Eve first");

    let paused = cli.clone().args(["pause", "--all"]).run();
    assert_eq!(paused.code(), exit::SUCCESS, "{}", paused.output());
    assert_eq!(paused.stdout, "Paused 2 agents.\n");
    let listed = cli.clone().args(["--json", "agents"]).run();
    assert_eq!(listed.code(), exit::SUCCESS, "{}", listed.output());
    let rows = listed.json::<Vec<AgentView>>();
    assert_eq!(rows.len(), 4, "the retired agent is absent from the roster");
    assert_eq!(rows.iter().filter(|row| row.paused).count(), 3);
    assert!(rows.iter().any(|row| row.id == ada && row.paused));
    assert!(rows.iter().any(|row| row.id == bob && row.paused));
    assert!(rows.iter().any(|row| row.id == cy && row.paused));
    assert!(
        rows.iter()
            .any(|row| row.id == dee && row.halted && !row.paused)
    );
    assert!(rows.iter().all(|row| row.id != eve));

    let again = cli.args(["pause", "--all"]).run();
    assert_eq!(again.code(), exit::SUCCESS, "{}", again.output());
    assert_eq!(again.stdout, "Paused 0 agents.\n");
}
