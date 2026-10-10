//! The predicate environment through the shipped composition: an ambient agent credential, socket and
//! vendor credential reach the real daemon process but not the real predicate child it launches.
//!
//! This is deliberately outside the library seam. `main.rs` must narrow the ambient environment
//! with `allowlisted`, then `dudes-host` must treat that `LaunchSpec.env` as complete by clearing
//! the child's inherited environment. The test crosses both boundaries: it starts the actual
//! `duded` binary with planted secrets, drives it over its socket, and reads `/usr/bin/env` output
//! from the predicate evidence in the actual journal. Removing either narrowing step makes this
//! named test disclose all three planted values.
//!
//! The managed CLI is a fixture executable because no vendor installation belongs on a runner.
//! The predicate is not a fixture answer: the same bare-program-through-an-external-symlink shape
//! as `predicate.rs` resolves to and executes the real `/usr/bin/env` child. Its successful output
//! is the positive spawn control for the in-tree arm's zero-runner-call refusal.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dudes_proto::goal::Evidence;
use dudes_proto::rpc::Method;
use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::{Record, rpc};
use dudes_rpc::Client;
use dudes_types::Provenance;

mod common;

use common::Fixture;

const AGENT_TOKEN: &str = "outer-process-agent-token";
const AGENT_SOCKET: &str = "/planted/outer-process.sock";
const VENDOR_CREDENTIAL: &str = "outer-process-vendor-credential";

/// A real daemon process, killed and reaped even when an assertion unwinds.
struct Daemon(std::process::Child);

impl Daemon {
    /// Start the shipped binary with an exact ambient environment, including the three values a
    /// predicate must never inherit. The fake `claude` and the predicate symlink both live in
    /// `bin`, outside the agent tree the RPC later hires.
    #[allow(
        clippy::disallowed_types,
        reason = "daemon source may not create children; this test harness starts \
                  the daemon binary itself, while host_seam.rs guards src/"
    )]
    fn start(fixture: &Fixture, bin: &Path) -> Self {
        #[cfg(target_os = "macos")]
        let config = fixture
            .runtime
            .path()
            .join("Library/Application Support/dudes/config.toml");
        #[cfg(not(target_os = "macos"))]
        let config = fixture.runtime.path().join(".config/dudes/config.toml");
        std::fs::create_dir_all(config.parent().expect("config parent"))
            .expect("create config directory");
        std::fs::write(config, "[budget]\nper_turn_floor = { usd = 0.000001 }\n")
            .expect("write reserve configuration");
        let path = std::env::join_paths([bin, Path::new("/usr/bin"), Path::new("/bin")])
            .expect("an executable search path");
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_duded"));
        command
            .env_clear()
            .env("PATH", path)
            .env("HOME", fixture.runtime.path())
            .env("TMPDIR", fixture.runtime.path())
            .env("LANG", "C")
            .env("DUDES_STATE_DIR", fixture.state.path())
            .env("DUDES_RUNTIME_DIR", fixture.runtime.path().join("run"))
            .env("DUDES_TOKEN", AGENT_TOKEN)
            .env("DUDES_SOCK", AGENT_SOCKET)
            .env("ANTHROPIC_API_KEY", VENDOR_CREDENTIAL)
            .stdin(std::process::Stdio::null());
        Self(command.spawn().expect("the shipped daemon starts"))
    }

    /// Poll an actual RPC call rather than the socket path, which can exist before the accept loop.
    fn await_ping(&mut self, socket: &Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(mut client) = Client::connect(socket)
                && client.call(Method::SystemPing, None).is_ok()
            {
                return;
            }
            if let Some(status) = self.0.try_wait().expect("wait") {
                panic!("the daemon exited before answering ping: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not answer ping within 20 seconds"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait for a graceful shutdown and fail instead of hanging.
    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.0.try_wait().expect("wait") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not stop within 20 seconds"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let pid = rustix::process::Pid::from_raw(
                i32::try_from(self.0.id()).expect("the daemon pid fits i32"),
            )
            .expect("a nonzero daemon pid");
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            let _ = self.0.wait();
        }
    }
}

/// Read the freshly minted credential the way an operator client does.
fn credential_of(fixture: &Fixture) -> OperatorCredential {
    let text = std::fs::read_to_string(
        fixture
            .runtime
            .path()
            .join("run")
            .join(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
    )
    .expect("the daemon minted an operator credential");
    OperatorCredential::new(text).expect("the credential is well formed")
}

/// Install the two external executables used by the outer process.
fn install_executables(bin: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let fake_cli = bin.join("claude");
    std::fs::write(&fake_cli, "#!/bin/sh\nexit 0\n").expect("write the fake managed CLI");
    std::fs::set_permissions(&fake_cli, std::fs::Permissions::from_mode(0o700))
        .expect("make the fake managed CLI executable");

    std::os::unix::fs::symlink("/usr/bin/env", bin.join("predicate-env"))
        .expect("install the external predicate symlink");
}

type RpcClient =
    Client<std::io::BufReader<std::os::unix::net::UnixStream>, std::os::unix::net::UnixStream>;

/// Hire the real process's fake managed agent and give it the external command predicate.
fn set_predicate_goal(client: &mut RpcClient, fixture: &Fixture, agent_tree: &Path) {
    let hire = client
        .call_as_operator(
            Method::AgentHire,
            Some(
                serde_json::to_value(common::hire_params(agent_tree))
                    .expect("hire params serialize"),
            ),
            credential_of(fixture),
        )
        .expect("agent.hire is answered");
    assert!(hire.error.is_none(), "agent.hire: {:?}", hire.error);
    let agent: rpc::agent::HireResult =
        serde_json::from_value(hire.result.expect("a hire result")).expect("decode hire result");

    let goal = client
        .call_as_operator(
            Method::GoalSet,
            Some(
                serde_json::to_value(common::goal_params(agent.agent, &["predicate-env"]))
                    .expect("goal params serialize"),
            ),
            credential_of(fixture),
        )
        .expect("goal.set is answered");
    assert!(goal.error.is_none(), "goal.set: {:?}", goal.error);
}

/// Assert the journal proves the external executable ran with the complete, fixed environment.
fn assert_real_scrubbed_evidence(fixture: &Fixture, bin: &Path) {
    let (pass, provenance, evidence) = fixture.await_record("PredicateResult", |record| {
        if let Record::PredicateResult {
            pass,
            provenance,
            evidence,
            ..
        } = record
        {
            Some((*pass, *provenance, evidence.clone()))
        } else {
            None
        }
    });
    assert!(pass, "the real external predicate child passed");
    assert_eq!(provenance, Provenance::Observed);

    let Evidence::Command {
        argv,
        program,
        output_tail,
        ..
    } = evidence
    else {
        panic!("a command predicate produces command evidence")
    };
    assert_eq!(argv, vec!["predicate-env"]);
    assert_eq!(
        program.map(PathBuf::from),
        Some(std::fs::canonicalize("/usr/bin/env").expect("canonical /usr/bin/env")),
        "the out-of-tree symlink resolved to the real executable that was spawned"
    );

    let expected_path = std::env::join_paths([bin, Path::new("/usr/bin"), Path::new("/bin")])
        .expect("the expected PATH");
    assert!(
        output_tail.lines().any(
            |line| line.strip_prefix("PATH=") == Some(expected_path.to_string_lossy().as_ref())
        ),
        "control: the real child received the allowlisted PATH: {output_tail:?}"
    );
    for barred in ["DUDES_TOKEN", "DUDES_SOCK", "ANTHROPIC_API_KEY"] {
        assert!(
            !output_tail
                .lines()
                .any(|line| line.split_once('=').is_some_and(|(name, _)| name == barred)),
            "{barred} reached the real predicate child: {output_tail:?}"
        );
    }
    assert!(
        !output_tail.contains(AGENT_TOKEN)
            && !output_tail.contains(AGENT_SOCKET)
            && !output_tail.contains(VENDOR_CREDENTIAL),
        "none of the planted values may reach the real predicate child: {output_tail:?}"
    );
}

/// The production environment contract, across the actual daemon and actual predicate child.
///
/// Load-bearing mutants, run in isolation and reverted: bypassing `allowlisted` in `main.rs` and
/// deleting `dudes_host::command`'s `env_clear()` each expose the planted variables below and fail
/// this test. `PATH` is the positive control that the child received the fixed allowlist, while
/// `program` and the output prove the external symlink resolved and spawned successfully.
#[test]
fn shipped_predicate_child_inherits_only_the_exact_allowlist() {
    let fixture = Fixture::new();
    let bin = tempfile::tempdir().expect("external executable directory");
    let agent_tree = tempfile::tempdir().expect("agent tree");
    install_executables(bin.path());

    let socket = fixture.runtime.path().join("run").join("duded.sock");
    let mut daemon = Daemon::start(&fixture, bin.path());
    daemon.await_ping(&socket);

    let mut client = Client::connect(&socket).expect("connect to the shipped daemon");
    set_predicate_goal(&mut client, &fixture, agent_tree.path());
    assert_real_scrubbed_evidence(&fixture, bin.path());

    let stopped = client
        .call_as_operator(Method::SystemShutdown, None, credential_of(&fixture))
        .expect("system.shutdown is answered");
    assert!(
        stopped.error.is_none(),
        "system.shutdown: {:?}",
        stopped.error
    );
    assert!(
        daemon.wait().success(),
        "the shipped daemon stopped cleanly"
    );
}
