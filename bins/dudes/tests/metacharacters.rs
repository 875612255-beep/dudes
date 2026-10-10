//! A goal containing shell metacharacters arrives at the daemon byte-identical, and the CLI spawns
//! no interpreter.
//!
//! # Byte-identical, against a fixture that fails loudly if it lost its own teeth
//!
//! The text below is passed as one `argv` element to the real binary and read back out of the
//! **journal** — the daemon's own durable record, not the CLI's echo. Comparing them is the
//! assertion.
//!
//! The trap this file is built to avoid: a fixture whose metacharacters were eaten on the way in
//! would round-trip perfectly and prove nothing. So before comparing, the test asserts the *input*
//! still contains each metacharacter it is supposed to carry. Without that arm, `""` round-trips
//! byte-identically too.
//!
//! # A predicate's argv makes the same point from the other side
//!
//! The goal's `text` is a value the daemon stores; a predicate's `argv` is a value the daemon will
//! one day *run*. Both are asserted, because a CLI that carried text safely and flattened argv into
//! a string would satisfy invariant 7 for the harmless half only.
//!
//! # What cannot be carried, said plainly rather than pretended about
//!
//! A NUL byte cannot appear in any argument of any process on Unix — `execve` takes NUL-terminated
//! strings — so no argv-driven program can carry one, and this file does not claim to. Every other
//! byte a shell treats as special is ordinary here.
#![cfg(unix)]

use std::sync::Arc;

use dudes::exit;
use dudes_proto::Record;
use dudes_proto::goal::Predicate;
use dudes_proto::rpc::agent::HireResult;

mod common;

use common::{Cli, Fixture, OpenRunner, SharedHost, start};

/// Every metacharacter a shell would act on, in one string, plus a **real newline** and a tab.
///
/// The newline is not decoration. This transport is newline-**delimited** JSON: a goal text carrying
/// one has to survive the framing as an escape inside the frame rather than ending it, and a
/// framing bug would split the request in two and produce a parse error rather than a wrong value.
/// So this one character exercises the codec as well as the parser.
const HOSTILE: &str =
    "; rm -rf / $(whoami) `id` && echo \"x\" 'y' | tee >(cat) # \\ \n a real newline \t a tab";

/// The pieces `HOSTILE` must still contain when the test runs.
///
/// This is the fixture's own control. A test that only compared input to output would pass against
/// an input that had been stripped to nothing.
const PIECES: &[&str] = &[
    ";",
    "rm -rf /",
    "$(whoami)",
    "`id`",
    "&&",
    "\"x\"",
    "'y'",
    "|",
    ">(cat)",
    "#",
    "\\",
    "\n",
    "\t",
];

#[test]
fn a_goals_text_and_its_predicate_argv_arrive_byte_identical() {
    // --- the fixture's own control, first --------------------------------------------------------
    for piece in PIECES {
        assert!(
            HOSTILE.contains(piece),
            "the fixture lost `{}`, so the round trip below would prove nothing",
            piece.escape_debug()
        );
    }

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

    // The goal's text *and* one predicate argument are hostile, so both halves are exercised in one
    // call: the value the daemon stores and the value it will one day run.
    let set = cli()
        .args(["goal", "--agent"])
        .arg(agent.to_string())
        .arg("--text")
        .arg(HOSTILE)
        .args([
            "--usd",
            "5",
            "--hop-cap",
            "8",
            "--check-may-already-pass",
            "--",
        ])
        .args(["/usr/bin/true"])
        .arg(HOSTILE)
        .run();
    assert_eq!(set.code(), exit::SUCCESS, "{}", set.output());

    // --- read it back off the daemon's own durable record ---------------------------------------
    let goal = fixture.await_record("GoalSet", |record| match record {
        Record::GoalSet { goal, .. } => Some(goal.clone()),
        _ => None,
    });

    assert_eq!(
        goal.text, HOSTILE,
        "the goal's text is not what was typed:\n typed: {:?}\n stored: {:?}",
        HOSTILE, goal.text
    );

    let Predicate::Command { argv, .. } = &goal.predicate else {
        panic!("the predicate is a command: {:?}", goal.predicate)
    };
    assert_eq!(
        argv,
        &vec!["/usr/bin/true".to_owned(), HOSTILE.to_owned()],
        "the predicate's argv is a typed list of the parts that were typed, in order"
    );
    assert_eq!(
        argv.len(),
        2,
        "two elements — a CLI that had joined them into one string would have one"
    );

    running.stop();
}

/// **A help token in the predicate's argv reaches the daemon.**
///
/// # This is a regression arm, and the class it guards against was a silent success
///
/// Help must not be decided by a scan of the **whole** argv before the `--` split and before any
/// value-taking flag has taken its value. Otherwise `goal … -- /usr/bin/make --help`, `--text help`,
/// `goal … -- /usr/bin/sort -h` and `hire --name help` would print the usage and exit **`0`
/// having sent nothing** — a no-op reported as success, on the door a loop drives. `exit::SUCCESS`
/// means the daemon ran the method, so that exit code would not be merely unhelpful, it would be wrong.
///
/// The control is the ordinary goal run first on the same daemon: it is the arm that fails if goals
/// have stopped working altogether, which is what would otherwise let a parser that simply rejected
/// every one of these lines look like a fix.
#[test]
fn a_help_token_in_the_predicate_argv_still_reaches_the_daemon() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let mut running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let socket = settings.socket();
    let cli = || Cli::new(&socket, &settings.runtime_dir);
    let agent = hire(&cli(), &fixture);

    // --- the control, first: an ordinary goal on this daemon works --------------------------------
    let ordinary = cli()
        .args(goal_line(&agent, "an ordinary goal", &["/usr/bin/true"]))
        .run();
    assert_eq!(
        ordinary.code(),
        exit::SUCCESS,
        "control: an ordinary goal reaches the daemon\n{}",
        ordinary.output()
    );

    for predicate in [
        &["/usr/bin/make", "--help"][..],
        &["/usr/bin/sort", "-h"][..],
    ] {
        let run = cli().args(goal_line(&agent, "t", predicate)).run();
        assert_eq!(
            run.code(),
            exit::SUCCESS,
            "{predicate:?}: the call must be sent, not answered with the usage text\n{}",
            run.output()
        );
        assert!(
            !run.stdout.contains("USAGE"),
            "{predicate:?}: the usage text was printed instead of the call being made\n{}",
            run.stdout
        );
        // And the goal that was typed reached the journal, with the help token intact in the
        // operand position — so this is not merely "an exit code changed".
        let program = predicate[0];
        let goal = fixture.await_record("GoalSet", |record| match record {
            Record::GoalSet { goal, .. } => {
                let Predicate::Command { argv, .. } = &goal.predicate else {
                    return None;
                };
                (argv.first().map(String::as_str) == Some(program)).then(|| goal.clone())
            }
            _ => None,
        });
        let Predicate::Command { argv, .. } = &goal.predicate else {
            panic!("a command predicate")
        };
        assert_eq!(
            argv,
            &predicate
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>(),
            "{predicate:?}: the predicate the operator typed, whole"
        );
    }

    running.stop();
}

/// **A help token given as a flag's value is that value**, and the call is still sent.
///
/// The other half of the same property: `--text help` and `hire --name help` are values, not
/// help requests. A value is consumed by the flag that takes it, so neither is ever in a flag
/// position.
#[test]
fn a_help_token_given_as_a_flag_value_still_reaches_the_daemon() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let mut running = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let socket = settings.socket();
    let cli = || Cli::new(&socket, &settings.runtime_dir);
    let agent = hire(&cli(), &fixture);

    let goal = cli()
        .args(goal_line(&agent, "help", &["/usr/bin/true"]))
        .run();
    assert_eq!(
        goal.code(),
        exit::SUCCESS,
        "`--text help` is a goal\n{}",
        goal.output()
    );
    let stored = fixture.await_record("GoalSet", |record| match record {
        Record::GoalSet { goal, .. } if goal.text == "help" => Some(goal.clone()),
        _ => None,
    });
    assert_eq!(
        stored.text, "help",
        "the goal's text is the word that was typed"
    );

    // `hire --name help` is an agent called "help", not a help request.
    let named = cli()
        .arg("--json")
        .args([
            "hire",
            "--name",
            "help",
            "--role",
            "infra",
            "--department",
            "infra",
            "--provider",
            "claude",
            "--cwd",
        ])
        .arg(fixture.state.path())
        .args(["--usd-per-day", "10"])
        .run();
    assert_eq!(
        named.code(),
        exit::SUCCESS,
        "`--name help` is a name and the hire must be sent\n{}",
        named.output()
    );
    let hired_as_help = named.json::<HireResult>().agent;
    assert!(
        fixture.records().iter().any(|record| matches!(
            record,
            Record::AgentHired { id, name, .. } if *id == hired_as_help && name == "help"
        )),
        "the agent named `help` reached the journal, under that name"
    );

    running.stop();
}

/// Hire an agent through the CLI and return its id as text.
fn hire(cli: &Cli, fixture: &Fixture) -> String {
    let hired = cli
        .clone()
        .arg("--json")
        .args(common::hire_args(fixture.state.path()))
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    hired.json::<HireResult>().agent.to_string()
}

/// A `goal` command line for `agent`, with `text` and `predicate`.
fn goal_line(agent: &str, text: &str, predicate: &[&str]) -> Vec<std::ffi::OsString> {
    let mut line = vec![
        "goal",
        "--agent",
        agent,
        "--text",
        text,
        "--usd",
        "5",
        "--hop-cap",
        "8",
        "--check-may-already-pass",
        "--",
    ];
    line.extend_from_slice(predicate);
    line.iter().map(std::ffi::OsString::from).collect()
}

/// A goal whose text *is* a flag of this CLI's own is still text.
///
/// The other way a value can be corrupted: not by a shell, but by the parser in front of it. `--`
/// is what separates them, and this is the assertion that it does.
#[test]
fn a_goal_text_that_looks_like_a_flag_is_still_text() {
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
    let agent = hired.json::<HireResult>().agent;

    let looks_like_a_flag = "--json --usd 999 --hop-cap 0";
    let set = cli()
        .args(["goal", "--agent"])
        .arg(agent.to_string())
        .arg("--text")
        .arg(looks_like_a_flag)
        .args([
            "--usd",
            "5",
            "--hop-cap",
            "8",
            "--check-may-already-pass",
            "--",
            "/usr/bin/true",
        ])
        .run();
    assert_eq!(set.code(), exit::SUCCESS, "{}", set.output());

    let goal = fixture.await_record("GoalSet", |record| match record {
        Record::GoalSet { goal, .. } => Some(goal.clone()),
        _ => None,
    });
    assert_eq!(goal.text, looks_like_a_flag);
    assert_eq!(
        goal.hop_cap, 8,
        "and the flags inside the text were not read as flags"
    );
    assert_eq!(goal.budget.usd, Some(dudes_types::Usd::from_dollars(5)));

    running.stop();
}

#[test]
fn rename_from_the_shipped_cli_records_display_text_without_new_work() {
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
        .args([
            "--json",
            "hire",
            "--name",
            "Original",
            "--role",
            "backend",
            "--department",
            "engineering",
            "--provider",
            "claude",
            "--cwd",
            fixture.state.path().to_str().unwrap(),
            "--usd-per-day",
            "10",
        ])
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let result: HireResult = hired.json();
    let id = result.agent.to_string();
    let before = fixture.records();
    let name = "Display $(literal) with spaces";
    let renamed = cli().args(["--json", "rename", &id, name]).run();
    assert_eq!(renamed.code(), exit::SUCCESS, "{}", renamed.output());
    assert_eq!(renamed.json::<serde_json::Value>(), serde_json::json!({}));
    let after = fixture.records();
    assert_eq!(after.len(), before.len() + 1);
    assert!(
        matches!(after.last(), Some(Record::AgentRenamed { id: agent, name: recorded, browser_rename: None }) if *agent == result.agent && recorded == name)
    );
    running.stop();
}

#[test]
fn look_from_the_shipped_cli_records_only_the_new_appearance() {
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
        .args([
            "--json",
            "hire",
            "--name",
            "Original",
            "--role",
            "backend",
            "--department",
            "engineering",
            "--provider",
            "claude",
            "--cwd",
            fixture.state.path().to_str().unwrap(),
            "--usd-per-day",
            "10",
        ])
        .run();
    assert_eq!(hired.code(), exit::SUCCESS, "{}", hired.output());
    let result: HireResult = hired.json();
    let before = fixture.records();
    let changed = cli()
        .args([
            "--json",
            "look",
            &result.agent.to_string(),
            "--role",
            "Design $(literal)",
            "--character",
            "h",
            "--accent",
            "automatic",
        ])
        .run();
    assert_eq!(changed.code(), exit::SUCCESS, "{}", changed.output());
    assert_eq!(changed.json::<serde_json::Value>(), serde_json::json!({}));
    let after = fixture.records();
    assert_eq!(after.len(), before.len() + 1);
    assert!(matches!(after.last(), Some(Record::AgentLookChanged {
        id, role, character: Some(dudes_types::CastCharacter::H),
        accent: None, browser_look: None,
    }) if *id == result.agent && role == "Design $(literal)"));
    running.stop();
}
