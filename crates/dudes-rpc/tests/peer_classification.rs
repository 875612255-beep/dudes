//! Peer classification over a real Unix socket: the end-to-end arm that shows the cells reach the
//! wire.
//!
//! # The grid itself is not in this file
//!
//! One assertion per cell of the classification table — the grid in row and column order, both of its
//! positive controls, and everything else that calls `classify` directly — is `tests/peer_classification_cells.rs`,
//! which binds no socket and carries no `cfg`. This file is the other half: it takes the same
//! classifier, puts it behind a real server whose transport resolves a scripted column, and
//! asserts that the verdicts survive the dispatch path.
//!
//! # What a fake resolver cannot tell you
//!
//! The provenance column is *scripted* here too. That is the point of the seam — the alternative
//! is a process-spawning suite that is slow and flaky by construction — but it
//! means nothing in this file is evidence about the ancestry walk itself, which does not exist
//! yet, or about what macOS reports for a peer that handed its socket to a grandchild. Those are
//! `PeerResolver`'s, and its own documentation says what it owes.
//!
//! # Unix only, like every sibling that binds a socket
//!
//! Every test below goes through [`server`], which binds a real Unix socket and needs
//! `testing::TestServer`, which is `#[cfg(unix)]`. Without the attribute below this file fails to
//! compile for `x86_64-pc-windows-msvc` — which the three-OS matrix would have found and a macOS
//! run does not.
//!
//! The residual is therefore the socket-binding tests in this file, and only those. They assert
//! that a verdict crosses a Unix socket, so there is nothing to run on a platform that has none;
//! the Windows leg needs a named-pipe transport before any of them means anything there. What is
//! not residual is the grid: those cells compile and run on every leg of the matrix,
//! in the file named above.
#![cfg(unix)]

mod common;

use std::sync::Arc;

use common::{FakeControl, FakeJournal};
use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::{ErrorCode, Method, RefusalGate, RefusedData};
use dudes_rpc::auth::{PeerClassifier, PeerProvenance};
use dudes_rpc::testing::{FakeMintedTokens, FakeResolver, TestServer};
use dudes_rpc::{Dispatcher, Requirement, Subscribers};
use dudes_types::AgentId;
use serde_json::json;
use tempfile::TempDir;

/// The agent whose token every row-2 test presents.
fn alpha() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ulid")
}

/// A *different* agent — the sibling whose tree a stolen token gets called from.
fn bravo() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAY").expect("a ulid")
}

/// Alpha's token text. Distinctive per agent so a projection that answered for any token at all
/// could not pass for one that answered for this one.
const ALPHA_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// A token no minting names.
const UNMINTED_TOKEN: &str = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
/// This daemon's operator credential.
const CREDENTIAL: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";

fn credential() -> OperatorCredential {
    OperatorCredential::new(CREDENTIAL).expect("a credential")
}

/// A classifier holding this daemon's credential and a projection that mints [`ALPHA_TOKEN`] for
/// alpha and nothing else.
fn classifier() -> PeerClassifier {
    PeerClassifier::new(
        credential(),
        Arc::new(FakeMintedTokens::minted(&[(ALPHA_TOKEN, alpha())])),
    )
}

/// A server whose seam is a real [`PeerClassifier`] and whose transport resolves `provenance` for
/// every peer.
///
/// **The column is the server's, not the classifier's.** It is resolved once per connection in
/// `serve`'s own frame, so selecting a column here is selecting a resolver — which is
/// also what makes these tests evidence that the resolved value survives the dispatch path rather
/// than evidence about a value the seam made up.
///
/// `system.ping` is `Unproven`-admitting; the ordinary operator surface is registered by
/// `register_all`; conditioned `agent.answer` and `agent.steer` are registered by
/// `register_agent_effects`; `board.claim` is (`Agent` · `Operator`)
/// (`Agent` · `Operator`) and is registered here rather than there because the daemon-side handler
/// for it does not exist yet — the handler below answers with the resolved role and nothing else,
/// which is all this file asserts about it.
fn server(dir: &TempDir, provenance: PeerProvenance) -> TestServer {
    let mut dispatcher = Dispatcher::new(classifier());
    dispatcher.register(Method::SystemPing, Requirement::AnyRole, |call| {
        Ok(json!({ "role": call.role.name() }))
    });
    dispatcher.register(Method::BoardClaim, Requirement::OperatorOrAgent, |call| {
        Ok(json!({ "role": call.role.name() }))
    });
    let control = FakeControl::new();
    dudes_rpc::methods::register_all(
        &mut dispatcher,
        &control,
        &FakeJournal::empty(),
        &Arc::new(Subscribers::new()),
    );
    dudes_rpc::methods::register_agent_effects(&mut dispatcher, &control);
    TestServer::start_with_resolver(
        dir.path().join("run"),
        dispatcher,
        Arc::new(FakeResolver::new(provenance)),
    )
}

fn code(response: &dudes_proto::rpc::Response) -> i32 {
    response.error.as_ref().expect("an error response").code
}

/// A peer that presents nothing reaches `system.ping` and nothing else, end to end.
///
/// Both arms. `system.ping` answers — that is the whole reason `Unproven` is a role rather than a
/// closed connection, and a daemon that refused it would be unprobeable before it is configured —
/// and every operator row is `-32001`.
#[test]
fn a_peer_presenting_nothing_reaches_only_system_ping() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::NoRecordedAgent);
    let mut client = server.client();

    let ping = client.call(Method::SystemPing, None).unwrap();
    assert_eq!(
        ping.result.expect("a result")["role"],
        "unproven",
        "positive control: the liveness probe answers, and says what the peer resolved to"
    );

    for method in [
        Method::LoopPause,
        Method::AgentHire,
        Method::GoalSet,
        Method::JournalTail,
    ] {
        let refused = client.call(method, Some(json!({}))).unwrap();
        assert_eq!(
            code(&refused),
            ErrorCode::Unauthorized.code(),
            "{} is an operator row and this peer proved nothing",
            method.name()
        );
    }
}

/// An agent-side peer calling a method **that is registered** on the operator table is refused
/// `-32001`.
///
/// The control is the same call from the same server presenting the operator credential, which
/// succeeds — so what is asserted is the *role*, not a server that refuses everything.
#[test]
fn an_agent_side_peer_is_refused_an_operator_row() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::Agent(alpha()));
    let mut client = server.client();

    let refused = client
        .call(
            Method::LoopPause,
            Some(json!({ "agent": common::TARGET, "token": ALPHA_TOKEN })),
        )
        .unwrap();
    assert_eq!(
        code(&refused),
        ErrorCode::Unauthorized.code(),
        "an agent is not an operator: the roles are a set, not a ladder"
    );

    let allowed = client
        .call(Method::BoardClaim, Some(json!({ "token": ALPHA_TOKEN })))
        .unwrap();
    assert_eq!(
        allowed.result.expect("a result")["role"],
        "agent",
        "positive control: the same peer, the same token, on a row that admits an agent"
    );
}

/// An agent cannot declare trust in the repository it works in: `agent.hire`
/// is an operator row, so a hire carrying `trusted: true` from an agent-side peer is `-32001`
/// before any handler reads the field.
#[test]
fn an_agent_side_peer_cannot_declare_trust_at_a_hire() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::Agent(alpha()));
    let mut client = server.client();

    let mut request = json!(common::hire_params());
    request["trusted"] = json!(true);
    request["token"] = json!(ALPHA_TOKEN);
    let refused = client.call(Method::AgentHire, Some(request)).unwrap();
    assert_eq!(
        code(&refused),
        ErrorCode::Unauthorized.code(),
        "an agent is not an operator, so its declaration of trust never reaches the hire"
    );
}

/// `agent.answer` is registered through the conditioned route and an agent-side peer is refused
/// at its `Operator` requirement, before disposition or persistence.
#[test]
fn an_agent_side_peer_is_refused_agent_answer_at_the_operator_requirement() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::Agent(alpha()));
    let mut client = server.client();

    let answered = client
        .call(
            Method::AgentAnswer,
            Some(json!({
                "agent": common::TARGET,
                "text": "ship it",
                "token": ALPHA_TOKEN,
            })),
        )
        .unwrap();
    assert_eq!(
        code(&answered),
        ErrorCode::Unauthorized.code(),
        "the seam admits the agent role and the method's Operator requirement refuses it"
    );
    assert_eq!(
        code(
            &client
                .call(Method::LoopPause, Some(json!({ "agent": common::TARGET })))
                .unwrap()
        ),
        ErrorCode::Unauthorized.code(),
        "positive control: a registered operator row does refuse this peer with -32001"
    );
}

/// The operator credential reaches the operator table over the wire, and a wrong one does not.
///
/// This is the operator-credential case end to end: the field on the envelope, the constant-time comparison at the seam, and
/// an operator row that runs. The refusing arm uses a *well-formed but different* credential, so
/// what it measures is the comparison rather than the parser.
#[test]
fn the_operator_credential_on_the_envelope_reaches_the_operator_table() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::NoRecordedAgent);
    let mut client = server.client();

    let allowed = client
        .call_as_operator(
            Method::LoopPause,
            Some(json!({ "agent": common::TARGET })),
            credential(),
        )
        .unwrap();
    assert!(
        allowed.error.is_none(),
        "positive control: the credential reached the operator table: {allowed:?}"
    );

    let wrong = OperatorCredential::new("D".repeat(43)).unwrap();
    let refused = client
        .call_as_operator(
            Method::LoopPause,
            Some(json!({ "agent": common::TARGET })),
            wrong,
        )
        .unwrap();
    assert_eq!(
        code(&refused),
        ErrorCode::Unauthorized.code(),
        "a credential this daemon did not mint presents nothing it recognises"
    );
}

/// The disqualification of an operator credential over the wire: the same credential that worked above is refused from a peer the daemon
/// placed inside an agent's tree.
#[test]
fn an_operator_credential_from_an_owned_peer_is_refused_over_the_wire() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::Agent(bravo()));
    let mut client = server.client();

    let refused = client
        .call_as_operator(
            Method::LoopPause,
            Some(json!({ "agent": common::TARGET })),
            credential(),
        )
        .unwrap();
    assert_eq!(code(&refused), ErrorCode::Unauthorized.code());
}

/// **The credential oracle, executed.** A peer can tell the daemon's credential from any other in
/// one `system.ping`, and no wording of the refusal message reaches that.
///
/// The two arms differ in whether an answer comes back at all, which is a property of the table's
/// shape rather than of any string:
///
/// - a **wrong** credential is classification-table row 4 in every column, resolves to `Unproven`, and
///   `system.ping` carries `Requirement::AnyRole`, which admits every role — so the call
///   **answers**;
/// - the **right** credential from a peer inside an agent's tree is row 1 column 1, refused at the
///   seam, and the seam runs before the method is looked up — so it is `-32001` on every method,
///   `system.ping` included.
///
/// Message discipline is still worth having and this does not argue against it; what it settles is
/// that the discipline is not what stops this, so nobody spends a diagnostic buying it.
/// `Requirement::refusal` therefore stays distinct from the seam's one string.
///
/// The provenance is the only variable between the two servers, and the credential is the only
/// variable between the two calls — so the difference below is the cell and not the setup.
#[test]
fn one_ping_separates_the_daemons_credential_from_any_other() {
    // Arm A: the wrong credential, from a peer in no agent's tree. Row 4 -> Unproven -> admitted.
    let dir_a = TempDir::new().unwrap();
    let server_a = server(&dir_a, PeerProvenance::NoRecordedAgent);
    let wrong = OperatorCredential::new("D".repeat(43)).unwrap();
    let answered = server_a
        .client()
        .call_as_operator(Method::SystemPing, None, wrong)
        .unwrap();
    assert!(
        answered.error.is_none(),
        "a credential this daemon does not know presents nothing, and nothing may ping: \
         {answered:?}"
    );
    assert_eq!(
        answered.result.expect("a result")["role"],
        "unproven",
        "and it is answered as the role row 4 resolves to"
    );

    // Arm B: the daemon's own credential, from a peer it placed inside an agent's tree. Row 1
    // column 1 -> refused at the seam, before `system.ping` is looked up.
    let dir_b = TempDir::new().unwrap();
    let server_b = server(&dir_b, PeerProvenance::Agent(alpha()));
    let refused = server_b
        .client()
        .call_as_operator(Method::SystemPing, None, credential())
        .unwrap();
    assert_eq!(
        code(&refused),
        ErrorCode::Unauthorized.code(),
        "the right credential from a disqualified peer is refused on the one method that admits \
         every role"
    );

    // The control that the second server is not simply broken: from a peer in no agent's tree it
    // answers the same call, so what refused above is the provenance column and not the setup.
    let dir_c = TempDir::new().unwrap();
    let server_c = server(&dir_c, PeerProvenance::NoRecordedAgent);
    let control = server_c
        .client()
        .call_as_operator(Method::SystemPing, None, credential())
        .unwrap();
    assert!(
        control.error.is_none(),
        "control: the same credential and the same method, from an unowned peer, answers: \
         {control:?}"
    );
    assert_eq!(control.result.expect("a result")["role"], "operator");
}

/// Row 3 reaches the wire as `-32004` with the typed gate, from a real connection.
#[test]
fn an_unminted_token_reaches_the_wire_as_refused_token() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, PeerProvenance::Agent(alpha()));
    let mut client = server.client();

    let refused = client
        .call(Method::BoardClaim, Some(json!({ "token": UNMINTED_TOKEN })))
        .unwrap();
    let error = refused.error.expect("an error response");
    assert_eq!(error.code, ErrorCode::Refused.code());
    let data: RefusedData = serde_json::from_value(error.data.expect("typed data")).unwrap();
    assert_eq!(data.gate, RefusalGate::Token);

    let allowed = client
        .call(Method::BoardClaim, Some(json!({ "token": ALPHA_TOKEN })))
        .unwrap();
    assert!(
        allowed.error.is_none(),
        "positive control: the minted token on the same connection is honoured"
    );
}
