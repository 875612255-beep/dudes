//! The authorisation seam sits on the dispatch path — proved with both arms.
//!
//! A refusal test on its own cannot tell a working seam from a broken transport: a server that
//! answers `-32001` to everything and a server that answers nothing useful at all look the same
//! from one arm. So every test here runs the *same* call, against the *same* registered handler,
//! over the *same* kind of real socket, and changes exactly one thing — the [`Authorizer`], or, in
//! [`the_provenance_serve_resolved_reaches_the_seam`], the `PeerResolver` the transport asks.
//!
//! What is proved is that the seam is *on the path*. What is deliberately **not** proved, because
//! this crate does not implement it, is peer classification: the classification tables, the operator
//! credential, default-deny by provenance and token set membership are all elsewhere.

//! Unix-only: the harness these tests drive binds a real Unix socket, and `TestServer` is exported
//! only on Unix. Gated at the file rather than per test, because the `use` above the first test is
//! what a Windows build fails on.
#![cfg(unix)]

use dudes_proto::rpc::{ErrorCode, Method, Response};
use dudes_rpc::auth::{Authorizer, PeerClassifier, PeerProvenance, PeerRole, Presented, Refusal};
use dudes_rpc::testing::{AdmitAll, FakeMintedTokens, FakeResolver, RefuseAll, TestServer};
use dudes_rpc::{Dispatcher, PingOnly, Requirement, Subscribers};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

mod common;

/// What the method behind the seam answers when it is allowed to run. Distinctive, so "the call
/// succeeded" cannot be satisfied by an empty result.
const CANNED: &str = "the handler ran";

/// A server whose only variable is `authorizer`: the same two methods, the same handlers, the same
/// transport, every time.
fn server(dir: &TempDir, authorizer: impl Authorizer + 'static) -> TestServer {
    let mut dispatcher = Dispatcher::new(authorizer);
    dispatcher.register(Method::SystemDoctor, Requirement::AnyRole, |_| {
        Ok(json!({ "canned": CANNED }))
    });
    dispatcher.register(Method::SystemPing, Requirement::AnyRole, |_| {
        Ok(json!({ "canned": CANNED }))
    });
    TestServer::start(dir.path().join("run"), dispatcher)
}

/// Call `method` against a server built with `authorizer`.
fn call(authorizer: impl Authorizer + 'static, method: Method) -> Response {
    let dir = TempDir::new().unwrap();
    let server = server(&dir, authorizer);
    server.client().call(method, None).expect("an answer")
}

/// A method registered behind the seam is refused with the contract's permission-denied
/// code, and **the same call succeeds when the seam admits it**.
///
/// Arm A and arm B differ in the `Authorizer` and in nothing else. `system.doctor` is registered in
/// both, so arm A's `-32001` cannot be "no such method" wearing a different code, and arm B's
/// success cannot be a handler that only exists in the permissive build.
#[test]
fn a_method_behind_the_seam_is_refused_and_the_same_call_succeeds_when_admitted() {
    let refused = call(PingOnly, Method::SystemDoctor);
    assert_eq!(
        refused.error.expect("an error").code,
        ErrorCode::Unauthorized.code(),
        "arm A: the default seam refuses with -32001"
    );

    let admitted = call(AdmitAll, Method::SystemDoctor);
    assert_eq!(
        admitted.result.expect("a result")["canned"],
        CANNED,
        "arm B: the identical call reaches the identical handler"
    );
}

/// Arm A above is not passing because everything is refused: under the same default seam,
/// `system.ping` succeeds.
///
/// This is the third assertion the seam needs. Without it, an `Authorizer` that returned an error
/// unconditionally — or a transport that had stopped dispatching at all — would satisfy the
/// refusal arm perfectly.
#[test]
fn the_default_seam_still_admits_the_one_method_it_is_meant_to() {
    let admitted = call(PingOnly, Method::SystemPing);
    assert!(
        admitted.error.is_none(),
        "the liveness probe must survive default-deny: {:?}",
        admitted.error
    );
    assert_eq!(admitted.result.expect("a result")["canned"], CANNED);
}

/// The seam is the *only* gate: with an authorizer that refuses everything, even the method the
/// default seam admits is refused.
///
/// This is what makes the seam a seam rather than a special case around one method. If `system.ping`
/// were admitted somewhere in the transport instead of by the `Authorizer`, this test would find
/// it — and it is exactly the shortcut someone would reach for to keep a liveness probe working.
#[test]
fn no_method_bypasses_the_seam() {
    for method in [Method::SystemPing, Method::SystemDoctor] {
        let refused = call(RefuseAll, method);
        assert_eq!(
            refused.error.expect("an error").code,
            ErrorCode::Unauthorized.code(),
            "`{}` must go through the seam like everything else",
            method.name()
        );
    }
}

/// An `Authorizer` that resolves a role hands that role to the handler. The seam decides *and*
/// classifies; the transport carries the verdict rather than recomputing it.
#[test]
fn the_role_the_seam_resolves_reaches_the_handler() {
    /// Resolves every call to `Operator`, so the role reaching the handler can only have come from
    /// the seam — nothing in `dudes-rpc` produces that role on its own.
    #[derive(Debug)]
    struct AlwaysOperator;

    impl Authorizer for AlwaysOperator {
        fn authorize(
            &self,
            _presented: Presented,
            _provenance: PeerProvenance,
            _method: &str,
        ) -> Result<PeerRole, Refusal> {
            Ok(PeerRole::Operator)
        }
    }

    let dir = TempDir::new().unwrap();
    let mut dispatcher = Dispatcher::new(AlwaysOperator);
    dispatcher.register(Method::SystemPing, Requirement::AnyRole, |call| {
        Ok(json!({ "role": call.role.name() }))
    });
    let server = TestServer::start(dir.path().join("run"), dispatcher);

    let response = server.client().call(Method::SystemPing, None).unwrap();
    assert_eq!(response.result.expect("a result")["role"], "operator");

    // Control: the same handler under the shipped seam reports the role this crate actually
    // produces, so the assertion above is reading the seam and not a constant.
    let dir = TempDir::new().unwrap();
    let mut dispatcher = Dispatcher::new(PingOnly);
    dispatcher.register(Method::SystemPing, Requirement::AnyRole, |call| {
        Ok(json!({ "role": call.role.name() }))
    });
    let server = TestServer::start(dir.path().join("run"), dispatcher);
    let response = server.client().call(Method::SystemPing, None).unwrap();
    assert_eq!(response.result.expect("a result")["role"], "unproven");
}

/// **The provenance `serve` resolved reaches the seam, and it was resolved from this connection's
/// own peer.**
///
/// This is the end-to-end half of the check: `resolve` is called once, in `serve`'s
/// connection frame, and its answer is what every call on that connection is decided under. The
/// peer stops at the seam and never reaches a handler.
///
/// Three things are asserted together, and each is what makes the next mean anything:
///
/// 1. **The transport asked, about this peer.** The resolver was handed a [`PeerIdentity`]
///    carrying the pid of *this test process*, because the peer of a loopback socket is the test
///    itself. A transport that passed `PeerIdentity::unknown()` — or a default — fails here.
/// 2. **The answer decided the call.** The verdict is `agent`, which the classifier can only reach
///    from classification-table row 2 column 1, and no other input to this server names an agent.
/// 3. **The control: one column over, on an otherwise identical server, the same call is
///    refused.** Without it, a `serve` that ignored the resolver and a classifier that admitted
///    everything would pass arms 1 and 2 together.
///
/// It asserts **once per connection**, not once per call: the resolver records what it was asked,
/// and two calls over one connection must leave one entry. That is the property the resolver contract needs,
/// and a `serve` that resolved per call would satisfy every verdict above and fail this line.
#[test]
fn the_provenance_serve_resolved_reaches_the_seam() {
    /// The agent whose token the connection presents.
    fn alpha() -> dudes_types::AgentId {
        dudes_types::AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ulid")
    }
    const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let seam = || {
        let credential =
            dudes_proto::rpc::cred::OperatorCredential::new("C".repeat(43)).expect("a credential");
        let mut dispatcher = Dispatcher::new(PeerClassifier::new(
            credential,
            Arc::new(FakeMintedTokens::minted(&[(TOKEN, alpha())])),
        ));
        dispatcher.register(Method::SystemPing, Requirement::AnyRole, |call| {
            Ok(json!({ "role": call.role.name() }))
        });
        dispatcher
    };

    let dir = TempDir::new().unwrap();
    let resolver = Arc::new(FakeResolver::new(PeerProvenance::Agent(alpha())));
    let server = TestServer::start_with_resolver(
        dir.path().join("run"),
        seam(),
        Arc::clone(&resolver) as Arc<dyn dudes_rpc::auth::PeerResolver>,
    );

    let mut client = server.client();
    let params = || Some(json!({ "token": TOKEN }));
    let first = client.call(Method::SystemPing, params()).unwrap();
    let second = client.call(Method::SystemPing, params()).unwrap();

    assert_eq!(
        first.result.expect("a result")["role"],
        "agent",
        "the column `serve` resolved is what the seam decided under"
    );
    assert_eq!(second.result.expect("a result")["role"], "agent");

    let asked = resolver.asked();
    assert_eq!(
        asked.len(),
        1,
        "resolved once for the connection, not once per call: {asked:?}"
    );
    assert_eq!(
        asked[0].pid.reported(),
        Some(i32::try_from(std::process::id()).expect("a pid fits")),
        "and it was asked about *this* connection's peer, not about a default"
    );

    // The control, one column over on an otherwise identical server.
    let dir = TempDir::new().unwrap();
    let control = TestServer::start_with_resolver(
        dir.path().join("run"),
        seam(),
        Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
    );
    let refused = control.client().call(Method::SystemPing, params()).unwrap();
    assert_eq!(
        refused.error.expect("an error").code,
        ErrorCode::Unauthorized.code(),
        "control: the same token on the same seam, one column over, is refused — so the verdict \
         above is the resolver's answer and not a server that admits everything"
    );
}

/// An `Operator` method is refused with the contract's permission-denied code for a
/// caller the seam does not classify as `Operator`, **with the admitting arm run as its control**.
///
/// Three arms over one variable — the `Authorizer` — with the transport, the method table, the
/// request and the peer held constant. `loop.pause` stands for the
/// **eight `Operator` rows** `register_all` carries, which it can because they share a requirement.
/// It does not stand for the ninth: `agent.ask` is `Agent`-only, so a seam
/// that resolves an operator is the wrong instrument for it entirely. What refuses an operator
/// *there* is `bins/duded/tests/ask.rs`, over a real socket and the shipped classifier.
///
/// - **Shipped.** `PingOnly` refuses at the seam. This is the honest statement about what a built
///   daemon does today: no peer is an operator, because nothing classifies one.
/// - **Admitted, then refused.** `AdmitAll` admits *and resolves to `Unproven`* — a caller the seam
///   does not classify as `Operator` — so its `-32001` comes from the method's own requirement,
///   past a seam that said yes. The message assertion is what tells the two refusals apart: arms 1
///   and 2 carry the same code and different reasons, and without it this arm would be
///   indistinguishable from arm 1.
/// - **Admitting control.** `AdmitAsOperator` resolves the identical call to `Operator`, and the
///   handler runs. Without it, a transport that had stopped dispatching altogether would satisfy
///   both refusal arms perfectly.
#[test]
fn an_operator_method_is_refused_for_a_caller_the_seam_does_not_classify_as_operator() {
    /// One server per authorizer, carrying the real `loop.pause` handler behind its real
    /// requirement — registered through `register_all`, so what is exercised is the row that ships.
    fn pause(authorizer: impl Authorizer + 'static) -> Response {
        let dir = TempDir::new().unwrap();
        let mut dispatcher = Dispatcher::new(authorizer);
        dudes_rpc::methods::register_all(
            &mut dispatcher,
            &common::FakeControl::new(),
            &common::FakeJournal::empty(),
            &Arc::new(Subscribers::new()),
        );
        let server = TestServer::start(dir.path().join("run"), dispatcher);
        server
            .client()
            .call(Method::LoopPause, Some(json!({ "agent": common::TARGET })))
            .expect("an answer")
    }

    let at_the_seam = pause(PingOnly).error.expect("arm 1: an error");
    assert_eq!(at_the_seam.code, ErrorCode::Unauthorized.code());

    let at_the_requirement = pause(AdmitAll).error.expect("arm 2: an error");
    assert_eq!(
        at_the_requirement.code,
        ErrorCode::Unauthorized.code(),
        "the contract's permission-denied code"
    );
    assert_eq!(
        at_the_requirement.message,
        Requirement::Operator.refusal(),
        "arm 2 was refused by the method's requirement, not by the seam"
    );
    assert_ne!(
        at_the_seam.message, at_the_requirement.message,
        "the two refusals are not one refusal wearing the same code twice"
    );

    let admitted = pause(common::AdmitAsOperator);
    assert!(
        admitted.error.is_none(),
        "arm 3: the identical call runs when the seam resolves an operator: {:?}",
        admitted.error
    );
    let ok: dudes_proto::rpc::Empty =
        serde_json::from_value(admitted.result.expect("a result")).expect("the contract type");
    assert_eq!(ok, dudes_proto::rpc::Empty {});
}
