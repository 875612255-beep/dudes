//! The operator board commands drive the real CLI and daemon socket.
#![cfg(unix)]

mod common;

use std::sync::Arc;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};
use dudes_proto::CardView;
use dudes_proto::rpc::ErrorCode;
use dudes_proto::rpc::agent::HireResult;
use dudes_proto::rpc::board::CardCreateResult;
use dudes_types::CardStatus;

#[test]
fn board_create_list_claim_and_move_round_trip_through_the_cli() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let title = "Ship the board; preserve 'literal' text";
    let created = cli
        .clone()
        .args([
            "--json",
            "board",
            "create",
            "--title",
            title,
            "--department",
            "infra",
        ])
        .run();
    assert_eq!(created.code(), 0, "{}", created.output());
    let card = created.json::<CardCreateResult>().card.to_string();
    let hired = cli
        .clone()
        .args(["--json"])
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), 0, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent.to_string();
    let claimed = cli
        .clone()
        .args([
            "--json",
            "board",
            "claim",
            "--card",
            &card,
            "--agent",
            &agent,
            "--expected-seq",
            "0",
        ])
        .run();
    assert_eq!(claimed.code(), 0, "{}", claimed.output());
    let moved = cli
        .clone()
        .args([
            "--json",
            "board",
            "move",
            "--card",
            &card,
            "--to",
            "doing",
            "--expected-seq",
            "1",
        ])
        .run();
    assert_eq!(moved.code(), 0, "{}", moved.output());
    let listed = cli
        .clone()
        .args(["--json", "board", "list", "--status", "doing"])
        .run();
    assert_eq!(listed.code(), 0, "{}", listed.output());
    let cards = listed.json::<Vec<CardView>>();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].title, title);
    assert_eq!(cards[0].status, CardStatus::Doing);
    assert_eq!(cards[0].seq.0, 2);
    let stale = cli
        .args([
            "--json",
            "board",
            "move",
            "--card",
            &card,
            "--to",
            "done",
            "--expected-seq",
            "1",
        ])
        .run();
    assert_eq!(stale.code(), 4, "{}", stale.output());
    assert_eq!(stale.json_error().code, ErrorCode::Conflict.code());
}
