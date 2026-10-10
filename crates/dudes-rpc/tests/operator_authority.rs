//! The independent operator-effect authority source over the real accepted-connection path.
//!
//! These tests keep peer classification in front of the conditioned router: every call crosses a
//! real Unix socket and the real `PeerClassifier`. The injected pieces are the independently
//! resolved provenance column, the run-local acquisition epoch, and one test-only conditioned
//! probe. The probe's persistence adapter appends through the real journal with full fsync before
//! its sink increments a counter, then reopens the journal for readback. The positive arm therefore
//! proves both authority plumbing and persist-before-effect without adding a production sink.
#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dudes_journal::{Fsync, JournalOptions, Reader, Writer, open};
use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::memory::{MemoryAppendParams, MemoryAppendResult};
use dudes_proto::rpc::{AgentText, ErrorCode, Method, RefusalGate, RpcError};
use dudes_proto::{ConditionedMethod, ConditionedTarget, Entry, Record};
use dudes_rpc::auth::{PeerClassifier, PeerProvenance, PeerRole};
use dudes_rpc::methods::{ConditionedMemoryControl, register_memory_effect};
use dudes_rpc::testing::{FakeMintedTokens, FakeResolver, TestServer};
use dudes_rpc::{
    Client, DecodedEffect, Dispatcher, OperatorAuthoritySource, PersistenceOperation, Requirement,
};
use dudes_types::{AgentId, Lsn, Timestamp};
use serde_json::{Value, json};

const CREDENTIAL: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const AGENT_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn agent() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ULID")
}

fn credential() -> OperatorCredential {
    OperatorCredential::new(CREDENTIAL).expect("the credential has the contract length")
}

struct Probe {
    journal_dir: std::path::PathBuf,
    writer: Mutex<Option<Writer>>,
    order: Mutex<Vec<&'static str>>,
    effects: AtomicUsize,
}

impl Probe {
    fn new(journal_dir: impl Into<std::path::PathBuf>) -> Arc<Self> {
        let journal_dir = journal_dir.into();
        let (writer, report) = open(
            &journal_dir,
            JournalOptions {
                fsync: Fsync::Full,
                ..JournalOptions::default()
            },
        )
        .expect("open the real test journal");
        assert_eq!(report.frames, 0, "the test journal starts empty");
        Arc::new(Self {
            journal_dir,
            writer: Mutex::new(Some(writer)),
            order: Mutex::new(Vec::new()),
            effects: AtomicUsize::new(0),
        })
    }

    fn journal_records(&self) -> Vec<Record> {
        drop(self.writer.lock().expect("the writer lock").take());
        let (reader, report) = Reader::open(&self.journal_dir).expect("reopen the real journal");
        assert!(report.torn.is_none(), "every acknowledged frame is intact");
        reader
            .iter_from(Lsn::ZERO)
            .expect("read the real journal")
            .map(|entry| entry.expect("an intact journal entry").1.record)
            .collect()
    }
}

impl ConditionedMemoryControl for Probe {
    fn append(
        &self,
        operation: PersistenceOperation<'_, MemoryAppendParams>,
    ) -> Result<Lsn, RpcError> {
        let record = if let Some(record) = operation.disposition_record() {
            record.clone()
        } else {
            let applied = operation
                .applied_effect()
                .expect("a persistence operation has exactly one arm");
            let ConditionedTarget::Agent { agent } = applied.target() else {
                panic!("memory has an agent target")
            };
            Record::MemoryAppend {
                agent: *agent,
                text: std::str::from_utf8(applied.payload().expect("memory payload"))
                    .expect("decoded memory is UTF-8")
                    .to_owned(),
                body: None,
                provenance: Some(applied.provenance()),
            }
        };
        let lsn = self
            .writer
            .lock()
            .expect("the real writer lock")
            .as_mut()
            .expect("readback follows the final call")
            .append(&Entry {
                at: Timestamp::from_unix_millis(1_788_434_400_000),
                record,
            })
            .map_err(|error| persistence_error(&error))?;
        self.order.lock().expect("the probe lock").push("persist");
        Ok(lsn)
    }
}

fn invalid_params(error: &impl ToString) -> RpcError {
    RpcError {
        code: ErrorCode::InvalidParams.code(),
        message: error.to_string(),
        data: None,
    }
}

fn persistence_error(error: &impl ToString) -> RpcError {
    RpcError {
        code: ErrorCode::InternalError.code(),
        message: error.to_string(),
        data: None,
    }
}

fn dispatcher(probe: &Arc<Probe>) -> Dispatcher {
    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        credential(),
        Arc::new(FakeMintedTokens::minted(&[(AGENT_TOKEN, agent())])),
    ));
    let for_persistence = Arc::clone(probe);
    let for_sink = Arc::clone(probe);
    dispatcher.register_conditioned(
        ConditionedMethod::AgentAnswer,
        Requirement::Operator,
        move |params, _role: &PeerRole| {
            let params: AgentText = serde_json::from_value(params.cloned().unwrap_or(Value::Null))
                .map_err(|error| invalid_params(&error))?;
            let payload = params.text.as_bytes().to_vec();
            Ok(DecodedEffect::new(
                params.clone(),
                ConditionedTarget::Agent {
                    agent: params.agent,
                },
                Some(payload),
            ))
        },
        move |operation| {
            let record = if let Some(record) = operation.disposition_record() {
                record.clone()
            } else {
                let applied = operation
                    .applied_effect()
                    .expect("a persistence operation has exactly one arm");
                Record::HumanAnswer {
                    agent: applied.params().agent,
                    text: applied.params().text.clone(),
                    provenance: Some(applied.provenance()),
                    browser_answer: None,
                }
            };
            for_persistence
                .writer
                .lock()
                .expect("the real writer lock")
                .as_mut()
                .expect("readback follows the final call")
                .append(&Entry {
                    at: Timestamp::from_unix_millis(1_788_434_400_000),
                    record,
                })
                .map_err(|error| persistence_error(&error))?;
            for_persistence
                .order
                .lock()
                .expect("the probe lock")
                .push("persist");
            Ok(())
        },
        move |_, ()| {
            for_sink.order.lock().expect("the probe lock").push("sink");
            for_sink.effects.fetch_add(1, Ordering::SeqCst);
            Ok(json!({ "applied": true }))
        },
    );
    register_memory_effect(&mut dispatcher, probe);
    dispatcher
}

fn answer() -> Value {
    json!({ "agent": agent(), "text": "ship it" })
}

fn error_code(response: &dudes_proto::rpc::Response) -> i32 {
    response.error.as_ref().expect("an error response").code
}

fn await_live_leases(source: &OperatorAuthoritySource, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while source.live_leases() != expected {
        assert!(
            Instant::now() < deadline,
            "live authority leases did not reach {expected}: {}",
            source.live_leases()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn assert_no_lease_while_connected<R, W>(
    source: &OperatorAuthoritySource,
    _client: &Client<R, W>,
    presentation: &str,
) {
    assert_eq!(
        source.live_leases(),
        0,
        "the live {presentation} connection acquired authority"
    );
}

#[test]
fn an_open_source_applies_once_after_persisting_observed_provenance() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let source = Arc::new(OperatorAuthoritySource::from_replayed_agents(false));
    let probe = Probe::new(dir.path().join("journal"));
    let server = TestServer::start_with_authority(
        dir.path().join("run"),
        dispatcher(&probe),
        Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
        Arc::clone(&source),
    );
    let mut client = server.client();
    let response = client
        .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
        .expect("the real connection answered");

    assert_eq!(response.result, Some(json!({ "applied": true })));
    assert_eq!(probe.effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        *probe.order.lock().expect("the probe lock"),
        ["persist", "sink"]
    );
    assert!(matches!(
        probe.journal_records().as_slice(),
        [Record::HumanAnswer {
            agent: who,
            text,
            provenance: Some(PeerProvenance::NoRecordedAgent),
            browser_answer: None,
        }] if *who == agent() && text == "ship it"
    ));
    assert_eq!(
        source.live_leases(),
        1,
        "the connection owns one live lease"
    );
}

#[test]
fn an_open_pre_hire_source_applies_memory_to_the_explicit_target_over_the_socket() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let source = Arc::new(OperatorAuthoritySource::from_replayed_agents(false));
    let probe = Probe::new(dir.path().join("journal"));
    let server = TestServer::start_with_authority(
        dir.path().join("run"),
        dispatcher(&probe),
        Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
        Arc::clone(&source),
    );
    let mut client = server.client();
    let response = client
        .call_as_operator(
            Method::MemoryAppend,
            Some(json!({ "agent": agent(), "text": "explicit pre-hire memory" })),
            credential(),
        )
        .expect("the real connection answered");
    let result: MemoryAppendResult = serde_json::from_value(response.result.expect("apply result"))
        .expect("typed memory result");

    let records = probe.journal_records();
    assert!(matches!(
        records.as_slice(),
        [Record::MemoryAppend {
            agent: target,
            text,
            body: None,
            provenance: Some(PeerProvenance::NoRecordedAgent),
        }] if *target == agent() && text == "explicit pre-hire memory"
    ));
    assert_eq!(result.lsn.segment(), 1);
    assert_eq!(*probe.order.lock().expect("the probe lock"), ["persist"]);
    assert_eq!(source.live_leases(), 1, "the socket owns the open lease");
}

#[test]
fn a_sealed_or_absent_source_suppresses_without_running_the_effect() {
    for with_source in [false, true] {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let source = Arc::new(OperatorAuthoritySource::from_replayed_agents(true));
        let probe = Probe::new(dir.path().join("journal"));
        let mut server = if with_source {
            TestServer::start_with_authority(
                dir.path().join("run"),
                dispatcher(&probe),
                Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
                Arc::clone(&source),
            )
        } else {
            TestServer::start_with_resolver(
                dir.path().join("run"),
                dispatcher(&probe),
                Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
            )
        };
        let response = server
            .client()
            .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
            .expect("the real connection answered");
        assert_eq!(error_code(&response), ErrorCode::Refused.code());
        assert_eq!(
            response
                .error
                .expect("the refusal")
                .data
                .expect("typed data")["gate"],
            serde_json::to_value(RefusalGate::ConditionedEffect).unwrap()
        );
        assert_eq!(probe.effects.load(Ordering::SeqCst), 0);
        let journal_records = probe.journal_records();
        assert!(matches!(
            journal_records.as_slice(),
            [Record::ConditionedEffectSuppressed {
                method: ConditionedMethod::AgentAnswer,
                target: ConditionedTarget::Agent { agent: who },
                provenance: PeerProvenance::NoRecordedAgent,
                reason,
                payload: Some(payload),
            }] if *who == agent()
                && reason == "operator authority unavailable"
                && payload.decoded_len == 7
                && payload.blake3 == blake3::hash(b"ship it").to_hex().to_string()
        ));
        assert_eq!(*probe.order.lock().expect("the probe lock"), ["persist"]);
        assert_eq!(source.live_leases(), 0);
        server.shutdown();
    }
}

#[test]
fn a_pre_hire_connection_survives_sealing_then_close_revokes_without_transfer() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let source = Arc::new(OperatorAuthoritySource::from_replayed_agents(false));
    let probe = Probe::new(dir.path().join("journal"));
    let server = TestServer::start_with_authority(
        dir.path().join("run"),
        dispatcher(&probe),
        Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
        Arc::clone(&source),
    );
    let mut pre_hire = server.client();

    let before = pre_hire
        .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
        .expect("the pre-hire connection is accepted");
    assert!(
        before.error.is_none(),
        "positive control: the source was live"
    );
    assert_eq!(source.live_leases(), 1);
    source.seal();
    let after = pre_hire
        .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
        .expect("the same connection survives sealing");
    assert!(after.error.is_none(), "the existing lease remains live");
    assert_eq!(probe.effects.load(Ordering::SeqCst), 2);

    drop(pre_hire);
    await_live_leases(&source, 0);
    let refused = server
        .client()
        .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
        .expect("a later connection is answered");
    assert_eq!(error_code(&refused), ErrorCode::Refused.code());
    assert_eq!(
        probe.effects.load(Ordering::SeqCst),
        2,
        "no lease transferred"
    );
}

#[test]
fn agent_acquisition_presentations_never_reach_the_effect_after_sealing() {
    let source = Arc::new(OperatorAuthoritySource::from_replayed_agents(false));

    // Positive control: the same source and real classifier can apply before it is sealed.
    {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let probe = Probe::new(dir.path().join("journal"));
        let server = TestServer::start_with_authority(
            dir.path().join("run"),
            dispatcher(&probe),
            Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
            Arc::clone(&source),
        );
        let response = server
            .client()
            .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
            .expect("the positive-control connection answered");
        assert!(response.error.is_none());
        assert_eq!(probe.effects.load(Ordering::SeqCst), 1);
    }
    await_live_leases(&source, 0);
    source.seal();

    // Nothing presented: the classification table resolves `Unproven`, and no lease is acquired.
    {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let probe = Probe::new(dir.path().join("journal"));
        let server = TestServer::start_with_authority(
            dir.path().join("run"),
            dispatcher(&probe),
            Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
            Arc::clone(&source),
        );
        let mut client = server.client();
        let response = client
            .call(Method::AgentAnswer, Some(answer()))
            .expect("the unproven connection answered");
        assert_eq!(error_code(&response), ErrorCode::Unauthorized.code());
        assert_eq!(probe.effects.load(Ordering::SeqCst), 0);
        assert_no_lease_while_connected(&source, &client, "unproven");
    }

    // Its own valid token: the classification table resolves the agent, and the operator-only requirement refuses.
    {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let probe = Probe::new(dir.path().join("journal"));
        let server = TestServer::start_with_authority(
            dir.path().join("run"),
            dispatcher(&probe),
            Arc::new(FakeResolver::new(PeerProvenance::Agent(agent()))),
            Arc::clone(&source),
        );
        let mut client = server.client();
        let response = client
            .call(
                Method::AgentAnswer,
                Some(json!({ "agent": agent(), "text": "ship it", "token": AGENT_TOKEN })),
            )
            .expect("the agent connection answered");
        assert_eq!(error_code(&response), ErrorCode::Unauthorized.code());
        assert_eq!(probe.effects.load(Ordering::SeqCst), 0);
        // `GrantAgentWhileSealed` raises this count despite the authorization refusal.
        assert_no_lease_while_connected(&source, &client, "agent-token");
        drop(client);
    }

    // A copied operator credential while still agent-correlated: the classification table refuses at the seam.
    {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let probe = Probe::new(dir.path().join("journal"));
        let server = TestServer::start_with_authority(
            dir.path().join("run"),
            dispatcher(&probe),
            Arc::new(FakeResolver::new(PeerProvenance::Agent(agent()))),
            Arc::clone(&source),
        );
        let mut client = server.client();
        let response = client
            .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
            .expect("the copied-credential connection answered");
        assert_eq!(error_code(&response), ErrorCode::Unauthorized.code());
        assert_eq!(probe.effects.load(Ordering::SeqCst), 0);
        // Authorization refusing the copied credential is not evidence about acquisition.
        assert_no_lease_while_connected(&source, &client, "agent-correlated copied-credential");
        drop(client);
    }

    // Decisive escaped case: the copied credential plus `NoRecordedAgent` passes the classification table, but the
    // independently sealed source supplied no lease, so the effect-local gate suppresses.
    {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let probe = Probe::new(dir.path().join("journal"));
        let server = TestServer::start_with_authority(
            dir.path().join("run"),
            dispatcher(&probe),
            Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
            Arc::clone(&source),
        );
        let mut client = server.client();
        let response = client
            .call_as_operator(Method::AgentAnswer, Some(answer()), credential())
            .expect("the escaped copied-credential connection answered");
        assert_eq!(error_code(&response), ErrorCode::Refused.code());
        assert_eq!(probe.effects.load(Ordering::SeqCst), 0);
        assert_no_lease_while_connected(&source, &client, "post-seal escaped");
        let journal_records = probe.journal_records();
        assert!(matches!(
            journal_records.as_slice(),
            [Record::ConditionedEffectSuppressed { reason, .. }]
                if reason == "operator authority unavailable"
        ));
    }

    assert_eq!(source.live_leases(), 0, "no post-seal connection acquired");
}
