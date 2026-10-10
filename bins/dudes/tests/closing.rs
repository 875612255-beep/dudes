//! Closing CLI protocol tests.
#![cfg(unix)]

mod common;

use common::Cli;
use dudes::exit;
use dudes_proto::rpc::closing::{ClosingReport, ClosingReportRef, ClosingStarted};
use dudes_proto::rpc::system::ShutdownReceipt;
use dudes_types::{AgentId, Lsn, Timestamp};
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

fn answer(dir: &Path, result: Value) -> (PathBuf, std::thread::JoinHandle<Value>) {
    let socket = dir.join("duded.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
        writeln!(&stream, "{response}").unwrap();
        request
    });
    (socket, thread)
}

#[test]
fn default_stop_announces_and_preserves_the_typed_final_census() {
    let dir = tempfile::tempdir().unwrap();
    let report = ClosingReport {
        run: "this-run".into(),
        announcement: Lsn(7),
        complete: Lsn(9),
        participants: vec![AgentId::NIL],
        acknowledged: vec![],
        missing: vec![AgentId::NIL],
        settled: vec![],
    };
    let started = ClosingStarted {
        announcement: Lsn(7),
        deadline: Timestamp::from_unix_millis(0),
        participants: report.participants.clone(),
        completion: ShutdownReceipt {
            path: dir.path().join("completion"),
            run: "this-run".into(),
        },
        report: ClosingReportRef {
            path: dir.path().join("report"),
            run: "this-run".into(),
        },
    };
    std::fs::write(&started.completion.path, "complete this-run\n").unwrap();
    std::fs::write(&started.report.path, serde_json::to_vec(&report).unwrap()).unwrap();
    let (socket, server) = answer(dir.path(), serde_json::to_value(started).unwrap());
    let run = Cli::new(&socket, dir.path()).args(["--json", "stop"]).run();
    let request = server.join().unwrap();
    assert_eq!(request["method"], "system.closingTime");
    assert_eq!(request["params"], json!({"action":"announce"}));
    assert_eq!(run.code(), exit::SUCCESS, "{}", run.output());
    assert_eq!(run.json::<ClosingReport>(), report);
    assert!(run.stderr.contains("without acknowledgements"));
    assert!(run.stderr.contains("00000000000000000000000000"));
    assert!(!run.output().contains("saved"));
}

#[test]
fn own_ack_uses_only_the_inherited_token_and_never_reads_operator_material() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
        "invalid",
    )
    .unwrap();
    let token = "T".repeat(43);
    let (socket, server) = answer(dir.path(), json!({}));
    let run = Cli::new(&socket, dir.path())
        .env("DUDES_TOKEN", &token)
        .args(["--json", "closing", "ack", "--announcement-lsn", "7"])
        .run();
    let request = server.join().unwrap();
    assert_eq!(request["method"], "system.closingTime");
    assert_eq!(request["params"]["action"], "ack");
    assert_eq!(request["params"]["lsn"], 7);
    assert_eq!(request["params"]["token"], token);
    assert!(request.get("cred").is_none());
    assert!(request["params"].get("agent").is_none());
    assert_eq!(run.code(), exit::SUCCESS, "{}", run.output());
    assert_eq!(
        run.json::<dudes_proto::rpc::Empty>(),
        dudes_proto::rpc::Empty {}
    );
    assert!(!run.output().contains(&token));
}

#[test]
fn default_stop_fails_when_the_final_report_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let started = ClosingStarted {
        announcement: Lsn(7),
        deadline: Timestamp::from_unix_millis(0),
        participants: vec![],
        completion: ShutdownReceipt {
            path: dir.path().join("completion"),
            run: "this-run".into(),
        },
        report: ClosingReportRef {
            path: dir.path().join("report"),
            run: "this-run".into(),
        },
    };
    std::fs::write(&started.completion.path, "complete this-run\n").unwrap();
    let (socket, server) = answer(dir.path(), serde_json::to_value(started).unwrap());
    let run = Cli::new(&socket, dir.path()).arg("stop").run();
    server.join().unwrap();
    assert_eq!(run.code(), exit::OTHER, "{}", run.output());
    assert!(run.stdout.is_empty());
    assert!(run.stderr.contains("closing report"));
}

#[test]
fn own_ack_requires_the_exact_empty_result() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, server) = answer(dir.path(), json!({"extra":"not an acknowledgement"}));
    let run = Cli::new(&socket, dir.path())
        .env("DUDES_TOKEN", "T".repeat(43))
        .args(["--json", "closing", "ack", "--announcement-lsn", "7"])
        .run();
    server.join().unwrap();
    assert_eq!(run.code(), exit::OTHER, "{}", run.output());
    assert!(run.stdout.is_empty());
}

#[test]
fn missing_ack_token_is_sent_without_operator_fallback_and_refusal_is_retained() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
        "invalid",
    )
    .unwrap();
    let socket = dir.path().join("duded.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(request["params"], json!({"action":"ack","lsn":7}));
        assert!(request.get("cred").is_none());
        writeln!(&stream, "{}", json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32001,"message":"not admitted"}})).unwrap();
    });
    let run = Cli::new(&socket, dir.path())
        .args(["--json", "closing", "ack", "--announcement-lsn", "7"])
        .run();
    server.join().unwrap();
    assert_eq!(run.code(), exit::REFUSED, "{}", run.output());
    assert!(run.stdout.is_empty());
    assert_eq!(
        serde_json::from_str::<dudes_proto::rpc::RpcError>(&run.stderr)
            .unwrap()
            .code,
        -32001
    );
}

fn stop_with(
    settled: Vec<dudes_proto::rpc::closing::ClosingSettled>,
    missing: Vec<AgentId>,
) -> common::Run {
    let dir = tempfile::tempdir().unwrap();
    let report = ClosingReport {
        run: "this-run".into(),
        announcement: Lsn(7),
        complete: Lsn(9),
        participants: missing.clone(),
        acknowledged: vec![],
        missing,
        settled,
    };
    let started = ClosingStarted {
        announcement: Lsn(7),
        deadline: Timestamp::from_unix_millis(0),
        participants: report.participants.clone(),
        completion: ShutdownReceipt {
            path: dir.path().join("completion"),
            run: "this-run".into(),
        },
        report: ClosingReportRef {
            path: dir.path().join("report"),
            run: "this-run".into(),
        },
    };
    std::fs::write(&started.completion.path, "complete this-run\n").unwrap();
    std::fs::write(&started.report.path, serde_json::to_vec(&report).unwrap()).unwrap();
    let (socket, server) = answer(dir.path(), serde_json::to_value(started).unwrap());
    let run = Cli::new(&socket, dir.path()).arg("stop").run();
    server.join().unwrap();
    run
}

#[test]
fn a_settled_participant_is_named_with_its_reason_and_raises_no_work_warning() {
    use dudes_proto::rpc::closing::{ClosingSettled, ClosingSettledReason};
    let agent = AgentId::NIL;
    let run = stop_with(
        vec![ClosingSettled {
            agent,
            reason: ClosingSettledReason::GoalDone,
        }],
        vec![agent],
    );
    assert_eq!(run.code(), exit::SUCCESS, "{}", run.output());
    assert!(run.stdout.contains("closing time acknowledgements: 0 of 1"));
    assert!(run.stdout.contains(&format!(
        "closing time settled without acknowledgement: {agent} (goal done)"
    )));
    assert!(!run.stderr.contains("without acknowledgements"));
}

#[test]
fn an_unsettled_missing_participant_still_raises_the_work_warning() {
    use dudes_proto::rpc::closing::{ClosingSettled, ClosingSettledReason};
    let settled = AgentId::NIL;
    let owed = AgentId::from_parts(0, 5);
    let run = stop_with(
        vec![ClosingSettled {
            agent: settled,
            reason: ClosingSettledReason::Retired,
        }],
        vec![settled, owed],
    );
    assert_eq!(run.code(), exit::SUCCESS, "{}", run.output());
    assert!(run.stderr.contains(&format!("from: {owed}")));
    assert!(!run.stderr.contains(&settled.to_string()));
    assert!(run.stdout.contains("(retired)"));
}

#[test]
fn a_settled_entry_outside_the_missing_set_is_an_invalid_report() {
    use dudes_proto::rpc::closing::{ClosingSettled, ClosingSettledReason};
    let agent = AgentId::NIL;
    let dir = tempfile::tempdir().unwrap();
    let report = ClosingReport {
        run: "this-run".into(),
        announcement: Lsn(7),
        complete: Lsn(9),
        participants: vec![agent],
        acknowledged: vec![agent],
        missing: vec![],
        settled: vec![ClosingSettled {
            agent,
            reason: ClosingSettledReason::GoalDone,
        }],
    };
    let started = ClosingStarted {
        announcement: Lsn(7),
        deadline: Timestamp::from_unix_millis(0),
        participants: vec![agent],
        completion: ShutdownReceipt {
            path: dir.path().join("completion"),
            run: "this-run".into(),
        },
        report: ClosingReportRef {
            path: dir.path().join("report"),
            run: "this-run".into(),
        },
    };
    std::fs::write(&started.completion.path, "complete this-run\n").unwrap();
    std::fs::write(&started.report.path, serde_json::to_vec(&report).unwrap()).unwrap();
    let (socket, server) = answer(dir.path(), serde_json::to_value(started).unwrap());
    let run = Cli::new(&socket, dir.path()).arg("stop").run();
    server.join().unwrap();
    assert_eq!(run.code(), exit::OTHER, "{}", run.output());
}
