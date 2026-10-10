//! Targeted operator controls and confirmation before local resources are opened.
#![cfg(unix)]

use std::ffi::OsString;
use std::io::{self, BufRead, Cursor};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use dudes::{Environment, exit};
use dudes_proto::Record;
use dudes_proto::rpc::ErrorCode;
use dudes_proto::rpc::agent::HireResult;
use dudes_proto::rpc::{Empty, RpcError};

mod common;
use common::{Fixture, OpenRunner, SharedHost, start};

fn args(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

fn invoke(
    argv: &[OsString],
    env: &Environment,
    input: Option<&mut dyn BufRead>,
) -> (u8, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = dudes::run(argv, env, input, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn environment(socket: &std::path::Path, runtime: &std::path::Path) -> Environment {
    Environment::resolve(|name| match name {
        "DUDES_SOCK" => Some(socket.as_os_str().to_owned()),
        "DUDES_RUNTIME_DIR" => Some(runtime.as_os_str().to_owned()),
        _ => None,
    })
}

fn hire(fixture: &Fixture, env: &Environment) -> String {
    let mut argv = common::hire_args(fixture.state.path());
    argv.insert(0, "--json".into());
    let (code, out, err) = invoke(&argv, env, None);
    assert_eq!(code, exit::SUCCESS, "{err}");
    serde_json::from_str::<HireResult>(&out)
        .unwrap()
        .agent
        .to_string()
}

#[test]
fn halt_and_existing_resume_dispatch_their_durable_controls() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _daemon = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let env = environment(&settings.socket(), &settings.runtime_dir);
    let agent = hire(&fixture, &env);
    let (code, out, err) = invoke(&args(&["--json", "halt", "--agent", &agent]), &env, None);
    assert_eq!(code, exit::SUCCESS, "{err}");
    serde_json::from_str::<Empty>(&out).unwrap();
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|row| matches!(row, Record::Halt { agent: id, .. } if id.to_string() == agent))
            .count(),
        1,
    );
    let (code, out, err) = invoke(&args(&["resume", &agent]), &env, None);
    assert_eq!(code, exit::SUCCESS, "{err}");
    assert_eq!(
        out,
        "Resume recorded. Pause, halt and automatic hold are cleared.\n"
    );
    assert!(
        fixture.records().iter().any(|row| {
            matches!(row, Record::Resume { agent: id, .. } if id.to_string() == agent)
        })
    );
    let (code, out, err) = invoke(&args(&["halt", "--agent", &agent]), &env, None);
    assert_eq!(code, exit::SUCCESS, "{err}");
    assert_eq!(
        out,
        "Halt recorded. Owned child teardown may still be in progress. Use resume to clear the halt.\n"
    );
}

#[test]
fn confirmed_retirement_uses_the_existing_operator_method_and_reason() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _daemon = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let env = environment(&settings.socket(), &settings.runtime_dir);
    let agent = hire(&fixture, &env);
    let mut input = Cursor::new(b"invalid\nYES\n");
    let (code, out, err) = invoke(
        &args(&["retire", "--agent", &agent]),
        &env,
        Some(&mut input),
    );
    assert_eq!(code, exit::SUCCESS, "{err}");
    assert!(out.contains("[y/N]"));
    assert!(out.contains("Answer y or n."));
    assert!(out.ends_with("Retirement recorded. Owned child teardown may still be in progress.\n"));
    assert!(fixture.records().iter().any(|row| {
        matches!(row, Record::AgentRetired { id, reason, .. }
            if id.to_string() == agent && reason == "retired by operator")
    }));
    let before = fixture.records();
    for target in [&agent, "01ARZ3NDEKTSV4RRFFQ69G5FAV"] {
        let (code, _, err) = invoke(&args(&["--json", "halt", "--agent", target]), &env, None);
        assert_eq!(code, exit::OTHER);
        assert_eq!(
            serde_json::from_str::<RpcError>(err.trim()).unwrap().code,
            ErrorCode::NotFound.code(),
        );
    }
    assert_eq!(fixture.records(), before);
}

#[test]
fn explicit_yes_is_required_for_json_and_never_reads_terminal_input() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let _daemon = start(
        &settings,
        SharedHost::new(),
        Arc::new(OpenRunner::default()),
    );
    let env = environment(&settings.socket(), &settings.runtime_dir);
    let agent = hire(&fixture, &env);
    let before = fixture.records();
    let mut input = Cursor::new(b"yes\n");
    let (code, out, err) = invoke(
        &args(&["--json", "retire", "--agent", &agent]),
        &env,
        Some(&mut input),
    );
    assert_eq!(code, exit::OTHER);
    assert!(err.contains("--yes"));
    assert!(out.is_empty());
    assert_eq!(input.position(), 0);
    assert_eq!(fixture.records(), before);
    let (code, out, err) = invoke(
        &args(&["--json", "retire", "--agent", &agent, "--yes"]),
        &env,
        Some(&mut input),
    );
    assert_eq!(code, exit::SUCCESS, "{err}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::json!({})
    );
    assert_eq!(input.position(), 0);
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|row| {
                matches!(row, Record::AgentRetired { id, .. } if id.to_string() == agent)
            })
            .count(),
        1,
    );
}

#[test]
fn no_default_eof_and_noninteractive_refusals_precede_connection_and_credential_read() {
    let owned = tempfile::tempdir().unwrap();
    let runtime = owned.path().join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let credential = runtime.join("operator.cred");
    std::fs::write(&credential, b"malformed-owned-fixture").unwrap();
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).unwrap();
    let socket = runtime.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let env = environment(&socket, &runtime);
    let agent = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    for answer in [b"no\n".as_slice(), b"\n", b""] {
        let mut input = Cursor::new(answer);
        let (code, _, err) = invoke(&args(&["retire", "--agent", agent]), &env, Some(&mut input));
        assert_eq!(code, exit::OTHER);
        assert!(err.contains("retirement canceled"), "{err}");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    let (code, out, err) = invoke(&args(&["retire", "--agent", agent]), &env, None);
    assert_eq!(code, exit::OTHER);
    assert!(out.is_empty());
    assert!(err.contains("--yes"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    let (code, _, err) = invoke(&args(&["retire", "--agent", agent, "--yes"]), &env, None);
    assert_eq!(code, exit::OTHER);
    assert!(err.contains("credential"), "{err}");
    listener.accept().unwrap();
}

#[test]
fn new_control_flags_reject_missing_duplicate_malformed_and_unrelated_operands() {
    let env = Environment::resolve(|_| None);
    let agent = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    for argv in [
        vec!["halt"],
        vec!["halt", "--agent"],
        vec!["halt", "--agent", "not-an-agent"],
        vec!["halt", "--agent", agent, "--agent", agent],
        vec!["halt", "--agent", agent, "--yes"],
        vec!["halt", agent],
        vec!["retire", "--yes"],
        vec!["retire", "--agent", agent, "--yes", "--yes"],
        vec!["retire", "--agent", agent, "--reason", "extra"],
        vec!["retire", "--agent", agent, "--", "extra"],
    ] {
        let (code, out, err) = invoke(&args(&argv), &env, None);
        assert_eq!(code, exit::USAGE, "{argv:?}: {err}");
        assert!(out.is_empty());
        assert!(!err.is_empty());
    }
}
