//! The conditioned-method partition and registration guard at the dispatcher's public seam, plus the
//! derived `memory.append` text cap that refuses at that seam before anything is appended.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::memory::MemoryAppendParams;
use dudes_proto::rpc::{ErrorCode, JSONRPC, Method, Request, RequestId, Response, RpcError};
use dudes_proto::{ConditionedMethod, ConditionedTarget, Record};
use dudes_rpc::auth::{PeerClassifier, PeerProvenance, PeerRole};
use dudes_rpc::conditioned::MAX_MEMORY_APPEND_TEXT_BYTES;
use dudes_rpc::testing::{AdmitAll, FakeMintedTokens};
use dudes_rpc::{DecodedEffect, Dispatcher, Outbound, R5Method, Requirement};
use dudes_types::AgentId;
use serde_json::{Value, json};

#[test]
fn r5_is_exactly_seven_conditioned_methods_plus_agent_ask() {
    assert_eq!(ConditionedMethod::ALL.len(), 7);
    assert_eq!(R5Method::ALL.len(), 8);

    let conditioned: Vec<Method> = R5Method::ALL
        .iter()
        .filter_map(|method| match method {
            R5Method::Conditioned(method) => Some((*method).into()),
            R5Method::BoundedQuestion => None,
        })
        .collect();
    let expected: Vec<Method> = ConditionedMethod::ALL.into_iter().map(Into::into).collect();
    assert_eq!(conditioned, expected);
    let exact = BTreeSet::from([
        Method::AgentSteer,
        Method::AgentAnswer,
        Method::AgentAttach,
        Method::AgentDetach,
        Method::PtyWrite,
        Method::MailSend,
        Method::MemoryAppend,
        Method::AgentAsk,
    ]);
    let actual = Method::ALL
        .iter()
        .copied()
        .filter(|method| R5Method::from_method(*method).is_some())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual, exact,
        "the composite has exactly the accepted eight rows"
    );
    assert_eq!(
        actual.len(),
        R5Method::ALL.len(),
        "no conditioned row is duplicated"
    );
    assert_eq!(
        R5Method::ALL
            .iter()
            .filter(|method| matches!(method, R5Method::BoundedQuestion))
            .count(),
        1
    );
    assert_eq!(
        R5Method::from_method(Method::AgentAsk),
        Some(R5Method::BoundedQuestion)
    );
}

#[test]
fn ordinary_registration_refuses_the_whole_r5_set_with_an_ordinary_control() {
    for method in ConditionedMethod::ALL {
        let rejected = catch_unwind(AssertUnwindSafe(|| {
            let mut dispatcher = Dispatcher::new(AdmitAll);
            dispatcher.register(method.into(), Requirement::OperatorOrAgent, |_| {
                Ok(().into())
            });
        }));
        assert!(
            rejected.is_err(),
            "{} used the ordinary route",
            method.name()
        );
    }

    let ask = catch_unwind(AssertUnwindSafe(|| {
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register(Method::AgentAsk, Requirement::Agent, |_| Ok(().into()));
    }));
    assert!(ask.is_err(), "agent.ask used the ordinary route");

    let mut control = Dispatcher::new(AdmitAll);
    control.register(Method::SystemDoctor, Requirement::Operator, |_| {
        Ok(().into())
    });
    assert_eq!(
        control.requirement(Method::SystemDoctor),
        Some(Requirement::Operator),
        "planted ordinary registration proves the route accepts a non-conditioned method"
    );
}

#[test]
fn agent_ask_is_not_a_conditioned_method() {
    assert!(ConditionedMethod::try_from(Method::AgentAsk).is_err());
    for method in ConditionedMethod::ALL {
        assert_eq!(
            ConditionedMethod::try_from(Method::from(method)),
            Ok(method)
        );
    }
}

#[test]
fn every_conditioned_method_registers_only_through_the_conditioned_route() {
    let mut dispatcher = Dispatcher::new(AdmitAll);
    for method in ConditionedMethod::ALL {
        dispatcher.register_conditioned::<(), (), _, _, _>(
            method,
            Requirement::OperatorOrAgent,
            |_, _| {
                Err(RpcError {
                    code: ErrorCode::InvalidParams.code(),
                    message: "not dispatched by this registration test".into(),
                    data: None,
                })
            },
            |_| Ok(()),
            |_, ()| Ok(serde_json::Value::Null),
        );
        assert_eq!(
            dispatcher.requirement(method.into()),
            Some(Requirement::OperatorOrAgent),
            "{} entered the conditioned table",
            method.name()
        );
    }
    assert_eq!(
        dispatcher
            .methods()
            .filter(|method| ConditionedMethod::try_from(*method).is_ok())
            .count(),
        7,
        "all seven, and no hidden eighth, entered the conditioned table"
    );
}

/// The agent whose token every call below presents, and whose own memory it appends to.
fn alpha() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ulid")
}

/// A *different* agent: the target a non-owning append names, which the conditioned router quarantines.
fn bravo() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAY").expect("a ulid")
}

/// Alpha's token text, and a credential no call here presents. Both are needed to build the real
/// [`PeerClassifier`]: this test drives the shipped seam rather than [`AdmitAll`], because
/// `AdmitAll` resolves `Unproven`, and `Requirement::OperatorOrAgent` refuses an unproven peer
/// with `-32001` at the requirement gate — before the conditioned route is entered at all, so no
/// disposition is reached and every arm below would answer the same refusal. (`decide_memory`'s
/// own `Unproven` arm is unreachable through this seam for the same reason; it is covered by the
/// exhaustive table in `conditioned.rs`, which calls `decide` directly.)
const ALPHA_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CREDENTIAL: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";

/// What the injected persistence adapter was asked to make durable, if anything.
#[derive(Debug, PartialEq, Eq)]
enum Persisted {
    /// A quarantine record, carrying the two fields that make it evidence: why the table withheld
    /// the append, and the digest's observed `decoded_len`. The record never carries the bytes
    /// (there is no such store), so the length is what a replay has.
    Quarantined { reason: String, decoded_len: u64 },
    /// A suppression record. `memory.append` never produces one — its whole column is `Quarantine`
    /// — so this arm appearing at all is a finding.
    Suppressed,
    /// An applied effect, carrying the byte length the adapter would have written.
    Applied(usize),
}

/// The two `reason` strings this file expects to see journalled, spelled as `conditioned.rs`
/// spells them. They are journal content, replayed rather than re-derived, so pinning them here
/// is a ratchet on a wire value and not on an internal message.
const OVER_CEILING: &str = "memory text exceeds the append ceiling";
const NOT_OWNED: &str = "resolved agent does not own the target";

/// Every durable and effectful call the router made, in order.
#[derive(Debug, Default)]
struct Ledger {
    persisted: Mutex<Vec<Persisted>>,
    sank: Mutex<Vec<usize>>,
}

/// A `memory.append` decoder in the shape the conditioned router requires: the payload is the exact
/// decoded UTF-8 bytes of `text`, with no normalisation, and the target is the params' `agent`
/// when one is named or the resolved caller when none is.
fn decode_memory_append(
    params: Option<&Value>,
    role: &PeerRole,
) -> Result<DecodedEffect<String>, RpcError> {
    let request: MemoryAppendParams =
        serde_json::from_value(params.cloned().unwrap_or(Value::Null)).map_err(|err| RpcError {
            code: ErrorCode::InvalidParams.code(),
            message: err.to_string(),
            data: None,
        })?;
    let agent = match (request.agent, role) {
        (Some(agent), _) => agent,
        (None, PeerRole::Agent { agent }) => *agent,
        (None, _) => {
            return Err(RpcError {
                code: ErrorCode::InvalidParams.code(),
                message: "an operator call names its target".into(),
                data: None,
            });
        }
    };
    let payload = request.text.clone().into_bytes();
    Ok(DecodedEffect::new(
        request.text,
        ConditionedTarget::Agent { agent },
        Some(payload),
    ))
}

/// One `memory.append` of `text`, presented with alpha's token from a peer the daemon resolved
/// into alpha's tree, naming `target` when the call is an operator-shaped one.
fn append(text: &str, target: Option<AgentId>) -> (Response, Arc<Ledger>) {
    let ledger = Arc::new(Ledger::default());
    let for_persist = Arc::clone(&ledger);
    let for_sink = Arc::clone(&ledger);

    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        OperatorCredential::new(CREDENTIAL).expect("a credential"),
        Arc::new(FakeMintedTokens::minted(&[(ALPHA_TOKEN, alpha())])),
    ));
    dispatcher.register_conditioned(
        ConditionedMethod::MemoryAppend,
        Requirement::OperatorOrAgent,
        decode_memory_append,
        move |operation| {
            let made_durable = match operation.disposition_record() {
                Some(Record::MemoryAppendQuarantined {
                    reason, payload, ..
                }) => Persisted::Quarantined {
                    reason: reason.clone(),
                    decoded_len: payload.decoded_len,
                },
                Some(_) => Persisted::Suppressed,
                None => Persisted::Applied(
                    operation
                        .applied_effect()
                        .expect("a persistence operation is one arm or the other")
                        .params()
                        .len(),
                ),
            };
            for_persist
                .persisted
                .lock()
                .expect("the ledger's lock")
                .push(made_durable);
            Ok(())
        },
        move |applied, ()| {
            for_sink
                .sank
                .lock()
                .expect("the ledger's lock")
                .push(applied.into_params().len());
            Ok(json!({ "appended": true }))
        },
    );

    let mut params = json!({ "text": text, "token": ALPHA_TOKEN });
    if let Some(agent) = target {
        params["agent"] = json!(agent);
    }
    let request = Request {
        jsonrpc: JSONRPC.to_owned(),
        id: RequestId::Number(1),
        method: Method::MemoryAppend.name().to_owned(),
        params: Some(params),
        v: None,
        cred: None,
    };

    let response = dispatcher.dispatch(
        &request,
        PeerProvenance::Agent(alpha()),
        &Arc::new(Outbound::sink()),
    );
    (response, ledger)
}

/// A text over the derived cap is quarantined rather than applied — with its digest journalled —
/// and a text at exactly the cap is appended whole.
///
/// # Which assertion is load-bearing
///
/// Not the error code. A router that applied an over-cap append and refused afterwards would
/// answer `-32004` identically, having already fed memory the bytes the ceiling exists to keep
/// out. What tells those two routers apart is the ledger: arm 1 asserts the *only* durable call
/// was a `MemoryAppendQuarantined`, and that the sink — the effect continuation — never ran.
///
/// # Why quarantine and not a refusal above the table
///
/// The cap sits **inside** the disposition table as a fact (`Facts::over_cap`), not as a
/// stop above `decide()`. Dispatch admits exactly one pre-disposition stop, invalid params, and
/// says invalid params remain `-32602` and produce no disposition; a second
/// pre-disposition class would have made an over-cap call the one admitted `memory.append` that
/// produces no journal record at all. Quarantining instead keeps the table exhaustive, keeps the
/// evidence — `decoded_len`, asserted below — and earns `RefusalGate::ConditionedEffect` honestly,
/// because a disposition really was decided and journalled.
///
/// There is no size argument against journalling it: `MemoryAppendQuarantined` carries a
/// `PayloadDigest`, never the bytes. There is no quarantine store: the
/// digest is the replayable payload evidence, so an over-cap append puts a
/// 64-byte hash and a length on the journal, not the payload.
///
/// # The controls, because an assertion about one call is not evidence on its own
///
/// - **Arm 3** is the boundary control on the cap: the identical non-owning call one byte shorter
///   quarantines for the *other* reason. The `reason` field is what discriminates, so arm 2's
///   quarantine is attributable to the length rather than to the target.
/// - **Arm 4** is the boundary control on the whole route: exactly at the cap, from the owning
///   agent, applies and reaches the sink — so arm 1's quarantine is about length and not about a
///   fixture that never applies anything. The length that reached the adapter is the whole text:
///   refused rather than truncated.
///
/// # Failing cases, measured rather than predicted
///
/// - `>` weakened to `>=` in the derived fact: red at arm 3 (`reason` becomes the ceiling's) and
///   at arm 4 (the append quarantines instead of applying).
/// - `Facts::over_cap` ignored by `decide_memory`: red at arm 1 (`Applied` where a quarantine is
///   expected) and at arm 2 (the reason becomes the target's).
/// - The cap re-spelled off its derivation from `MAX_FRAME_BYTES` never reaches a test at all: it
///   is `error[E0080]` at the `const` assertion beside the constant.
#[test]
fn memory_append_over_cap_is_quarantined_and_exact_cap_is_appended() {
    let over = "m".repeat(MAX_MEMORY_APPEND_TEXT_BYTES + 1);
    let at_cap = "m".repeat(MAX_MEMORY_APPEND_TEXT_BYTES);
    let over_len = u64::try_from(MAX_MEMORY_APPEND_TEXT_BYTES + 1).expect("a length fits u64");
    let cap_len = u64::try_from(MAX_MEMORY_APPEND_TEXT_BYTES).expect("a length fits u64");

    // Arm 1 — the owning agent, one byte over. It would otherwise apply.
    let (refused, ledger) = append(&over, None);
    let err = refused.error.expect("one byte over the cap is refused");
    assert_eq!(err.code, ErrorCode::Refused.code());
    assert_eq!(
        err.data.expect("the conditioned-effect gate")["gate"],
        json!("conditioned_effect"),
        "the refusal names the gate because a disposition was decided and journalled"
    );
    assert_eq!(
        *ledger.persisted.lock().expect("the lock"),
        vec![Persisted::Quarantined {
            reason: OVER_CEILING.to_owned(),
            decoded_len: over_len,
        }],
        "exactly one durable call, and it is the quarantine — never the applied arm, or the \
         over-cap text entered memory"
    );
    assert!(
        ledger.sank.lock().expect("the lock").is_empty(),
        "and the effect continuation never ran"
    );

    // Arm 2 — a non-owning target, one byte over. Two reasons to quarantine; the cap is the one
    // recorded, and the digest carries the observed length as the evidence of what was withheld.
    let (refused, ledger) = append(&over, Some(bravo()));
    let err = refused
        .error
        .expect("a non-owning over-cap append is refused");
    assert_eq!(err.code, ErrorCode::Refused.code());
    assert_eq!(
        err.data.expect("the conditioned-effect gate")["gate"],
        json!("conditioned_effect")
    );
    assert_eq!(
        *ledger.persisted.lock().expect("the lock"),
        vec![Persisted::Quarantined {
            reason: OVER_CEILING.to_owned(),
            decoded_len: over_len,
        }],
        "exactly one MemoryAppendQuarantined, carrying the length rather than the bytes"
    );
    assert!(ledger.sank.lock().expect("the lock").is_empty());

    // Arm 3 — the cap's boundary control: the identical non-owning call at the cap quarantines for
    // the target instead, so arm 2's record is attributable to the length.
    let (quarantined, ledger) = append(&at_cap, Some(bravo()));
    let err = quarantined
        .error
        .expect("a non-owning append is refused after its quarantine is durable");
    assert_eq!(err.code, ErrorCode::Refused.code());
    assert_eq!(
        err.data.expect("the conditioned-effect gate")["gate"],
        json!("conditioned_effect")
    );
    assert_eq!(
        *ledger.persisted.lock().expect("the lock"),
        vec![Persisted::Quarantined {
            reason: NOT_OWNED.to_owned(),
            decoded_len: cap_len,
        }]
    );
    assert!(ledger.sank.lock().expect("the lock").is_empty());

    // Arm 4 — the route's positive control: exactly at the cap, from the owning agent, is appended
    // whole.
    let (admitted, ledger) = append(&at_cap, None);
    assert!(
        admitted.error.is_none(),
        "control: exactly at the cap is admitted: {:?}",
        admitted.error
    );
    assert_eq!(admitted.result.expect("a result")["appended"], json!(true));
    assert_eq!(
        *ledger.persisted.lock().expect("the lock"),
        vec![Persisted::Applied(MAX_MEMORY_APPEND_TEXT_BYTES)],
        "one persistence call, carrying the whole text: it is refused rather than truncated"
    );
    assert_eq!(
        *ledger.sank.lock().expect("the lock"),
        vec![MAX_MEMORY_APPEND_TEXT_BYTES],
        "and the sink ran once, after the adapter acknowledged durability"
    );
}
