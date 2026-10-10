//! Text the daemon hands back reaches the operator's terminal as inert characters, and the daemon
//! admits no control character into a name or a role.
#![cfg(unix)]

use std::sync::Arc;

use dudes::exit;
use dudes_journal::{Fsync, JournalOptions};
use dudes_proto::rpc::ErrorCode;
use dudes_proto::rpc::agent::HireResult;
use dudes_proto::rpc::board::CardCreateResult;
use dudes_proto::{AgentView, CardView, Entry, Record, RecordKind};
use dudes_types::Timestamp;

mod common;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};

/// OSC 52 (writes the clipboard), a CSI, a newline that starts a forged row, and a right-to-left
/// override.
const HOSTILE: &str =
    "\u{1b}]52;c;aGk=\u{7}\u{1b}[2J\n01ARZ3NDEKTSV4RRFFQ69G5FAV  Fake  admin  Claude  Idle\u{202e}";
const SHOWN: &str = r"\u{1b}]52;c;aGk=\u{7}\u{1b}[2J\u{a}01ARZ3NDEKTSV4RRFFQ69G5FAV  Fake  admin  Claude  Idle\u{202e}";

#[test]
fn agents_escape_both_line_separators_and_json_keeps_them_exact() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let mut args = common::hire_args(fixture.state.path());
    args[2] = "He\u{2028}ctor\u{2029}".into();
    args[4] = "in\u{2029}fra\u{2028}".into();
    let hired = cli.clone().arg("--json").args(args).run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;

    let human = cli.clone().args(["agents"]).run();
    assert_eq!(human.code(), exit::SUCCESS, "{}", human.output());
    assert_eq!(
        human.stdout,
        format!(
            "{agent}  He\\u{{2028}}ctor\\u{{2029}}  in\\u{{2029}}fra\\u{{2028}}  Claude  Idle\n"
        )
    );

    let machine = cli.args(["--json", "agents"]).run();
    assert_eq!(machine.code(), exit::SUCCESS, "{}", machine.output());
    let rows = machine.json::<Vec<AgentView>>();
    assert_eq!(rows[0].name, "He\u{2028}ctor\u{2029}");
    assert_eq!(rows[0].role, "in\u{2029}fra\u{2028}");
}

#[test]
fn hire_caps_role_bytes_before_journaling_and_keeps_the_512_byte_role() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let host = SharedHost::new();
    let _running = start(&settings, host.clone(), Arc::new(OpenRunner::default()));
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let hire = |role: &str| {
        let mut args = common::hire_args(fixture.state.path());
        args[4] = role.into();
        cli.clone().arg("--json").args(args).run()
    };

    for role in [
        "x".repeat(513),
        format!("{}x", "é".repeat(256)),
        " ".repeat(513),
    ] {
        let refused = hire(&role);
        assert_eq!(refused.code(), exit::OTHER, "{}", refused.output());
        let error = refused.json_error();
        assert_eq!(error.code, ErrorCode::InvalidParams.code());
        assert_eq!(error.message, "role must contain at most 512 UTF-8 bytes");
    }
    assert!(fixture.records().is_empty());

    for role in [
        "x".repeat(512),
        "é".repeat(256),
        "  infra  ".to_owned(),
        String::new(),
    ] {
        let hired = hire(&role);
        assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
        let agent = hired.json::<HireResult>().agent;
        let listed = cli.clone().args(["--json", "agents"]).run();
        assert_eq!(listed.code(), exit::SUCCESS, "{}", listed.output());
        let rows = listed.json::<Vec<AgentView>>();
        assert_eq!(rows.iter().find(|row| row.id == agent).unwrap().role, role);
    }
    assert_eq!(host.children(), 0);
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|record| record.kind() == RecordKind::AgentHired)
            .count(),
        4
    );
}

#[test]
fn board_list_escapes_a_hostile_card_and_json_keeps_it_exact() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let created = cli
        .clone()
        .args(["--json", "board", "create", "--title", HOSTILE])
        .args(["--department", HOSTILE])
        .run();
    assert_eq!(created.code(), exit::SUCCESS, "{}", created.output());
    let card = created.json::<CardCreateResult>().card;

    let human = cli.clone().args(["board", "list"]).run();
    assert_eq!(human.code(), exit::SUCCESS, "{}", human.output());
    assert_eq!(
        human.stdout,
        format!("{card}  seq 0  Todo  {SHOWN}  {SHOWN}\n")
    );

    let machine = cli.args(["--json", "board", "list"]).run();
    assert_eq!(machine.code(), exit::SUCCESS, "{}", machine.output());
    let cards = machine.json::<Vec<CardView>>();
    assert_eq!(
        (cards[0].title.as_str(), cards[0].department.as_str()),
        (HOSTILE, HOSTILE)
    );
}

#[test]
fn a_name_or_role_holding_a_control_character_is_refused_and_a_bidi_one_is_escaped() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let hire = |name: &str, role: &str| {
        let mut args = common::hire_args(fixture.state.path());
        args[2] = name.into();
        args[4] = role.into();
        cli.clone().arg("--json").args(args).run()
    };

    for (name, role) in [("He\u{1b}[2Jctor", "infra"), ("Hector", "in\nfra")] {
        let refused = hire(name, role);
        assert_eq!(refused.code(), exit::OTHER, "{}", refused.output());
        assert_eq!(
            refused.json_error().code,
            ErrorCode::InvalidParams.code(),
            "{}",
            refused.output()
        );
    }

    let hired = hire("He\u{202e}ctor", "in\u{2066}fra");
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;

    let id = agent.to_string();
    let role = "sa\u{1b}]52;c;aGk=\u{7}les";
    let look = ["--json", "look", &id, "--role", role];
    for args in [
        vec!["--json", "rename", &id, "Hector\u{9b}31m"],
        [
            &look[..],
            &["--character", "automatic", "--accent", "automatic"],
        ]
        .concat(),
    ] {
        let refused = cli.clone().args(args).run();
        assert_eq!(refused.code(), exit::OTHER, "{}", refused.output());
        assert_eq!(
            refused.json_error().code,
            ErrorCode::InvalidParams.code(),
            "{}",
            refused.output()
        );
    }

    let listed = cli.args(["agents"]).run();
    assert_eq!(listed.code(), exit::SUCCESS, "{}", listed.output());
    assert_eq!(
        listed.stdout,
        format!("{agent}  He\\u{{202e}}ctor  in\\u{{2066}}fra  Claude  Idle\n")
    );
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|record| matches!(
                record.kind(),
                RecordKind::AgentHired | RecordKind::AgentRenamed | RecordKind::AgentLookChanged
            ))
            .count(),
        1,
        "only the admitted hire reached the journal"
    );
}

/// A daemon from before the refusal admitted such names, and replay keeps them.
#[test]
fn agents_escape_a_name_journaled_before_the_refusal() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let mut running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let cli = Cli::new(&settings.socket(), &settings.runtime_dir);
    let hired = cli
        .clone()
        .args(["--json"])
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let agent = hired.json::<HireResult>().agent;
    running.stop();

    let (mut writer, _) = dudes_journal::open(
        &fixture.state.path().join(duded::boot::JOURNAL_DIR),
        JournalOptions {
            fsync: Fsync::Full,
            ..JournalOptions::default()
        },
    )
    .expect("the stopped daemon's journal");
    writer
        .append(&Entry {
            at: Timestamp::from_unix_millis(1_787_394_600_000),
            record: Record::AgentRenamed {
                id: agent,
                name: HOSTILE.to_owned(),
                browser_rename: None,
            },
        })
        .expect("append the historical rename");
    drop(writer);

    let _running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let listed = cli.args(["agents"]).run();
    assert_eq!(listed.code(), exit::SUCCESS, "{}", listed.output());
    assert_eq!(
        listed.stdout,
        format!("{agent}  {SHOWN}  infra  Claude  Idle\n")
    );
}
