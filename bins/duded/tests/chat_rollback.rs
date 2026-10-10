//! Refusal and legacy controls against the pinned pre-framing daemon and library.
#![cfg(unix)]
#![allow(missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dudes_host::{ChildExit, ChildStream, ChildStreams, OneShot, OneShotOutcome, RealHost};
use dudes_providers::adapter::ManagedInstall;
use dudes_providers::launch::{EnvVar, LaunchSpec};
use dudes_supervisor::host::{AdoptionKey, KillOutcome, ProcessHost as _, SpawnResult};

use dudes_journal::frame::{Header, crc32c};
use dudes_journal::segment::segment_path;
use dudes_journal::{JournalOptions, Projections, Reader, open, replay};
use dudes_proto::Entry;
use dudes_types::{AgentId, GoalId, Lsn, Timestamp, TurnId};
use serde_json::{Value, json};

const PINNED_SHA: &str = "0d3db71bf29f5a9993e494f7b2724cbad0f6d089";

fn agent() -> AgentId {
    AgentId::from_parts(1, 1)
}
fn goal() -> GoalId {
    GoalId::from_parts(2, 1)
}
fn turn() -> TurnId {
    TurnId::from_parts(3, 1)
}

#[derive(Clone, Copy)]
enum History {
    Chat,
    Compatible,
    OwedCompatible,
}

struct Fixture {
    state: tempfile::TempDir,
    home: tempfile::TempDir,
    runtime: tempfile::TempDir,
    barrier: Option<Lsn>,
}

fn old_program(variable: &str) -> PathBuf {
    let program = std::env::var_os(variable)
        .expect("prebuild the pinned old binaries before running ignored rollback tests");
    let path = PathBuf::from(program);
    assert!(path.is_file(), "pinned binary is absent");
    path
}

fn entry(value: Value) -> Entry {
    Entry {
        at: Timestamp::UNIX_EPOCH,
        record: serde_json::from_value(value).unwrap(),
    }
}

impl Fixture {
    fn new(history: History, advanced: bool, damaged: bool) -> Self {
        let chat = matches!(history, History::Chat);
        let legacy_issue = matches!(history, History::OwedCompatible);
        let state = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let path = state.path().join("journal");
        let (mut writer, _) = open(
            &path,
            JournalOptions {
                checkpoint_every: 0,
                ..JournalOptions::default()
            },
        )
        .unwrap();
        writer.append(&entry(json!({"t":"agent_hired","id":agent(),"name":"Owned old-start control","role":"test","department":"test","provider":"claude","cwd":state.path(),"budget":{}}))).unwrap();
        writer.append(&entry(json!({"t":"goal_set","agent":agent(),"goal":{"id":goal(),"text":"Owned work","predicate":{"command":{"argv":["/usr/bin/false"],"timeout_s":1}},"budget":{},"hop_cap":50}}))).unwrap();
        let barrier = if chat {
            Some(writer.append(&entry(json!({"t":"browser_note_queued","agent":agent(),"text":"owned note","destination":{"kind":"managed_goal","goal":goal()},"browser":{"request_id_digest":"11".repeat(32),"request_digest":"22".repeat(32),"session_digest":"33".repeat(32)}}))).unwrap())
        } else {
            None
        };
        if advanced || legacy_issue {
            let decision = writer.append(&entry(json!({"t":"loop_decided","agent":agent(),"input_digest":"33".repeat(32),"decision":{"kind":"continue","task":"owned work"},"gate_trace":[]}))).unwrap();
            let mut issue = json!({"t":if chat{"turn_issued_with_browser_note"}else{"turn_issued"},"agent":agent(),"turn":turn(),"goal":goal(),"task":"owned work","session":null,"mode":"managed","model_selection":{},"lsn_of_decision":decision,"answers":[]});
            if let Some(note_lsn) = barrier {
                issue["note_lsn"] = json!(note_lsn);
            }
            writer.append(&entry(issue)).unwrap();
            if advanced {
                if let Some(note_lsn) = barrier {
                    writer.append(&entry(json!({"t":"browser_note_dispatch_intent","agent":agent(),"note_lsn":note_lsn,"turn":turn()}))).unwrap();
                }
                writer.append(&entry(json!({"t":"turn_spawned","agent":agent(),"turn":turn(),"pid":4242,"start_time":1,"session":null,"reissued":false}))).unwrap();
                writer.append(&entry(json!({"t":"turn_ended","agent":agent(),"turn":turn(),"mode":"managed","provenance":"observed","exit":{"code":0},"outcome":{"kind":"completed"},"last_message":"owned final","usage":{"input":10,"output":5,"usd":7}}))).unwrap();
            }
        }
        writer
            .append(&entry(json!({"t":"resume","agent":agent()})))
            .unwrap();
        drop(writer);
        let mut store = Projections::open(&state.path().join("state.redb")).unwrap();
        if advanced {
            let (reader, _) = Reader::open(&path).unwrap();
            replay(&reader, &mut store).unwrap();
            assert!(store.last_applied_lsn() > barrier.expect("advanced Chat barrier"));
            assert_eq!(store.spend_by_goal(goal()).unwrap().tokens.count(), 15);
        } else {
            assert_eq!(store.last_applied_lsn(), Lsn::ZERO);
        }
        drop(store);
        if damaged {
            let path = segment_path(&path, 1);
            let mut bytes = fs::read(&path).unwrap();
            bytes[..4].copy_from_slice(&16_777_216u32.to_le_bytes());
            fs::write(path, bytes).unwrap();
        }
        Self {
            state,
            home,
            runtime,
            barrier,
        }
    }

    fn spec(&self, program: &Path) -> LaunchSpec {
        LaunchSpec {
            argv: vec![fs::canonicalize(program).unwrap().into_os_string()],
            env: vec![
                EnvVar::new("HOME", self.home.path()),
                EnvVar::new("DUDES_STATE_DIR", self.state.path()),
                EnvVar::new("DUDES_RUNTIME_DIR", self.runtime.path().join("run")),
                EnvVar::new("PATH", "/usr/bin:/bin"),
                EnvVar::new(
                    "DUDES_STUB_PROVIDER_COMMAND",
                    old_program("DUDES_TEST_OLD_JOURNAL_PROBE"),
                ),
            ],
            cwd: self.state.path().to_path_buf(),
            stdin: dudes_providers::launch::LaunchInput::Null,
        }
    }

    fn bytes(&self) -> (Vec<(PathBuf, Vec<u8>)>, Vec<u8>) {
        let mut logs: Vec<(PathBuf, Vec<u8>)> = fs::read_dir(self.state.path().join("journal"))
            .unwrap()
            .map(|file| {
                let file = file.unwrap().path();
                (file.file_name().unwrap().into(), fs::read(&file).unwrap())
            })
            .collect();
        logs.sort_by(|a, b| a.0.cmp(&b.0));
        (
            logs,
            fs::read(self.state.path().join("state.redb")).unwrap(),
        )
    }
}

fn run_short(spec: &LaunchSpec) -> (i32, String) {
    let runner = OneShot::with_grace(Duration::from_millis(300));
    let outcome = runner.run(spec, Duration::from_secs(5)).unwrap();
    assert_eq!(runner.in_flight(), 0);
    match outcome {
        OneShotOutcome::Finished {
            exit: Some(code),
            output_tail,
            timed_out: false,
        } => (code, output_tail),
        other => panic!("pinned process did not finish normally: {other:?}"),
    }
}

#[derive(Clone)]
struct Exits(mpsc::Sender<ChildExit>);

impl ChildStreams for Exits {
    fn watch(&self, _key: Option<&AdoptionKey>) -> Box<dyn ChildStream> {
        Box::new(self.clone())
    }
}

impl ChildStream for Exits {
    fn line(&mut self, _text: &str) {}
    fn ended(self: Box<Self>, exit: ChildExit) {
        let _ = self.0.send(exit);
    }
}

struct OwnedDaemon {
    host: RealHost,
    key: Option<AdoptionKey>,
    exited: mpsc::Receiver<ChildExit>,
}

impl OwnedDaemon {
    fn start(spec: &LaunchSpec) -> Self {
        let (tx, exited) = mpsc::channel();
        let mut daemon = Self {
            host: RealHost::watching_with_grace(Arc::new(Exits(tx)), Duration::from_secs(5)),
            key: None,
            exited,
        };
        match daemon.host.spawn(spec, &ManagedInstall::default()) {
            SpawnResult::Spawned(key) => daemon.key = Some(key),
            other => panic!("pinned daemon did not launch: {other:?}"),
        }
        daemon
    }

    fn stop(&mut self) -> ChildExit {
        let key = self.key.take().unwrap();
        let outcome = self.host.kill(&key);
        assert!(matches!(outcome, KillOutcome::Signalled), "{outcome:?}");
        let exit = self
            .exited
            .recv_timeout(Duration::from_secs(5))
            .expect("pinned daemon was reaped");
        assert_eq!(self.host.owned(), 0);
        exit
    }
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let _ = self.host.kill(&key);
        }
        let _ = self.host.shutdown();
    }
}

#[test]
#[ignore = "requires the pinned pre-framing 0.49.0 daemon and library probe"]
fn pinned_old_start_and_open_replay_refuse_before_log_or_cache_mutation() {
    assert_eq!(PINNED_SHA.len(), 40);
    let daemon = old_program("DUDES_TEST_OLD_DUDED");
    let probe = old_program("DUDES_TEST_OLD_JOURNAL_PROBE");
    for damaged in [false, true] {
        for advanced in [false, true] {
            let fixture = Fixture::new(History::Chat, advanced, damaged);
            let before = fixture.bytes();
            for mode in ["daemon", "writer-open", "offline-replay"] {
                let mut spec = fixture.spec(if mode == "daemon" { &daemon } else { &probe });
                if mode != "daemon" {
                    spec.argv.push(mode.into());
                    spec.argv.push(fixture.state.path().as_os_str().to_owned());
                }
                let (code, reason) = run_short(&spec);
                assert_ne!(code, 0);
                if damaged {
                    assert!(
                        reason
                            .contains("journal corrupt at 1:0: short frame: 16777228 bytes needed"),
                        "{reason}"
                    );
                } else {
                    let barrier = fixture.barrier.unwrap();
                    assert!(
                        reason.contains(&format!("journal version skew at {barrier}")),
                        "{reason}"
                    );
                    assert!(
                        reason.contains("frame version 2, this build reads 1"),
                        "{reason}"
                    );
                }
                assert_eq!(
                    fixture.bytes(),
                    before,
                    "{mode}, advanced={advanced}, damaged={damaged}"
                );
                assert!(
                    !fixture
                        .state
                        .path()
                        .join("rollback-engine-launched")
                        .exists()
                );
                let run = fixture.runtime.path().join("run");
                assert!(!run.join("duded.sock").exists());
                assert!(!run.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME).exists());
            }
            let barrier = fixture.barrier.unwrap();
            assert_eq!(barrier.offset(), 0);
            let bytes = fs::read(segment_path(
                &fixture.state.path().join("journal"),
                barrier.segment(),
            ))
            .unwrap();
            assert_eq!(Header::parse(bytes[..12].try_into().unwrap()).ver, 2);
        }
    }
}

fn raw(version: u16, kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Header {
        len: u32::try_from(payload.len()).unwrap(),
        ver: version,
        kind,
        crc: crc32c(version, kind, payload),
    }
    .to_bytes()
    .to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
#[ignore = "requires the pinned pre-framing 0.49.0 library probe"]
fn pinned_old_v1_skip_and_unsafe_same_segment_repair_are_controls() {
    let probe = old_program("DUDES_TEST_OLD_JOURNAL_PROBE");
    let fixture = Fixture::new(History::Compatible, false, false);
    let journal = fixture.state.path().join("journal");
    for path in fs::read_dir(&journal).unwrap() {
        fs::remove_file(path.unwrap().path()).unwrap();
    }
    let pause = entry(json!({"t":"resume","agent":agent()}));
    let payload = serde_json::to_vec(&pause).unwrap();
    let mut bytes = raw(1, 1_000, b"unknown compatible non-JSON payload");
    bytes.extend(raw(1, pause.record.kind().tag(), &payload));
    fs::write(segment_path(&journal, 1), &bytes).unwrap();
    let mut spec = fixture.spec(&probe);
    spec.argv.push("offline-replay".into());
    spec.argv.push(fixture.state.path().as_os_str().to_owned());
    let (code, reason) = run_short(&spec);
    assert_eq!(code, 0, "{reason}");
    assert_eq!(fs::read(segment_path(&journal, 1)).unwrap(), bytes);

    let mut hidden = raw(1, pause.record.kind().tag(), &payload);
    hidden[..4].copy_from_slice(&16_777_216u32.to_le_bytes());
    hidden.extend(raw(2, 46, &payload));
    fs::write(segment_path(&journal, 1), &hidden).unwrap();
    let mut spec = fixture.spec(&probe);
    spec.argv.push("reader-open".into());
    spec.argv.push(fixture.state.path().as_os_str().to_owned());
    let (code, reason) = run_short(&spec);
    assert_eq!(code, 0, "{reason}");
    assert_eq!(fs::read(segment_path(&journal, 1)).unwrap(), hidden);
    assert!(Reader::open(&journal).is_err());
    assert_eq!(fs::read(segment_path(&journal, 1)).unwrap(), hidden);
    let mut spec = fixture.spec(&probe);
    spec.argv.push("writer-open".into());
    spec.argv.push(fixture.state.path().as_os_str().to_owned());
    let (code, reason) = run_short(&spec);
    assert_eq!(code, 0, "{reason}");
    assert!(fs::read(segment_path(&journal, 1)).unwrap().is_empty());
}

#[test]
#[ignore = "requires the pinned pre-framing 0.49.0 daemon and owned stand-in"]
fn pinned_old_v1_daemon_reaches_ping_and_owned_dispatch() {
    let daemon = old_program("DUDES_TEST_OLD_DUDED");
    for owed_issue in [false, true] {
        let fixture = Fixture::new(
            if owed_issue {
                History::OwedCompatible
            } else {
                History::Compatible
            },
            false,
            false,
        );
        let mut child = OwnedDaemon::start(&fixture.spec(&daemon));
        let socket = fixture.runtime.path().join("run/duded.sock");
        let limit = Instant::now() + Duration::from_secs(10);
        loop {
            let ping = dudes_rpc::Client::connect(&socket)
                .ok()
                .and_then(|mut client| client.call(dudes_proto::rpc::Method::SystemPing, None).ok())
                .and_then(|reply| reply.result);
            if let Some(ping) = ping {
                assert_eq!(ping["version"], "0.49.0");
                if !owed_issue
                    || fixture
                        .state
                        .path()
                        .join("rollback-engine-launched")
                        .exists()
                {
                    break;
                }
            }
            assert!(
                child.exited.try_recv().is_err(),
                "old control exited before ping"
            );
            assert!(
                Instant::now() < limit,
                "old control did not reach owned boundary"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let exit = child.stop();
        assert_eq!(exit.code, Some(0));
        assert_eq!(exit.signal, None);
    }
}
