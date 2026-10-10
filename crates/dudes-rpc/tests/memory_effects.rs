//! `memory.append` enters only through the production conditioned decoder and registration.

use std::sync::{Arc, Mutex};

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::memory::MemoryAppendParams;
use dudes_proto::rpc::{ErrorCode, JSONRPC, Method, Request, RequestId, RpcError};
use dudes_proto::{ConditionedTarget, PayloadDigest, PeerProvenance, Record};
use dudes_rpc::methods::{ConditionedMemoryControl, register_memory_effect};
use dudes_rpc::testing::FakeMintedTokens;
use dudes_rpc::{Dispatcher, Outbound, PersistenceOperation, Requirement};
use dudes_types::{AgentId, Lsn};
use serde_json::{Value, json};

const CREDENTIAL: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn agent() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ULID")
}

#[derive(Clone, Debug)]
enum Persisted {
    Applied {
        text: String,
        param_target: Option<AgentId>,
        target: AgentId,
        payload: Vec<u8>,
        digest: PayloadDigest,
        provenance: PeerProvenance,
    },
    Quarantined(Box<Record>),
}

#[derive(Debug, Default)]
struct MemoryControl {
    persisted: Mutex<Vec<Persisted>>,
}

impl ConditionedMemoryControl for MemoryControl {
    fn append(
        &self,
        operation: PersistenceOperation<'_, MemoryAppendParams>,
    ) -> Result<Lsn, RpcError> {
        let persisted = if let Some(record) = operation.disposition_record() {
            Persisted::Quarantined(Box::new(record.clone()))
        } else {
            let applied = operation
                .applied_effect()
                .expect("a persistence operation has exactly one arm");
            let ConditionedTarget::Agent { agent: target } = applied.target() else {
                panic!("memory has an agent target")
            };
            Persisted::Applied {
                text: applied.params().text.clone(),
                param_target: applied.params().agent,
                target: *target,
                payload: applied
                    .payload()
                    .expect("memory has payload bytes")
                    .to_vec(),
                digest: applied
                    .payload_digest()
                    .expect("memory has a payload digest")
                    .clone(),
                provenance: applied.provenance(),
            }
        };
        self.persisted
            .lock()
            .expect("the control lock")
            .push(persisted);
        Ok(Lsn(41))
    }
}

fn dispatcher(control: &Arc<MemoryControl>) -> Dispatcher {
    let mut dispatcher = Dispatcher::new(dudes_rpc::PeerClassifier::new(
        OperatorCredential::new(CREDENTIAL).expect("a credential"),
        Arc::new(FakeMintedTokens::minted(&[(TOKEN, agent())])),
    ));
    register_memory_effect(&mut dispatcher, control);
    dispatcher
}

fn request(params: Value, credential: Option<&str>) -> Request {
    Request {
        jsonrpc: JSONRPC.to_owned(),
        id: RequestId::Number(1),
        method: Method::MemoryAppend.name().to_owned(),
        params: Some(params),
        v: None,
        cred: credential.map(|value| OperatorCredential::new(value).expect("a credential")),
    }
}

#[test]
fn owning_agent_append_returns_the_acknowledged_lsn_and_exact_decoded_bytes() {
    for text in [
        "",
        "escaped \\\"quote\\\"",
        "mātauranga\nline two",
        // U+0000 is refused before any disposition; every other control is exact.
        "soh\u{1}byte",
        " \te\u{301}\n ",
    ] {
        let control = Arc::new(MemoryControl::default());
        let dispatcher = dispatcher(&control);
        assert_eq!(
            dispatcher.requirement(Method::MemoryAppend),
            Some(Requirement::OperatorOrAgent)
        );

        let response = dispatcher.dispatch(
            &request(json!({"text": text, "token": TOKEN}), None),
            PeerProvenance::Agent(agent()),
            &Arc::new(Outbound::sink()),
        );

        assert_eq!(response.error, None, "{response:?}");
        assert_eq!(response.result, Some(json!({"lsn": 41})));
        let persisted = control.persisted.lock().expect("the control lock");
        assert!(matches!(
            persisted.as_slice(),
            [Persisted::Applied {
                text: persisted_text,
                param_target,
                target,
                payload,
                digest,
                provenance: PeerProvenance::Agent(owner),
            }] if param_target.is_none()
                && persisted_text == text
                && *target == agent()
                && payload.as_slice() == text.as_bytes()
                && digest.decoded_len == u64::try_from(text.len()).unwrap()
                && digest.blake3 == blake3::hash(text.as_bytes()).to_hex().to_string()
                && *owner == agent()
        ));
    }
}

#[test]
fn caller_kind_decides_whether_the_explicit_target_is_absent_or_present() {
    for explicit in [agent(), AgentId::NIL] {
        let control = Arc::new(MemoryControl::default());
        let response = dispatcher(&control).dispatch(
            &request(
                json!({"agent": explicit, "text": "agent cannot choose", "token": TOKEN}),
                None,
            ),
            PeerProvenance::Agent(agent()),
            &Arc::new(Outbound::sink()),
        );
        assert_eq!(
            response.error.expect("agent target is invalid").code,
            ErrorCode::InvalidParams.code()
        );
        assert!(
            control
                .persisted
                .lock()
                .expect("the control lock")
                .is_empty()
        );
    }

    let control = Arc::new(MemoryControl::default());
    let absent = dispatcher(&control).dispatch(
        &request(json!({"text": "operator omitted target"}), Some(CREDENTIAL)),
        PeerProvenance::NoRecordedAgent,
        &Arc::new(Outbound::sink()),
    );
    assert_eq!(
        absent.error.expect("operator target is required").code,
        ErrorCode::InvalidParams.code()
    );
    assert!(
        control
            .persisted
            .lock()
            .expect("the control lock")
            .is_empty()
    );

    let response = dispatcher(&control).dispatch(
        &request(
            json!({"agent": agent(), "text": "explicit operator target"}),
            Some(CREDENTIAL),
        ),
        PeerProvenance::NoRecordedAgent,
        &Arc::new(Outbound::sink()),
    );
    assert_eq!(
        response.error.expect("no authority quarantines").code,
        ErrorCode::Refused.code()
    );
    let stored = control.persisted.lock().expect("the control lock");
    let [Persisted::Quarantined(record)] = stored.as_slice() else {
        panic!("one quarantined record");
    };
    assert!(matches!(record.as_ref(), Record::MemoryAppendQuarantined {
        agent: target, payload, provenance: PeerProvenance::NoRecordedAgent, ..
    } if *target == agent()
        && payload.decoded_len == 24
        && payload.blake3 == blake3::hash(b"explicit operator target").to_hex().to_string()));
}
