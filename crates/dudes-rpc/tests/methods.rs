//! One test per method, each driven over a real Unix socket, each asserting that what came back
//! deserialises into the type the contract crate names for that method.
//!
//! **Eight tests, because eight of the nine registered methods require `Operator`.**
//! `loop.pause`, `loop.resume` and `loop.halt` share their params, their result and their
//! requirement, which is exactly what makes folding them into one test look reasonable; a client
//! calls them by name and the dispatch table is keyed by name, so each gets its own.
//!
//! The eighth, `agent.ask` (an `Agent`-only method), is not here and its absence is deliberate: the seam
//! below resolves an operator, and `agent.ask` admits `Agent` alone, so a test of it in this file would
//! be a test of the refusal rather than of the answer. Its round trip over a real socket is
//! `bins/duded/tests/ask.rs`, which is where an agent-resolving column can be selected.
//!
//! What "deserialises into the contract type" is worth is decided by the fake behind the method:
//! each answer carries a **distinctive** value, and each test asserts that value. `HireResult` and
//! `GoalSetResult` are single-field structs, so a test that only checked "it decoded" would be
//! satisfied by any identifier at all — including one an echo server made up.

//! Unix-only: these drive the loopback harness, which binds a real Unix socket and is exported only
//! on Unix. Gated at the file rather than per test, because the `use` above the first test is what
//! a Windows build fails on.
#![cfg(unix)]

mod common;

use std::sync::Arc;

use common::{AdmitAsOperator, DIGEST, FakeControl, FakeJournal, GOAL, HIRED, TARGET};
use dudes_proto::rpc::agent::{HireResult, WhyResult};
use dudes_proto::rpc::goal::GoalSetResult;
use dudes_proto::rpc::{Empty, ErrorCode, Method, Response};
use dudes_rpc::testing::TestServer;
use dudes_rpc::{Dispatcher, Subscribers};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A server carrying the whole registered surface behind an admitting seam, so each test varies
/// only the call it makes.
///
/// The seam is [`AdmitAsOperator`] because every method this file drives is an `Operator` row;
/// what happens when the seam does *not* resolve an operator is `seam.rs`'s subject, not this
/// file's.
fn server(dir: &TempDir, control: &Arc<FakeControl>, journal: &Arc<FakeJournal>) -> TestServer {
    let mut dispatcher = Dispatcher::new(AdmitAsOperator);
    dudes_rpc::methods::register_all(
        &mut dispatcher,
        control,
        journal,
        &Arc::new(Subscribers::new()),
    );
    TestServer::start(dir.path().join("run"), dispatcher)
}

/// Call `method` with `params` against a server with a fresh [`FakeControl`], returning the answer
/// and the calls the control recorded.
fn call(method: Method, params: Value) -> (Response, Vec<String>) {
    let dir = TempDir::new().unwrap();
    let control = FakeControl::new();
    let journal = FakeJournal::empty();
    let server = server(&dir, &control, &journal);
    let response = server
        .client()
        .call(method, Some(params))
        .expect("an answer over the socket");
    (response, control.calls())
}

/// The result of a call that was expected to succeed, decoded into `T`.
fn result<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let value = response
        .result
        .unwrap_or_else(|| panic!("expected a result, got {:?}", response.error));
    serde_json::from_value(value).expect("the contract type")
}

/// `agent.hire`.
#[test]
fn agent_hire_answers_a_hire_result() {
    let (response, calls) = call(Method::AgentHire, json!(common::hire_params()));

    let hired: HireResult = result(response);
    assert_eq!(hired.agent.to_string(), HIRED, "the daemon's own answer");
    assert_eq!(
        calls,
        vec!["hire Kim sandbox=None".to_owned()],
        "the params reached it"
    );
}

/// `agent.retire`.
#[test]
fn agent_retire_answers_the_empty_result() {
    let (response, calls) = call(
        Method::AgentRetire,
        json!({ "agent": TARGET, "reason": "the quarter ended" }),
    );

    assert_eq!(result::<Empty>(response), Empty {});
    assert_eq!(
        calls,
        vec![format!("retire {TARGET} the quarter ended")],
        "the exact agent and reason reached the daemon"
    );
}

/// `agent.why`.
#[test]
fn agent_why_answers_a_why_result() {
    let (response, _) = call(Method::AgentWhy, json!({ "agent": TARGET }));

    let why: WhyResult = result(response);
    assert_eq!(why.input_digest, DIGEST);
    assert_eq!(why.lsn.0, 4_242);
    assert_eq!(why.gate_trace.len(), 1, "the trace travelled with it");
    assert!(why.gate_trace[0].fired);
}

/// `goal.set`.
#[test]
fn goal_set_answers_a_goal_set_result() {
    let (response, calls) = call(Method::GoalSet, json!(common::goal_set_params()));

    let set: GoalSetResult = result(response);
    assert_eq!(set.goal.to_string(), GOAL);
    assert_eq!(
        calls,
        vec![format!("set_goal {TARGET} make the suite green")],
        "the goal text reached the daemon verbatim"
    );
}

#[test]
fn goal_set_refuses_path_hash_before_control_and_the_connection_survives() {
    let dir = TempDir::new().unwrap();
    let control = FakeControl::new();
    let journal = FakeJournal::empty();
    let server = server(&dir, &control, &journal);
    let mut client = server.client();

    let refused = client
        .call(
            Method::GoalSet,
            Some(json!({
                "agent": TARGET,
                "text": "hash the artifact",
                "predicate": {"path_hash": {
                    "path": "dist/app.js",
                    "blake3": "abababababababababababababababababababababababababababababababab"
                }},
                "budget": {},
                "hop_cap": 12
            })),
        )
        .expect("the socket answers the unsupported predicate");
    let error = refused.error.expect("path_hash must be refused");
    assert_eq!(error.code, ErrorCode::Refused.code());
    assert_eq!(error.message, "goal.set: path_hash has no predicate runner");
    assert!(refused.result.is_none());
    assert!(control.calls().is_empty(), "no goal reached the daemon");

    let accepted = client
        .call(
            Method::GoalSet,
            Some(json!({
                "agent": TARGET,
                "text": "make the suite green",
                "predicate": {"command": {"argv": ["cargo", "nextest", "run"], "timeout_s": 600}},
                "budget": {},
                "hop_cap": 12
            })),
        )
        .expect("the same connection accepts a supported predicate");
    assert_eq!(result::<GoalSetResult>(accepted).goal.to_string(), GOAL);
    assert_eq!(
        control.calls(),
        vec![format!("set_goal {TARGET} make the suite green")]
    );
}

#[test]
fn goal_set_refuses_agent_verify_before_control_and_the_connection_survives() {
    let dir = TempDir::new().unwrap();
    let control = FakeControl::new();
    let journal = FakeJournal::empty();
    let server = server(&dir, &control, &journal);
    let mut client = server.client();

    let refused = client
        .call(
            Method::GoalSet,
            Some(json!({
                "agent": TARGET,
                "text": "verify the outcome",
                "predicate": {"agent_verify": {
                    "prompt": "Does the artifact satisfy the goal?",
                    "budget": 2_000_000
                }},
                "budget": {},
                "hop_cap": 12
            })),
        )
        .expect("the socket answers the unsupported predicate");
    let error = refused.error.expect("agent_verify must be refused");
    assert_eq!(error.code, ErrorCode::Refused.code());
    assert_eq!(
        error.message,
        "goal.set: agent_verify has no predicate runner"
    );
    assert!(refused.result.is_none());
    assert!(control.calls().is_empty(), "no goal reached the daemon");

    let accepted = client
        .call(
            Method::GoalSet,
            Some(json!({
                "agent": TARGET,
                "text": "make the suite green",
                "predicate": {"command": {"argv": ["cargo", "nextest", "run"], "timeout_s": 600}},
                "budget": {},
                "hop_cap": 12
            })),
        )
        .expect("the same connection accepts a supported predicate");
    assert_eq!(result::<GoalSetResult>(accepted).goal.to_string(), GOAL);
    assert_eq!(
        control.calls(),
        vec![format!("set_goal {TARGET} make the suite green")]
    );
}

#[test]
fn goal_set_accepts_board_state_done() {
    let (response, calls) = call(
        Method::GoalSet,
        json!({
            "agent": TARGET,
            "text": "finish the board card",
            "predicate": {"board_state": {
                "card": "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
                "status": "done"
            }},
            "budget": {},
            "hop_cap": 12
        }),
    );

    assert_eq!(result::<GoalSetResult>(response).goal.to_string(), GOAL);
    assert_eq!(
        calls,
        vec![format!("set_goal {TARGET} finish the board card")]
    );
}

/// `loop.pause`.
#[test]
fn loop_pause_answers_the_empty_result() {
    let (response, calls) = call(Method::LoopPause, json!({ "agent": TARGET }));

    assert_eq!(result::<Empty>(response), Empty {});
    assert_eq!(calls, vec![format!("pause {TARGET}")]);
}

/// `loop.resume`.
///
/// A separate test from `loop.pause` for a reason the assertion shows: the recorded call names
/// *resume*, so a dispatch table that routed every `loop.*` name to one handler would fail here.
#[test]
fn loop_resume_answers_the_empty_result() {
    let (response, calls) = call(Method::LoopResume, json!({ "agent": TARGET }));

    assert_eq!(result::<Empty>(response), Empty {});
    assert_eq!(calls, vec![format!("resume {TARGET}")]);
}

/// `loop.halt`.
#[test]
fn loop_halt_answers_the_empty_result() {
    let (response, calls) = call(Method::LoopHalt, json!({ "agent": TARGET }));

    assert_eq!(result::<Empty>(response), Empty {});
    assert_eq!(calls, vec![format!("halt {TARGET}")]);
}

/// `journal.tail`.
///
/// Answered here only as far as "the subscription was accepted and the response decodes into the
/// contract type". What it *delivers* is `tail.rs`'s subject.
#[test]
fn journal_tail_answers_the_empty_result_and_registers_a_subscriber() {
    let dir = TempDir::new().unwrap();
    let control = FakeControl::new();
    let journal = FakeJournal::empty();
    let subscribers = Arc::new(Subscribers::new());
    let mut dispatcher = Dispatcher::new(AdmitAsOperator);
    dudes_rpc::methods::register_all(&mut dispatcher, &control, &journal, &subscribers);
    let server = TestServer::start(dir.path().join("run"), dispatcher);

    assert!(subscribers.is_empty(), "nobody is subscribed yet");
    let mut client = server.client();
    let response = client
        .call(Method::JournalTail, Some(json!({})))
        .expect("an answer");

    assert_eq!(result::<Empty>(response), Empty {});
    assert_eq!(subscribers.len(), 1, "the connection is now subscribed");
}

/// At least one method fails cleanly on a malformed parameter rather than panicking.
///
/// Three assertions, and each rules out a different way of passing:
///
/// 1. the answer is `-32602` and not a dropped connection;
/// 2. the **same** connection answers a following call, so "cleanly" is measured rather than
///    assumed — a server that answered and then hung up looks identical from assertion 1 alone;
/// 3. the same method with well-formed params succeeds, so `-32602` is about *these* params and
///    not a handler that refuses everything.
#[test]
fn a_malformed_parameter_is_invalid_params_and_the_connection_survives() {
    let dir = TempDir::new().unwrap();
    let control = FakeControl::new();
    let journal = FakeJournal::empty();
    let server = server(&dir, &control, &journal);
    let mut client = server.client();

    let refused = client
        .call(Method::GoalSet, Some(json!({ "agent": 7 })))
        .expect("an answer, not a dropped connection");
    assert_eq!(
        refused.error.expect("an error").code,
        ErrorCode::InvalidParams.code(),
        "params that are not the method's shape are -32602"
    );
    assert!(
        control.calls().is_empty(),
        "the daemon was never asked to do anything"
    );

    let survived = client
        .call(Method::LoopPause, Some(json!({ "agent": TARGET })))
        .expect("the same connection still answers");
    assert!(survived.error.is_none(), "{:?}", survived.error);

    let (accepted, calls) = call(Method::GoalSet, json!(common::goal_set_params()));
    assert!(
        accepted.error.is_none(),
        "positive control: the same method accepts well-formed params: {:?}",
        accepted.error
    );
    assert_eq!(calls.len(), 1);
}

/// The contract code the daemon chose reaches the caller unchanged rather than being flattened
/// into an internal error.
///
/// `-32004 Refused` is the interesting one: it is the code a gate produces, and a door that mapped
/// every port failure to `-32603` would leave an operator unable to tell "a ceiling stopped this"
/// from "the daemon is broken".
#[test]
fn a_refusal_from_the_daemon_keeps_its_contract_code() {
    let dir = TempDir::new().unwrap();
    let control = FakeControl::refusing(ErrorCode::Refused);
    let journal = FakeJournal::empty();
    let server = server(&dir, &control, &journal);

    let response = server
        .client()
        .call(Method::LoopPause, Some(json!({ "agent": TARGET })))
        .expect("an answer");
    assert_eq!(
        response.error.expect("an error").code,
        ErrorCode::Refused.code()
    );

    // Control: the same call against a control that does not refuse succeeds, so the code above is
    // the fake's and not something the transport produces for every call.
    let (allowed, _) = call(Method::LoopPause, json!({ "agent": TARGET }));
    assert!(allowed.error.is_none(), "{:?}", allowed.error);
}

/// `agent.why` for an agent the journal does not know is `-32003 NotFound`, not an empty
/// `WhyResult`: "nothing decided yet" is an answer a caller must be able to tell from a decision.
#[test]
fn agent_why_for_an_unknown_agent_is_not_found() {
    let (response, _) = call(Method::AgentWhy, json!({ "agent": HIRED }));
    assert_eq!(
        response.error.expect("an error").code,
        ErrorCode::NotFound.code()
    );
}
