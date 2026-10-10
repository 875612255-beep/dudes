//! A connection whose provenance resolution never answered is refused before the dispatcher.
//!
//! # What is counted, and why it is the dispatcher and not the client
//!
//! The obvious observable — "the client's call failed" — cannot tell a refusal apart from a server
//! that died, a socket that was never bound, or a handler that panicked. So the counter lives on
//! the *server* side: a handler registered behind the seam increments an `AtomicUsize` before it
//! answers, and the proposition is that it never ran. A killed client cannot produce that number.
//!
//! # The happens-before, which is the whole of the arm
//!
//! Asserting `0` proves nothing on its own: `0` is equally what you get when the connection is
//! refused *before the client managed to write anything*, and a refusal that only beats a slow
//! client is not the refusal this file is about. So the refusing resolution **blocks** until the
//! client's framed write has returned. The write is therefore ordered before the refusal by
//! construction, and the count is read after the server has closed the connection — a point the
//! refusal path has provably reached, not a sleep.
//!
//! Asserting the write instead would not discriminate: against a server that simply drops the
//! connection the write still succeeds 19 times in 20, so "the client wrote" proves nothing.
//!
//! # The control
//!
//! Same server, same dispatcher, same socket, one thing changed: the next connection's resolution
//! *answers* a column. The handler then runs and returns its canned result, the counter reads `1`,
//! and the resolver reports two connections asked. Without it, `0` is also what a dead server
//! gives — and a dead server is a live possibility here, since a refused connection looks from
//! outside exactly like one.
#![cfg(unix)]

use std::io::Read as _;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use dudes_proto::rpc::{Method, Request, RequestId};
use dudes_rpc::auth::{PeerIdentity, PeerProvenance, PeerResolution, PeerResolver};
use dudes_rpc::testing::{AdmitAll, FakeResolver, TestServer};
use dudes_rpc::{Dispatcher, Requirement, write_frame};
use serde_json::json;
use tempfile::TempDir;

/// What the handler behind the seam answers when it is allowed to run. Distinctive, so "the call
/// succeeded" cannot be satisfied by an empty result.
const CANNED: &str = "the handler ran";

/// How long the refused connection is given to end.
///
/// **A liveness guard, not a measurement.** A refusal ends the connection at once; this bound only
/// exists so that a server which *serves* the connection instead fails this arm with a sentence
/// rather than blocking a reader that has nothing left to wait for.
const ANSWER_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

/// A latch the test opens once, from the client's thread, after its framed write has returned.
#[derive(Debug, Default)]
struct Latch {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Latch {
    fn open(&self) {
        *self.open.lock().expect("the latch") = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().expect("the latch");
        while !*open {
            open = self.changed.wait(open).expect("the latch");
        }
    }
}

/// The scripted fake, held open on its refusing answer until the test lets it through.
///
/// It delegates the answer and the per-connection record to [`FakeResolver`] rather than keeping
/// its own, so `asked()` here is the shipped fake's count and not a second thing that could
/// disagree with it. All this type adds is the ordering.
#[derive(Debug)]
struct GatedResolver {
    inner: FakeResolver,
    latch: Arc<Latch>,
}

impl GatedResolver {
    fn new(answer: PeerResolution, latch: Arc<Latch>) -> Self {
        Self {
            inner: FakeResolver::answering(answer),
            latch,
        }
    }

    fn set(&self, answer: PeerResolution) {
        self.inner.set(answer);
    }

    fn asked(&self) -> Vec<PeerIdentity> {
        self.inner.asked()
    }
}

impl PeerResolver for GatedResolver {
    fn resolve(&self, peer: &PeerIdentity) -> PeerResolution {
        let answer = self.inner.resolve(peer);
        if answer == PeerResolution::Refuse {
            self.latch.wait();
        }
        answer
    }
}

/// A request written on a connection whose resolution refused never reaches the registered
/// handler, while the identical request on the next connection does.
///
/// The production mutant this exists for is in `src/server.rs`: let the `Refuse` arm fall through
/// into `ConditionedConnectionContext::from_accepted_peer` instead of returning from the
/// connection's closure. The resolver still answers `Refuse`, the connection is served anyway, and
/// the count reads `1`.
#[test]
fn a_buffered_request_from_a_refused_connection_never_reaches_the_dispatcher() {
    let dir = TempDir::new().unwrap();
    let executions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&executions);

    let mut dispatcher = Dispatcher::new(AdmitAll);
    dispatcher.register(Method::SystemDoctor, Requirement::AnyRole, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(json!({ "canned": CANNED }))
    });

    let latch = Arc::new(Latch::default());
    let resolver = Arc::new(GatedResolver::new(
        PeerResolution::Refuse,
        Arc::clone(&latch),
    ));
    let server = TestServer::start_with_resolver(
        dir.path().join("run"),
        dispatcher,
        Arc::clone(&resolver) as Arc<dyn PeerResolver>,
    );

    // Arm: write a whole request frame, and only then let the refusal proceed.
    let mut stream = UnixStream::connect(server.socket()).expect("connect");
    let request = Request {
        jsonrpc: "2.0".to_string(),
        id: RequestId::Number(1),
        method: Method::SystemDoctor.name().to_string(),
        params: None,
        v: None,
        cred: None,
    };
    write_frame(&mut stream, &request).expect("the request frame is written");
    stream
        .set_read_timeout(Some(ANSWER_BOUND))
        .expect("a read deadline");
    latch.open();

    // The server closing the connection is the sync point: the refusal path has run to its end.
    //
    // **Bounded, and `read_to_end` is wrong here.** A served connection stays open for the next
    // request, so a read with no deadline never returns against the very mutant this arm exists to
    // catch — the arm would hang the harness instead of failing on a sentence. Measured: with
    // `read_to_end` the mutant ran past ten minutes. One bounded read distinguishes all three
    // outcomes by hand: the connection ending is the refusal, `Ok(n)` is a served connection
    // answering, and a timeout is neither.
    //
    // **The connection ending has two spellings, and the kernel picks which.** Closing a stream
    // socket that still holds an unread request queues a reset for the peer on Linux, where macOS
    // delivers a clean end-of-file for the identical sequence. Measured directly — write a frame,
    // close the far end without reading it, then read: Darwin answers `Ok(0)`, and the Ubuntu leg
    // answered `Err(ConnectionReset)`, os error 104, at this line. Accepting only `Ok(0)` made this
    // a platform assertion rather than a refusal assertion.
    //
    // The reset is the *stronger* evidence for the proposition, not a weaker one: it is the kernel
    // reporting that the server dropped the connection with this request still unread. Neither
    // spelling loosens the mutant — a served connection reads the frame and writes an answer, which
    // lands in `Ok(n)` below — and a genuine hang is `WouldBlock`/`TimedOut`, which still falls to
    // the last arm.
    let mut first = [0_u8; 512];
    match stream.read(&mut first) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ) => {}
        Ok(n) => panic!(
            "a refused connection answered {n} bytes: {:?}",
            String::from_utf8_lossy(&first[..n])
        ),
        Err(error) => {
            panic!("the connection neither answered nor ended within {ANSWER_BOUND:?}: {error}")
        }
    }
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "the buffered request reached the dispatcher of a connection whose provenance never \
         resolved"
    );

    // Control: same server, same handler, same socket — the next resolution answers a column.
    resolver.set(PeerResolution::Resolved(PeerProvenance::NoRecordedAgent));
    let answered = server
        .client()
        .call(Method::SystemDoctor, None)
        .expect("an answer");
    assert_eq!(
        answered.result.expect("a result")["canned"],
        CANNED,
        "control: the identical call on a resolved connection reaches the identical handler"
    );
    assert_eq!(
        executions.load(Ordering::SeqCst),
        1,
        "control: exactly the control's call ran the handler"
    );
    assert_eq!(
        resolver.asked().len(),
        2,
        "control: both connections were resolved, one per connection"
    );
}

/// The standalone control for the mutation run, and it is standalone for a reason.
///
/// The arm above carries its own control on the *same* server, which is the stronger
/// discriminator — same dispatcher, same socket, one answer changed. But it runs *after* the
/// count assertion, so under the production mutant that assertion fails and the control never
/// executes: the mutation run would then have a subject that reddened and no live control at all.
/// This one runs whatever the subject does.
#[test]
fn a_resolved_connection_reaches_the_dispatcher() {
    let dir = TempDir::new().unwrap();
    let executions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&executions);

    let mut dispatcher = Dispatcher::new(AdmitAll);
    dispatcher.register(Method::SystemDoctor, Requirement::AnyRole, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(json!({ "canned": CANNED }))
    });

    let resolver = Arc::new(GatedResolver::new(
        PeerResolution::Resolved(PeerProvenance::NoRecordedAgent),
        Arc::new(Latch::default()),
    ));
    let server = TestServer::start_with_resolver(
        dir.path().join("run"),
        dispatcher,
        Arc::clone(&resolver) as Arc<dyn PeerResolver>,
    );

    let answered = server
        .client()
        .call(Method::SystemDoctor, None)
        .expect("an answer");
    assert_eq!(answered.result.expect("a result")["canned"], CANNED);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        resolver.asked().len(),
        1,
        "one resolution per connection, and this fixture opened one"
    );
}
