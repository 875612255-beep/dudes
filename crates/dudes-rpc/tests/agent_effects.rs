//! `agent.answer` and `agent.steer` enter only through the conditioned registration surface.

use std::sync::{Arc, Mutex};

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::{AgentText, ErrorCode, JSONRPC, Method, Request, RequestId, RpcError};
use dudes_proto::{ConditionedMethod, ConditionedTarget, Record};
use dudes_rpc::auth::{PeerClassifier, PeerProvenance};
use dudes_rpc::methods::{ConditionedAgentControl, register_agent_effects};
use dudes_rpc::testing::{AuthorityMutationConnection, FakeMintedTokens};
use dudes_rpc::{
    Dispatcher, MAX_AGENT_ANSWER_TEXT_BYTES, MAX_AGENT_STEER_TEXT_BYTES, OperatorAuthoritySource,
    Outbound, PersistenceOperation, Requirement,
};
use dudes_types::AgentId;
use serde_json::json;

const CREDENTIAL: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";

fn agent() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ULID")
}

#[derive(Debug, Default)]
struct Recorded {
    calls: Mutex<Vec<(ConditionedMethod, Record)>>,
    applied: Mutex<Vec<(ConditionedMethod, AgentText)>>,
}

impl Recorded {
    fn persist(&self, method: ConditionedMethod, operation: &PersistenceOperation<'_, AgentText>) {
        if let Some(record) = operation.disposition_record() {
            self.calls
                .lock()
                .expect("the fixture lock")
                .push((method, record.clone()));
        } else {
            self.applied.lock().expect("the fixture lock").push((
                method,
                operation
                    .applied_effect()
                    .expect("a persistence operation has one arm")
                    .params()
                    .clone(),
            ));
        }
    }
}

#[test]
fn agent_text_boundaries_use_the_real_decoded_utf8_payload() {
    let control = Arc::new(Recorded::default());
    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        OperatorCredential::new(CREDENTIAL).expect("a credential"),
        Arc::new(FakeMintedTokens::none()),
    ));
    register_agent_effects(&mut dispatcher, &control);
    let source = OperatorAuthoritySource::from_replayed_agents(false);
    let connection = AuthorityMutationConnection::acquire(&source, PeerProvenance::NoRecordedAgent)
        .expect("the pre-agent source grants the accepted connection");
    let outbound = Arc::new(Outbound::sink());

    for (method, cap) in [
        (Method::AgentAnswer, MAX_AGENT_ANSWER_TEXT_BYTES),
        (Method::AgentSteer, MAX_AGENT_STEER_TEXT_BYTES),
    ] {
        let at_cap = "\u{1}".repeat(cap);
        let response = connection.dispatch(
            &dispatcher,
            &Request {
                jsonrpc: JSONRPC.to_owned(),
                id: RequestId::Number(1),
                method: method.name().to_owned(),
                params: Some(json!({ "agent": agent(), "text": at_cap })),
                v: None,
                cred: Some(OperatorCredential::new(CREDENTIAL).expect("a credential")),
            },
            &outbound,
        );
        assert!(response.error.is_none(), "exactly at the ceiling applies");

        let response = connection.dispatch(
            &dispatcher,
            &Request {
                jsonrpc: JSONRPC.to_owned(),
                id: RequestId::Number(2),
                method: method.name().to_owned(),
                params: Some(json!({ "agent": agent(), "text": "x".repeat(cap + 1) })),
                v: None,
                cred: Some(OperatorCredential::new(CREDENTIAL).expect("a credential")),
            },
            &outbound,
        );
        assert_eq!(
            response.error.expect("cap + 1 is refused").code,
            ErrorCode::Refused.code()
        );
    }

    let applied = control.applied.lock().expect("the fixture lock");
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0].1.text.len(), MAX_AGENT_ANSWER_TEXT_BYTES);
    assert_eq!(applied[1].1.text.len(), MAX_AGENT_STEER_TEXT_BYTES);
    let calls = control.calls.lock().expect("the fixture lock");
    assert_eq!(calls.len(), 2, "one suppression per over-cap call");
    for (_, record) in calls.iter() {
        assert!(matches!(
            record,
            Record::ConditionedEffectSuppressed { payload: Some(payload), .. }
                if payload.decoded_len == 65_537
        ));
    }
}

impl ConditionedAgentControl for Recorded {
    fn answer(&self, operation: PersistenceOperation<'_, AgentText>) -> Result<(), RpcError> {
        self.persist(ConditionedMethod::AgentAnswer, &operation);
        Ok(())
    }

    fn steer(&self, operation: PersistenceOperation<'_, AgentText>) -> Result<(), RpcError> {
        self.persist(ConditionedMethod::AgentSteer, &operation);
        Ok(())
    }
}

/// Protects the conditioned agent-text decoder and registration: both method names retain their
/// exact target and payload under the Operator-only conditioned route.
#[test]
fn answer_and_steer_decode_target_and_payload_on_the_conditioned_route() {
    let control = Arc::new(Recorded::default());
    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        OperatorCredential::new(CREDENTIAL).expect("a credential"),
        Arc::new(FakeMintedTokens::none()),
    ));
    register_agent_effects(&mut dispatcher, &control);

    for method in [Method::AgentAnswer, Method::AgentSteer] {
        assert_eq!(dispatcher.requirement(method), Some(Requirement::Operator));
        let response = dispatcher.dispatch(
            &Request {
                jsonrpc: JSONRPC.to_owned(),
                id: RequestId::Number(1),
                method: method.name().to_owned(),
                params: Some(json!({
                    "agent": agent(),
                    "text": "keep the provenance",
                })),
                v: None,
                cred: Some(OperatorCredential::new(CREDENTIAL).expect("a credential")),
            },
            PeerProvenance::NoRecordedAgent,
            &Arc::new(Outbound::sink()),
        );
        assert_eq!(
            response
                .error
                .expect("no authority suppresses the effect")
                .code,
            ErrorCode::Refused.code()
        );
    }

    let calls = control.calls.lock().expect("the fixture lock");
    assert_eq!(calls.len(), 2);
    for (expected, actual) in [
        (ConditionedMethod::AgentAnswer, &calls[0]),
        (ConditionedMethod::AgentSteer, &calls[1]),
    ] {
        assert_eq!(actual.0, expected);
        assert!(matches!(
            &actual.1,
            Record::ConditionedEffectSuppressed {
                method,
                target: ConditionedTarget::Agent { agent: target },
                provenance: PeerProvenance::NoRecordedAgent,
                payload: Some(payload),
                ..
            } if *method == expected
                && *target == agent()
                && payload.decoded_len == 19
                && payload.blake3
                    == "638e410b62ffe331269d69f06139c10c74208f8e87d69821f5ae58d080da792b"
        ));
    }
}
