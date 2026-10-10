//! The accept loop: synchronous, a thread per connection, generic over the transport.
//!
//! [`serve`] never names a socket type. It takes a [`Listener`] that yields a [`Connection`] and
//! whatever the transport could resolve about that peer, so the loop, the codec, the dispatcher and
//! every handler are written once. [`LocalTransport`] selects a Unix socket or a Windows named
//! pipe behind that pair: invariant 10 forbids a listener outside the machine, and neither arm
//! binds a network address.
//!
//! A thread per connection rather than an async task, because invariant 9 forbids this crate from
//! linking a runtime at all. The control plane is a handful of connections — an operator CLI, a
//! shim that sends one request, a floor — so the thread is the cheaper thing to reason about as
//! well as the only one available. "A handful" is a **cap [`serve`] requires its caller to state**,
//! not an assumption: a thread per connection with no ceiling is a peer's decision about how many
//! threads the daemon runs.
//!
//! Every accepted connection is registered with the [`StopFlag`] before its thread starts, and
//! `stop()` halts what is registered. That is what makes shutdown bounded: a peer parked in a
//! blocking read cannot observe a flag, and `thread::scope` joins it before [`serve`] returns.

use std::fmt;
use std::io::{BufRead, Write};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::{ErrorCode, RequestId, Response};

#[cfg(any(windows, test))]
use crate::auth::PeerPid;
use crate::auth::{PeerIdentity, PeerProvenance, PeerResolution, PeerResolver};
use crate::codec::{FrameError, read_frame};
use crate::conditioned::{ConditionedConnectionContext, OperatorAuthoritySource};
use crate::dispatch::{Dispatcher, UNIDENTIFIED_REQUEST_ID, error};
use crate::outbound::{Outbound, SendError};

/// A byte stream this transport can serve.
///
/// Split into a reader and a writer up front rather than serving through one `Read + Write` handle:
/// the reader wants buffering and the writer must not be buffered past a flush, and a transport
/// that cannot hand out both halves (an in-memory pipe, a pair of pipes) can implement this while
/// it could not implement `Read + Write` on one value.
pub trait Connection: Send + 'static {
    /// The buffered read half.
    type Reader: BufRead;
    /// The write half.
    ///
    /// `Send + 'static` because a subscription outlives the call that started it: the write half is
    /// shared into [`Outbound`], which a registry on another thread writes to. A borrowed writer
    /// could not be handed out that way, so the bound is what makes any notification possible at
    /// all rather than a convenience.
    type Writer: Write + Send + 'static;

    /// Split into the two halves.
    ///
    /// # Errors
    ///
    /// Whatever duplicating the underlying handle costs.
    fn split(self) -> std::io::Result<(Self::Reader, Self::Writer)>;

    /// A handle that can end this connection from another thread.
    ///
    /// Taken by `&self`, before [`Connection::split`] consumes the value, so a connection can be
    /// registered with its owner before anything starts reading it.
    ///
    /// # Errors
    ///
    /// Whatever duplicating the underlying handle costs. A connection whose handle cannot be
    /// obtained is refused rather than served — serving it would put a thread inside a blocking
    /// read that nothing could wake, which is what [`Halt`] exists to prevent.
    fn halt_handle(&self) -> std::io::Result<Box<dyn Halt>>;

    /// [`Connection::split`], with a probe that says whether the peer has gone.
    ///
    /// A handler that waits on long work before it appends, such as a goal's check run, asks it
    /// first, so a peer that closed or shut down its write side gets no effect it can no longer
    /// read. The probe costs nothing until it is asked. The default never reports a hang-up.
    ///
    /// # Errors
    ///
    /// As [`Connection::split`].
    fn split_with_hang_up(self) -> std::io::Result<(Self::Reader, Self::Writer, HangUp)>
    where
        Self: Sized,
    {
        let (reader, writer) = self.split()?;
        Ok((reader, writer, HangUp::never()))
    }
}

/// Whether the peer of one connection has gone: closed the connection, or shut down its write
/// side. Cheap to clone and safe to ask from any thread.
#[derive(Clone)]
pub struct HangUp(Arc<dyn Fn() -> bool + Send + Sync>);

impl HangUp {
    /// A probe that asks `gone`.
    pub fn new(gone: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(gone))
    }

    /// A probe for a transport that cannot tell, which never reports a hang-up.
    #[must_use]
    pub fn never() -> Self {
        Self::new(|| false)
    }

    /// Whether the peer has gone.
    #[must_use]
    pub fn hung_up(&self) -> bool {
        (self.0)()
    }
}

impl fmt::Debug for HangUp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HangUp")
    }
}

/// A handle that ends one live connection, from a thread that is not serving it.
///
/// At teardown two different threads can be parked on a single connection and **neither can observe
/// a flag**: the connection's own thread inside a blocking read, and — when that connection holds a
/// `journal.tail` subscription — whoever is fanning a notification out to it, inside a blocking
/// write. A read timeout would only address the first, and only by waking a thread forever to learn
/// nothing; ending the connection underneath both is one call and costs an idle peer nothing while
/// the daemon runs.
///
/// It is deliberately not `close`. The connection's halves are owned elsewhere by then — the reader
/// by its thread, the writer by an [`Outbound`] that may be shared into the subscriber registry —
/// so nothing here can take them. Halting acts on the shared endpoint and leaves that ownership
/// alone.
pub trait Halt: Send + Sync + 'static {
    /// End the connection.
    ///
    /// Idempotent, and infallible from the caller's side: a connection that has already gone is
    /// already in the state this call was asking for.
    fn halt(&self);
}

/// A transport that accepts connections and reports what it resolved about each peer.
///
/// The peer identity is produced *by the transport*, because only the transport knows what its
/// platform can answer. A transport with no local process behind it returns
/// [`PeerIdentity::unknown`], and that is a representable answer rather than a panic — which is the
/// whole reason [`PeerIdentity`]'s fields are `Option`.
pub trait Listener {
    /// What this listener accepts.
    type Conn: Connection;

    /// Block until a peer connects.
    ///
    /// # Errors
    ///
    /// Whatever the underlying accept returns.
    fn accept(&self) -> std::io::Result<(Self::Conn, PeerIdentity)>;
}

/// The stop signal for one [`serve`] loop, and the live connections that signal has to reach.
///
/// Two separate things keep [`serve`] running, and a flag on its own addresses only the first:
///
/// 1. **The blocking accept** does not observe the flag until the next connection arrives.
///    Connecting to the socket once wakes it — `testing::TestServer::shutdown` does that. Not a
///    link: that module is behind the `testing` feature, so a link here would resolve in one build
///    configuration and dangle in the default one.
/// 2. **The joined connection threads.** [`serve`] runs its threads in a `thread::scope`, which
///    joins every one of them before returning, and a peer that connects and never sends a byte
///    parks its thread in a blocking read. **A flag cannot be observed from inside that read**, so
///    a flag alone leaves `serve` running for as long as any peer cares to hold a connection —
///    it stays running after `stop()` and is released the instant the peer was dropped.
///
/// So every accepted connection is registered here, with a [`Halt`] handle, before its thread is
/// spawned; `stop()` sets the flag *and* halts every connection registered, and each thread then
/// returns from a read that has already ended. `stop()` therefore means "return", not merely
/// "accept nothing further".
///
/// **The flag and the registry are under one lock, and that is the whole of why this is not
/// flaky.** With the flag outside it, a `stop()` landing between the accept and the registration
/// would leave that connection unhalted and `serve` parked exactly as it was before — but only
/// sometimes, which is worse than reliably, because it passes CI. Registration happens on the
/// accept thread, takes the same lock `stop()` takes, and a registration that finds the flag
/// already set halts its connection instead of registering it.
///
/// The registry is also the connection count, which is what [`serve`]'s cap is checked against:
/// one structure, because "how many connections are live" and "which connections must be halted"
/// are the same question asked twice.
pub struct StopFlag {
    inner: Mutex<Live>,
}

/// The stop flag and the live connections. One lock, for the reason [`StopFlag`] gives.
struct Live {
    stopped: bool,
    /// The next slot number to hand out.
    ///
    /// Monotonic rather than an index into `connections`: a slot freed by one connection must not
    /// be releasable a second time by a later connection that happened to take its place in the
    /// vector.
    next_slot: u64,
    connections: Vec<(u64, Box<dyn Halt>)>,
}

/// What [`StopFlag::admit`] decided about a connection that has just been accepted. In every
/// refusing arm the connection has already been halted.
enum Admission<'a> {
    /// Serve it; the slot is freed when the returned guard drops.
    Admitted(Registration<'a>),
    /// The loop is stopping, so the loop ends.
    Stopping,
    /// The cap is full, so the loop continues with the next peer.
    OverCap,
}

/// One live connection's slot, released when the thread serving it ends.
///
/// A guard rather than a call at the end of the handler, so a handler that panics frees its slot
/// too: a cap that leaked a slot per panic would refuse every later caller, and would look from
/// outside exactly like a daemon that had stopped answering.
struct Registration<'a> {
    flag: &'a StopFlag,
    slot: u64,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.flag.release(self.slot);
    }
}

impl fmt::Debug for StopFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StopFlag")
            .field("stopped", &self.is_stopped())
            .field("live_connections", &self.live_connections())
            .finish()
    }
}

impl Default for StopFlag {
    fn default() -> Self {
        Self::new()
    }
}

impl StopFlag {
    /// A flag that is not set, with nothing registered.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(Live {
                stopped: false,
                next_slot: 0,
                connections: Vec::new(),
            }),
        }
    }

    /// Ask [`serve`] to stop, and end every connection it is currently serving.
    ///
    /// A peer mid-request loses that request. That is what teardown is, and it is the price of the
    /// guarantee: nothing is ever disconnected by this crate while the daemon is running.
    pub fn stop(&self) {
        let mut live = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        live.stopped = true;
        for (_, handle) in &live.connections {
            handle.halt();
        }
    }

    /// Whether [`StopFlag::stop`] has been called.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stopped
    }

    /// How many connections are being served. For diagnostics, and the number [`serve`]'s cap is
    /// checked against.
    #[must_use]
    pub fn live_connections(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .connections
            .len()
    }

    /// Take a slot for a connection about to be served, or refuse it.
    ///
    /// The flag check, the cap check and the registration are one critical section. Splitting them
    /// would reintroduce the race this type exists to close.
    fn admit(&self, handle: Box<dyn Halt>, cap: NonZeroUsize) -> Admission<'_> {
        let mut live = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if live.stopped {
            handle.halt();
            return Admission::Stopping;
        }
        if live.connections.len() >= cap.get() {
            handle.halt();
            return Admission::OverCap;
        }
        let slot = live.next_slot;
        live.next_slot += 1;
        live.connections.push((slot, handle));
        Admission::Admitted(Registration { flag: self, slot })
    }

    /// Free `slot`. Called by [`Registration`]'s drop and by nothing else.
    fn release(&self, slot: u64) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .connections
            .retain(|(registered, _)| *registered != slot);
    }
}

/// Accept connections until `stop` is set or the listener fails, serving each on its own thread,
/// and never more than `max_connections` at a time.
///
/// Returns when the loop ends *and* every connection thread has finished: the scope is what keeps a
/// handler from outliving the dispatcher it borrows. Because every connection is registered with
/// `stop` before its thread starts, and `stop()` halts what is registered, "every thread has
/// finished" is bounded rather than at the peers' discretion — see [`StopFlag`].
///
/// # The cap is an argument and has no default
///
/// A daemon that ships an unbounded default ships the defect: one OS thread per connection, inside
/// a scope that cannot return until every one of them ends. Requiring the number here means no
/// build can inherit "unlimited" by omission, and [`NonZeroUsize`] means no caller can pass a cap
/// of zero and serve nobody.
///
/// **The number itself is the caller's**, and deliberately not this crate's: a control plane's
/// working set depends on how many agents it supervises, since an agent's hook shim opens one
/// connection per call, and a cap set below that is a self-inflicted refusal of the daemon's own
/// children.
///
/// **What the cap buys is bounded work at teardown, not availability.** At most `max_connections`
/// connection threads are live at once, so `stop` has a bounded set to halt and this scope a
/// bounded set to join — that is the whole of the guarantee. Capacity is first-come and a slot
/// belongs to whoever took it until that connection ends; the section below is what that leaves
/// open.
///
/// A connection over the cap is **accepted, halted and dropped**: it ends without an answer, and
/// no thread is spawned for it. Not an error frame — the contract's `ErrorCode` is an exhaustive
/// public enum with no code meaning "too many connections", so answering with one would be a
/// wire-format change, and writing a refusal on the accept thread is precisely the blocking write
/// this loop must not do.
///
/// **What that end looks like to the peer depends on where it had got to**, and a caller should not
/// expect a clean end of stream. A peer that is reading sees one; a peer whose request is already
/// sitting unread in the daemon's receive queue is reset instead, and reads `ENOTCONN` or writes
/// into `EPIPE`. Measured on macOS, where a peer that had not yet finished setting up its own
/// socket also saw `setsockopt` fail. All of them are the same event: the connection ended and
/// nothing was written to it.
///
/// `dudes-hook` sees this as `ClientError::Closed`, or as a frame I/O error of kind `BrokenPipe` or
/// `NotConnected` (`ConnectionReset` and `UnexpectedEof` are treated the same way). For a
/// `PreToolUse` or `UserPromptSubmit` event it exits 2 on any of them, so the cap cannot be used to
/// let a paused agent's next step through. A refused connect, which is what a stopped daemon gives,
/// stays fail-open.
///
/// # A slot is charged before its peer has proved anything
///
/// Admission happens here, on the accept thread; authorisation happens per call inside
/// `serve_connection`, after the slot has been taken. So a peer whose every call would be refused
/// holds a slot for as long as it cares to, and `max_connections` peers that connect and send
/// nothing are enough to refuse the daemon's own hook shims — by the same silent end every
/// over-cap refusal uses. A shim tells that end from a daemon that has died, because the connect
/// itself succeeded, and blocks its control step. There is no idle
/// reclamation, no reserved capacity and no per-peer accounting.
///
/// This sits under the ceiling invariant 8 already states and does not raise — same uid is not a
/// boundary. Denying service by exhausting the daemon's descriptor or thread limits is replaced
/// by opening exactly `max_connections` connections and sleeping. The [`PeerIdentity`] a
/// per-peer limit would key on is already resolved before `admit` is called.
///
/// # Errors
///
/// Whatever [`Listener::accept`] returns. A connection that fails mid-stream does not end the loop
/// — one peer hanging up is not a reason to stop answering the others.
///
/// **The bound above is on the `stop` path and not on this one.** An accept that fails ends the
/// loop without anything having halted the live connections, and the scope then joins them exactly
/// as it always did: a daemon that ran out of descriptors would return an error here and park. It
/// is the same defect through a different door, it is not what this function was asked to fix, and
/// it is recorded rather than quietly widened — halting here would be a behaviour no test in this
/// repository drives. `scope.spawn` is the same door again: it **panics** when the OS refuses the
/// thread, the panic unwinds out of this closure, and the scope joins the live connections with
/// nothing having halted them — much less reachable now that the cap bounds how many threads this
/// loop asks for, which is part of what the cap is for.
///
/// **Both of those doors are still open in this function, and one of them is now closed around
/// it.** What is unchanged: an accept that fails still ends the loop without halting anything, and
/// a `scope.spawn` the OS refuses still unwinds out of this closure — an embedder of `serve` gets
/// exactly what the two paragraphs above describe. What a caller can add is a frame that catches
/// that unwind and decides what it means, which is the same division of labour the resolver
/// heading below sets out; the daemon in this workspace does that too, stopping for a named cause
/// rather than running on with a listener its accept thread took down with it.
///
/// # A resolver that panics leaves its own connection unserved, and nothing else
///
/// `resolver` is asked on the connection's thread, so a panic inside it unwinds that thread alone.
/// **That connection is not served**: the panic is raised before the conditioned context exists
/// and before a byte of a request is read, so no call is ever dispatched against it and no peer is
/// handed a provenance that was not resolved. Scoped threads are joined at the end of the scope,
/// not when they die, so the panic is **not** observed here until this loop stops for some other
/// reason — until then the door stays open and every subsequent connection ends the same way.
///
/// That is the right containment for this function, which cannot know what a caller wants a broken
/// resolver to mean. A caller for whom a resolver that can no longer answer is a fault of the whole
/// program should wrap its resolver in one that catches the panic and acts, rather than expect
/// `serve` to decide; the daemon in this workspace does exactly that — its wrapper stops the daemon
/// for a named cause and re-raises, so the connection that broke the resolver and every later one
/// are left unserved until the door closes.
///
/// **A wrapper that returns a provenance after catching is fabricating one**, which is a peer
/// admitted on evidence that does not exist. There is no column here that means *"nothing was
/// resolved"*: an embedder that wants a resolver failure to be survivable has to express that in
/// its own resolver, not in what it hands back to this loop.
pub fn serve<L>(
    listener: &L,
    resolver: &dyn PeerResolver,
    dispatcher: &Dispatcher,
    stop: &StopFlag,
    max_connections: NonZeroUsize,
) -> std::io::Result<()>
where
    L: Listener + Sync,
{
    serve_inner(listener, resolver, dispatcher, None, stop, max_connections)
}

/// Serve with the daemon's independently owned operator-effect authority source.
///
/// The source is consulted once per accepted connection, on that connection's own thread, after
/// provenance has been independently resolved there and before any request on it is read — so a
/// connection racing `seal()` may find the epoch closed where the accept thread would have found it
/// open: one connection fewer with authority, never one more. [`serve`] remains the authority-free
/// entry point for callers that do not own that daemon lifecycle state.
///
/// # Errors
///
/// As [`serve`].
pub fn serve_with_authority<L>(
    listener: &L,
    resolver: &dyn PeerResolver,
    dispatcher: &Dispatcher,
    authority: &OperatorAuthoritySource,
    stop: &StopFlag,
    max_connections: NonZeroUsize,
) -> std::io::Result<()>
where
    L: Listener + Sync,
{
    serve_inner(
        listener,
        resolver,
        dispatcher,
        Some(authority),
        stop,
        max_connections,
    )
}

fn serve_inner<L>(
    listener: &L,
    resolver: &dyn PeerResolver,
    dispatcher: &Dispatcher,
    authority: Option<&OperatorAuthoritySource>,
    stop: &StopFlag,
    max_connections: NonZeroUsize,
) -> std::io::Result<()>
where
    L: Listener + Sync,
{
    thread::scope(|scope| {
        while !stop.is_stopped() {
            let (conn, peer) = listener.accept()?;
            let handle = match conn.halt_handle() {
                Ok(handle) => handle,
                Err(err) => {
                    // Refused rather than served without one: a connection nothing can halt is a
                    // thread nothing can wake at teardown.
                    tracing::debug!(error = %err, "a connection with no halt handle is refused");
                    continue;
                }
            };
            let registration = match stop.admit(handle, max_connections) {
                Admission::Admitted(registration) => registration,
                // The connection that woke a stopping loop is not served: an owner that set the
                // flag and then connected to unblock the accept is asking to stop, not to be
                // answered.
                Admission::Stopping => break,
                Admission::OverCap => {
                    tracing::debug!(
                        cap = max_connections.get(),
                        "a connection over the cap is refused"
                    );
                    continue;
                }
            };
            // Everything the accept thread owed this connection is done: it is admitted, its slot
            // is charged, and it has a thread of its own. What is left belongs to that thread.
            scope.spawn(move || {
                // **Once per connection, here, and this is the whole of peer provenance.** The resolver
                // is asked in the connection's own frame, after the slot is charged and before a
                // single byte of a request is read, so the transport-observed peer and its
                // *resolved provenance* are fixed together for that connection and no request can
                // be dispatched against a provenance that does not exist yet.
                //
                // **On this thread rather than the accept thread, and that is the point.** A
                // resolver that reads a process table takes real time, and every statement the
                // accept loop executes between two accepts is a statement every waiting peer is
                // queued behind — so a walk left up there would make the door deny service to
                // itself under load. Resolution is per-connection work; it runs where the rest of
                // this connection's work runs.
                //
                // What that does **not** relax: the conditioned context still retains only the
                // transport-built peer actor material the router's persistence seam needs, and
                // cannot reach general `Call` or an effect sink, so per-call re-resolution stays
                // unwritable from behind `Authorizer` rather than merely forbidden. What it costs
                // is stated rather than hidden: `resolver` is now in scope in this frame, so it is
                // an edit in *this* closure — not one behind the seam — that could ask twice.
                //
                // **A resolution that never answered ends the connection here**, and "here" is
                // load-bearing: this is the first statement of the closure, so an early return
                // from it builds no `ConditionedConnectionContext`, reads no line, writes no
                // response byte and asks no `Authorizer`. The registration is owned by this
                // closure, so the slot is freed by the drop on the way out — there is no second
                // `drop(registration)` to add.
                let PeerResolution::Resolved(provenance) = resolver.resolve(&peer) else {
                    // `warn`, and worded apart from the two refusals above it: those are a door
                    // doing its job under load, this is a peer the daemon could not place.
                    tracing::warn!(
                        "a connection whose peer provenance never resolved is refused before any \
                         request context exists"
                    );
                    return;
                };
                // A lease is acquired for this accepted connection, not for a request. Provenance
                // is an independently resolved deny/corroboration input; the source owns the
                // pre-agent epoch and is the only thing that can construct authority. Acquiring it
                // here means a connection racing `seal()` may now find the epoch closed where the
                // accept thread would have found it open — one connection fewer with authority,
                // never one more, and the connection whose own call does the sealing is unaffected
                // because its acquisition precedes its first request either way.
                let context =
                    ConditionedConnectionContext::from_accepted_peer(peer, provenance, authority);
                if let Err(err) = serve_connection_on_context(conn, context, dispatcher) {
                    tracing::debug!(error = %err, "connection ended");
                }
                // The slot is freed here on the clean path and by unwinding on the panicking one.
                drop(registration);
            });
        }
        Ok(())
    })
}

/// Serve one connection until the peer closes it.
///
/// # Errors
///
/// [`FrameError`] from the underlying stream. The two non-I/O framing failures are handled
/// differently on purpose, and the difference is whether the stream is still in sync:
///
/// - **Not UTF-8** — the line was consumed to its newline, so the peer is answered a parse error
///   and the connection continues.
/// - **Over the cap** — the peer is answered and then the connection ends, because the rest of an
///   oversized line cannot be skipped without reading exactly the bytes the cap exists to refuse.
///
/// A *response* over the cap is not either of those and does not end the connection: the writer
/// refuses it with nothing written, so the stream is in sync and [`send`] answers an error under
/// the same id instead.
pub fn serve_connection<C: Connection>(
    conn: C,
    peer: PeerIdentity,
    provenance: PeerProvenance,
    dispatcher: &Dispatcher,
) -> Result<(), FrameError> {
    let context = ConditionedConnectionContext::from_peer(peer, provenance);
    serve_connection_on_context(conn, context, dispatcher)
}

/// Declared after `Outbound`, so unwind drops this first and revokes before its writer drops.
struct RevokeBeforeOutbound<'a>(&'a ConditionedConnectionContext);

impl Drop for RevokeBeforeOutbound<'_> {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

fn serve_connection_on_context<C: Connection>(
    conn: C,
    context: ConditionedConnectionContext,
    dispatcher: &Dispatcher,
) -> Result<(), FrameError> {
    let (reader, writer, hang_up) = conn.split_with_hang_up()?;
    let context = context.with_hang_up(hang_up);
    let outbound = Arc::new(Outbound::new(writer));
    let revoke_before_outbound = RevokeBeforeOutbound(&context);
    let result = serve_stream_on_connection(reader, &outbound, &context, dispatcher);
    // The reader loop has ended, so this peer is gone whether it hung up or the stream failed.
    // Revoke before closing the write half: authority cannot outlive the input half even if
    // outbound teardown blocks. `ConditionedConnectionContext::drop` is the unwind backstop.
    context.revoke();
    // Closing here gives a subscription a deterministic end. Waiting for a notification to fail
    // would leave the subscriber registered until the next append, which may never come.
    outbound.close();
    drop(revoke_before_outbound);
    drop(context);
    result
}

/// Serve one already-split stream. The transport-free core of [`serve_connection`].
///
/// Responses are written through the same [`Outbound`] a handler's notifications use, so a
/// notification from another thread cannot land inside a response line. It also means a
/// subscription's first notifications may reach the peer *before* the response to the call that
/// started it — a subscribing client reads frames rather than answers, which is what
/// [`crate::Client::recv`] is for.
///
/// # Errors
///
/// As [`serve_connection`].
pub fn serve_stream(
    reader: impl BufRead,
    outbound: &Arc<Outbound>,
    peer: PeerIdentity,
    provenance: PeerProvenance,
    dispatcher: &Dispatcher,
) -> Result<(), FrameError> {
    let context = ConditionedConnectionContext::from_peer(peer, provenance);
    serve_stream_on_connection(reader, outbound, &context, dispatcher)
}

fn serve_stream_on_connection(
    mut reader: impl BufRead,
    outbound: &Arc<Outbound>,
    context: &ConditionedConnectionContext,
    dispatcher: &Dispatcher,
) -> Result<(), FrameError> {
    loop {
        match read_frame(&mut reader) {
            Ok(None) => return Ok(()),
            Ok(Some(line)) => {
                let outcome = dispatcher
                    .handle_line_with_after_response_on_connection(&line, context, outbound);
                let send_result = send(outbound, outcome.response_ref());
                outcome.run_after_response();
                send_result?;
            }
            Err(FrameError::NotUtf8) => {
                // The line was consumed to its newline, so the stream is still in sync and the
                // next frame starts where it should: answer and keep going. Ending the connection
                // here would make one stray byte cost every call pipelined behind it — the same
                // silent-hang failure the parse-error path exists to avoid.
                let response = Response::err(
                    RequestId::Number(UNIDENTIFIED_REQUEST_ID),
                    error(ErrorCode::ParseError, "the frame was not valid UTF-8"),
                );
                send(outbound, &response)?;
            }
            Err(FrameError::TooLarge) => {
                let response = Response::err(
                    RequestId::Number(UNIDENTIFIED_REQUEST_ID),
                    error(ErrorCode::ParseError, "the frame exceeded the 1 MiB cap"),
                );
                send(outbound, &response)?;
                return Err(FrameError::TooLarge);
            }
            Err(err) => return Err(err),
        }
    }
}

/// Write one response, substituting an error for a result this transport cannot carry.
///
/// A response over the frame cap is refused by the writer with nothing written, so the stream is
/// still in sync and this peer is owed an answer rather than a disconnection — and a JSON-RPC
/// error *is* the answer to a call whose result does not fit. It goes back under the same request
/// id, so a client waiting on that id is answered rather than left waiting.
///
/// **The peer receives a *different value* here than the one the handler produced**, where
/// otherwise it would receive a dead connection. The substitution happens inside this function, so
/// callers handling `FrameError` still describes the types and misses the wire.
///
/// **The substitute is not immune to the cap, only smaller than what it replaces.** Its message is
/// constant but its id is not: `RequestId::String` is peer-supplied, so a peer that sends a ~1 MiB
/// id gets a substitute over the cap as well, and this function returns `TooLarge` having written
/// nothing. Measured: an id of 1 048 458 bytes arrives in a 1 048 507-byte frame, which `read_frame`
/// accepts, and yields a 1 048 577-byte substitute against the 1 048 576 cap — one byte over — so
/// `serve_stream` ends the connection with **0 bytes** delivered, where the same call under a short
/// id is answered normally. That peer gets no answer and no connection, which is the failure this
/// function exists to prevent. It is reachable only by the peer that supplies the id, so it is
/// self-inflicted — sloppy rather than exploitable, and not bounded here, because bounding the id
/// is a contract decision and not this function's.
///
/// The retry is bounded at one regardless, and that is what needs no guard: the substitution is
/// attempted once, from the `Err` arm, and its own failure is returned rather than re-substituted.
fn send(outbound: &Arc<Outbound>, response: &Response) -> Result<(), FrameError> {
    match write(outbound, response) {
        Err(FrameError::TooLarge) => write(
            outbound,
            &Response::err(
                response.id.clone(),
                error(
                    ErrorCode::InternalError,
                    "the result exceeded the 1 MiB frame cap and was not sent",
                ),
            ),
        ),
        other => other,
    }
}

/// Write one frame, reporting a closed connection as the end of the stream it is.
fn write(outbound: &Arc<Outbound>, response: &Response) -> Result<(), FrameError> {
    match outbound.send(response) {
        Ok(()) => Ok(()),
        Err(SendError::Frame(err)) => Err(err),
        Err(SendError::Closed) => Err(FrameError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "the connection was closed",
        ))),
    }
}

/// The name the operator credential takes inside the runtime directory.
///
/// A constant rather than an argument: the file is a fixed part of a daemon's runtime directory,
/// and a caller free to name it is a caller free to name it something outside the directory the
/// binding code verified.
///
/// **Unconditional, while the modes beside it are not**. The *name* is a fact
/// about the layout of a runtime directory, which every platform has and which the CLI reads to
/// find the credential it presents; the *permissions* are a fact about how one platform restricts a
/// file, and `socket_permissions`, `runtime_dir_permissions` and `operator_credential_permissions`
/// stay `#[cfg(unix)]` for exactly that reason. Gating the name too is what made
/// `bins/dudes/src/credential.rs` and `bins/duded/src/socket.rs` fail to resolve for
/// `x86_64-pc-windows-msvc`, on a line that had nothing to say about a mode.
pub const OPERATOR_CREDENTIAL_NAME: &str = "operator.cred";

/// The operator credential could not be created, written or synced.
///
/// The constructors that write the credential return this inside their [`std::io::Error`], with the
/// original error's kind. It is a type of its own so a caller can tell this failure from the
/// socket's beside it and let it lead its own message.
#[derive(Debug, thiserror::Error)]
#[error("creating the operator credential at {}: {error}", path.display())]
pub struct CredentialCreationError {
    path: std::path::PathBuf,
    error: std::io::Error,
}

impl CredentialCreationError {
    fn wrap(path: &std::path::Path, error: std::io::Error) -> std::io::Error {
        std::io::Error::new(
            error.kind(),
            Self {
                path: path.to_owned(),
                error,
            },
        )
    }
}

/// Create the operator credential once, write its exact text, and sync it before the transport is
/// served.
///
/// Unix supplies mode `0600` to `open(2)`. Windows supplies the current `TokenUser` SID as both
/// owner and sole DACL trustee to `CreateFileW`, so there is no create-then-set window. Both
/// platform transports use this one writer.
#[derive(Debug)]
struct WrittenCredential {
    path: std::path::PathBuf,
    #[cfg(windows)]
    lease: std::fs::File,
}

#[cfg(not(windows))]
fn write_operator_credential(
    dir: &std::path::Path,
    credential: &OperatorCredential,
) -> std::io::Result<WrittenCredential> {
    let path = dir.join(OPERATOR_CREDENTIAL_NAME);
    let create = || {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(unix::OPERATOR_CREDENTIAL_PERMISSIONS);
        }
        let mut file = options.open(&path)?;
        file.write_all(credential.expose().as_bytes())?;
        file.sync_all()
    };
    create().map_err(|error| CredentialCreationError::wrap(&path, error))?;
    Ok(WrittenCredential { path })
}

#[cfg(windows)]
fn write_operator_credential(
    dir: &std::path::Path,
    credential: &OperatorCredential,
) -> std::io::Result<WrittenCredential> {
    write_operator_credential_with(
        dir,
        credential,
        std::io::Write::write_all,
        std::fs::File::sync_all,
    )
    .map_err(|error| CredentialCreationError::wrap(&dir.join(OPERATOR_CREDENTIAL_NAME), error))
}

#[cfg(windows)]
fn write_operator_credential_with(
    dir: &std::path::Path,
    credential: &OperatorCredential,
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    sync: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<WrittenCredential> {
    let credential_path = dir.join(OPERATOR_CREDENTIAL_NAME);
    let mut file = dudes_win::create_file_creator_only(&credential_path)?;
    write(&mut file, credential.expose().as_bytes())?;
    sync(&file)?;
    Ok(WrittenCredential {
        path: credential_path,
        lease: file,
    })
}

/// Build the exact creator-only pipe descriptor the Windows security model requires.
///
/// Kept pure so the owner/trustee equality has a portable mutation control even though the
/// descriptor itself can only be deserialized on Windows.
#[cfg(any(windows, test))]
fn pipe_security_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;;GA;;;{sid})")
}

/// Translate the safe named-pipe pid read into the transport identity.
///
/// A read error is `Unreadable`: a named pipe has a process behind it, and failing to learn which
/// process is the least privileged answer.
#[cfg(any(windows, test))]
fn windows_peer_identity(result: std::io::Result<u32>) -> PeerIdentity {
    match result.and_then(|pid| {
        i32::try_from(pid).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the named-pipe client pid does not fit the identity contract",
            )
        })
    }) {
        Ok(pid) => PeerIdentity {
            uid: None,
            gid: None,
            pid: PeerPid::Reported {
                pid,
                pidversion: None,
            },
        },
        Err(error) => {
            tracing::warn!(%error, "could not read the named-pipe client pid; the peer stays unknown");
            PeerIdentity::unknown()
        }
    }
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::io::{self, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use dudes_proto::rpc::cred::OperatorCredential;
    use std::io::BufReader;

    use super::{Connection, Halt, HangUp, Listener, PeerIdentity, write_operator_credential};

    /// The mode a control socket is given: owner read/write, nobody else.
    pub const SOCKET_PERMISSIONS: u32 = 0o600;

    /// The mode a runtime directory is given: owner-only, so a socket inside it cannot be reached
    /// — or replaced — by another user on the box.
    pub const RUNTIME_DIR_PERMISSIONS: u32 = 0o700;

    /// The mode the operator credential file is created with.
    pub const OPERATOR_CREDENTIAL_PERMISSIONS: u32 = 0o600;

    /// The mode of a control socket file, as [`LocalTransport::bind`] sets it.
    #[must_use]
    pub const fn socket_permissions() -> u32 {
        SOCKET_PERMISSIONS
    }

    /// The mode of a runtime directory, as [`LocalTransport::bind_in`] creates it.
    #[must_use]
    pub const fn runtime_dir_permissions() -> u32 {
        RUNTIME_DIR_PERMISSIONS
    }

    /// The mode of the operator credential file, as
    /// [`LocalTransport::bind_in_with_credential`] creates it.
    #[must_use]
    pub const fn operator_credential_permissions() -> u32 {
        OPERATOR_CREDENTIAL_PERMISSIONS
    }

    /// A [`Halt`] over a duplicate of an accepted socket.
    ///
    /// `shutdown(2)` acts on the socket, not on the descriptor, so ending it through a duplicate
    /// ends the connection whichever descriptor a thread is blocked on. That is what makes this
    /// possible at all: by the time a connection is being served its halves are owned by the thread
    /// reading it and by an [`Outbound`](crate::Outbound) that may be shared into the subscriber
    /// registry, and neither can be reached from the accept loop.
    #[derive(Debug)]
    struct StreamHalt(UnixStream);

    impl Halt for StreamHalt {
        fn halt(&self) {
            // Ignored: `ENOTCONN` from a peer that has already gone is the state this call was
            // asking for, and at teardown there is nobody to report it to.
            let _ = self.0.shutdown(Shutdown::Both);
        }
    }

    impl Connection for UnixStream {
        type Reader = BufReader<UnixStream>;
        type Writer = SharedStream;

        fn split(self) -> io::Result<(Self::Reader, Self::Writer)> {
            let (reader, writer, _) = self.split_with_hang_up()?;
            Ok((reader, writer))
        }

        fn halt_handle(&self) -> io::Result<Box<dyn Halt>> {
            Ok(Box::new(StreamHalt(self.try_clone()?)))
        }

        /// The probe peeks the write half's own descriptor, through a weak reference: no
        /// descriptor is duplicated for it, and once the connection's writer is dropped the
        /// probe reads the peer as gone.
        fn split_with_hang_up(self) -> io::Result<(Self::Reader, Self::Writer, HangUp)> {
            let reader = BufReader::new(self.try_clone()?);
            let writer = Arc::new(self);
            let probe = Arc::downgrade(&writer);
            let hang_up =
                HangUp::new(move || probe.upgrade().is_none_or(|stream| peer_gone(&stream)));
            Ok((reader, SharedStream(writer), hang_up))
        }
    }

    /// The write half of an accepted socket, shared with that connection's hang-up probe.
    #[doc(hidden)]
    #[derive(Debug)]
    pub struct SharedStream(Arc<UnixStream>);

    impl Write for SharedStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            (&*self.0).write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            (&*self.0).flush()
        }
    }

    /// Peek one byte without blocking. `MSG_DONTWAIT` makes this one call non-blocking without
    /// touching the descriptor's flag, which the duplicate shares with the stream being served.
    /// End of stream is a peer that closed or shut down its write side; pending bytes or nothing
    /// to read yet is a peer that is still there.
    fn peer_gone(stream: &UnixStream) -> bool {
        use nix::errno::Errno;
        use nix::sys::socket::{MsgFlags, recv};
        use std::os::fd::AsRawFd;
        let mut byte = [0_u8; 1];
        loop {
            return match recv(
                stream.as_raw_fd(),
                &mut byte,
                MsgFlags::MSG_PEEK | MsgFlags::MSG_DONTWAIT,
            ) {
                Ok(0) => true,
                Ok(_) | Err(Errno::EAGAIN) => false,
                Err(Errno::EINTR) => continue,
                Err(_) => true,
            };
        }
    }

    /// A control socket: a bound [`UnixListener`] that reads peer credentials at accept.
    ///
    /// The path is *given*, never resolved here. Resolving it means knowing the daemon's runtime
    /// directory, which is the daemon's to know — and the crate that would resolve it is a
    /// dependency this crate does not take.
    #[derive(Debug)]
    pub struct LocalTransport {
        listener: UnixListener,
        path: PathBuf,
        credential: Option<PathBuf>,
    }

    impl LocalTransport {
        /// Bind `path` and restrict it to its owner.
        ///
        /// Four steps, and the order is the security property:
        ///
        /// 1. [`refuse_existing_path`] — `lstat(2)` the path and refuse anything already standing
        ///    there. This extends `bind(2)`'s own refusal to the **dangling symlink**, which
        ///    `bind(2)` does not refuse but *follows*.
        /// 2. `UnixListener::bind`.
        /// 3. [`verify_bound_socket`] — `lstat(2)` again and require a real socket. **This is the
        ///    load-bearing step**: step 1 and step 2 are not atomic and there is no `O_NOFOLLOW`
        ///    for `bind(2)`, so step 1 alone is a time-of-check/time-of-use gap. A link planted in
        ///    that window is followed *and is still at the path afterwards*, which is exactly what
        ///    this step reads.
        /// 4. `chmod` to [`SOCKET_PERMISSIONS`] — **after** step 3, never before, because
        ///    `chmod(2)` follows a symlink to its target. Applied before the re-verification it
        ///    would land on whatever an attacker had pointed the link at.
        ///
        /// **This call creates; it never removes.** A path this call did not create is never
        /// unlinked, and a failed bind can therefore leave a socket behind that blocks the next
        /// start until an operator clears it. That consequence is accepted deliberately: a visible
        /// failure is preferable to a daemon running at a path it cannot name.
        ///
        /// # What binding successfully does *not* establish
        ///
        /// Not that the path is exclusively the daemon's, and this call cannot establish it — see
        /// **peer classification**, which states the residual and the measurements behind it. In
        /// particular a same-uid peer may `unlink` this socket and `bind` its own at the same path
        /// at any moment afterwards, with no symlink involved; established connections survive, so
        /// the daemon observes nothing while new clients reach the other process. A symlink at a
        /// **non-final** component of `path` is not covered either — `lstat(2)` follows every
        /// component but the last, so both checks above report on a path that has already been
        /// redirected.
        ///
        /// The mode above is not a same-uid boundary either. The runtime directory has to
        /// be `0700`, which keeps *other users on the box* out; the directory is created by the
        /// same uid a hostile agent runs as. [`LocalTransport::bind_in`] is the constructor that
        /// establishes that directory instead of assuming it, and is what a daemon should use.
        ///
        /// # Errors
        ///
        /// [`io::ErrorKind::AlreadyExists`] when anything already stands at `path`, a link
        /// included; [`io::ErrorKind::InvalidData`] when the path is not a socket once bound; and
        /// whatever `lstat`, binding or `chmod` returns.
        pub fn bind(path: impl Into<PathBuf>) -> io::Result<Self> {
            let path = path.into();
            refuse_existing_path(&path)?;
            let listener = UnixListener::bind(&path)?;
            // A refusal here drops `listener`, which closes the descriptor and — because
            // `UnixListener` has no unlink-on-drop — leaves the filesystem exactly as it was
            // found. That is the "creates, never removes" rule holding on the failure path, and
            // it is a property of std rather than something this function arranges.
            verify_bound_socket(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_PERMISSIONS))?;
            Ok(Self {
                listener,
                path,
                credential: None,
            })
        }

        /// Create `dir` as an owner-only runtime directory and bind `file_name` inside it.
        ///
        /// This is the owner the socket directory did not have. [`LocalTransport::bind`]'s window
        /// is closed by the directory above the socket, and until now nothing created that
        /// directory and nothing checked it: a security precondition asserted in a doc comment and
        /// enforced nowhere.
        ///
        /// How the directory is created and checked — and what that does **not** close — is
        /// [`establish`], which this shares with
        /// [`LocalTransport::bind_in_with_credential`]. A daemon wants that one: it establishes the
        /// same directory and writes the operator credential inside it.
        ///
        /// # Errors
        ///
        /// Whatever creating the directory, reading its mode back, binding or `chmod` returns, and
        /// [`io::ErrorKind::PermissionDenied`] when `dir` is not owner-only.
        pub fn bind_in(dir: impl AsRef<Path>, file_name: impl AsRef<Path>) -> io::Result<Self> {
            let dir = dir.as_ref();
            establish(dir)?;
            Self::bind(dir.join(file_name))
        }

        /// As [`LocalTransport::bind_in`], and write `credential` into the same directory as
        /// [`OPERATOR_CREDENTIAL_NAME`], mode [`OPERATOR_CREDENTIAL_PERMISSIONS`].
        ///
        /// This is the constructor a daemon uses. The credential and the socket share one
        /// directory-establishing call on purpose: it is the only directory in the tree whose mode
        /// is given to `mkdir(2)` rather than chmodded afterwards and then **read back**, and
        /// putting the credential anywhere else would add a second path-resolution site to a
        /// surface that has an open symlink hole at its first one. Here there is no
        /// second site: the parent is the directory this call just established and verified.
        ///
        /// # Three properties of the creation, each load-bearing
        ///
        /// - **No mode window.** The mode is an argument to `open(2)` through
        ///   `OpenOptions::mode`, unlike [`LocalTransport::bind`]'s `chmod` *after* the bind. A
        ///   `umask` can only clear bits, never set them, so the result is a subset of
        ///   [`OPERATOR_CREDENTIAL_PERMISSIONS`] under any umask — never wider.
        /// - **No symlink.** `create_new` is `O_CREAT|O_EXCL`, and `open(2)` on this platform
        ///   documents that the pair fails when the last component is a symbolic link, *"even if
        ///   the symbolic link points to a non-existent name"*. So this adds no reachable
        ///   path-resolution site of its own.
        /// - **A second daemon fails loudly.** `create_new` refuses an existing file, mirroring
        ///   `bind`'s refusal to bind a path that already exists. A run that bound the socket and
        ///   then died without cleaning up leaves both files, and the next start says so rather
        ///   than serving from a credential it did not mint.
        ///
        /// The credential is **not minted here**: this crate has no randomness dependency and is
        /// not gaining one, so the 32 bytes arrive from the one place a daemon's randomness lives.
        /// What lands in the file is exactly the credential's text — no newline, nothing else — so
        /// a reader is `read_to_string` and a comparison, not a parser.
        ///
        /// # Errors
        ///
        /// Whatever establishing the directory, creating the credential file, binding or `chmod`
        /// returns; [`io::ErrorKind::AlreadyExists`] when a credential file is already there; and
        /// [`io::ErrorKind::PermissionDenied`] when `dir` is not owner-only. A failure to create
        /// the credential carries a [`CredentialCreationError`](crate::CredentialCreationError).
        pub fn bind_in_with_credential(
            dir: impl AsRef<Path>,
            file_name: impl AsRef<Path>,
            credential: &OperatorCredential,
        ) -> io::Result<Self> {
            let dir = dir.as_ref();
            establish(dir)?;
            let credential_path = write_operator_credential(dir, credential)?.path;
            let mut transport = Self::bind(dir.join(file_name))?;
            transport.credential = Some(credential_path);
            Ok(transport)
        }

        /// As [`LocalTransport::bind_in_with_credential`], accepting the Windows halt capability
        /// at the same call site on every platform.
        ///
        /// Unix owns its halt through `shutdown(2)`, so the injected function is deliberately
        /// unused here. Keeping this additive constructor preserves every existing Unix caller.
        pub fn bind_in_with_credential_and_halt<H>(
            dir: impl AsRef<Path>,
            file_name: impl AsRef<Path>,
            credential: &OperatorCredential,
            _halt: H,
        ) -> io::Result<Self> {
            Self::bind_in_with_credential(dir, file_name, credential)
        }

        /// The bound path.
        #[must_use]
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Where the operator credential was written, for a transport that wrote one.
        #[must_use]
        pub fn credential_path(&self) -> Option<&Path> {
            self.credential.as_deref()
        }
    }

    /// Create `dir` as an owner-only runtime directory, or verify the one that is already there.
    ///
    /// Extracted from [`LocalTransport::bind_in`] so that the constructor which also writes a
    /// secret into this directory cannot be written without it: the precondition is enforced by
    /// there being one way to get a directory, rather than asserted in a doc comment.
    ///
    /// The mode is given to `mkdir(2)` rather than chmodded afterwards, so this has no window of
    /// its own, and it is **read back**: `stat(2)` on `dir`, which *follows*, so what is refused is
    /// a path whose **resolved** mode is anything but owner-only. A real directory that arrives
    /// with another mode is therefore refused rather than served from; a symlink standing where the
    /// directory was to be created is not — `mkdir(2)` returns `EEXIST` through the
    /// `AlreadyExists` arm below, and the mode then read back is the target's. A directory that
    /// already exists is checked rather than recreated or silently re-chmodded: a daemon restarting
    /// into its own runtime directory is ordinary, and changing the mode of a directory this call
    /// did not create is not its decision.
    ///
    /// **This does not close the symlink question.** The directory modes are for other users on the box, not
    /// for the agent; a same-uid peer that can place a symlink where the directory is about to be
    /// created is a separate hole with a separate owner, and nothing here resolves paths.
    fn establish(dir: &Path) -> io::Result<()> {
        match fs::DirBuilder::new()
            .mode(RUNTIME_DIR_PERMISSIONS)
            .create(dir)
        {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
        let mode = fs::metadata(dir)?.permissions().mode() & 0o777;
        if mode != RUNTIME_DIR_PERMISSIONS {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "the runtime directory {} is mode {mode:04o}, not \
                     {RUNTIME_DIR_PERMISSIONS:04o}",
                    dir.display()
                ),
            ));
        }
        Ok(())
    }

    /// Refuse a path that already has anything standing at it, **a dangling symlink included**.
    ///
    /// `bind(2)` refuses a path that already exists, and that refusal is what
    /// [`LocalTransport::bind`]'s contract might lean on. It is the wrong half. `bind(2)` refuses
    /// a path whose final component **resolves** — an ordinary file, a live socket — and *follows*
    /// a link pointing at nothing, creating the socket at the link's target while the caller still
    /// holds the path it asked for. So the case that costs nothing is refused and the case that
    /// hands the control plane to whoever planted the link is not.
    ///
    /// `symlink_metadata` is `lstat(2)`, which does not follow the final component: a dangling link
    /// is `Ok` here, where `metadata` would report `NotFound`. **Any `exists()`-shaped test is
    /// `stat(2)` and inverts on exactly this input**, which is why the check is spelled this way
    /// rather than with the shorter call.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::AlreadyExists`], naming the path and whether what stands there is a link;
    /// whatever `lstat(2)` returns for anything other than `NotFound`.
    fn refuse_existing_path(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
            Ok(metadata) => {
                let what = if metadata.file_type().is_symlink() {
                    "a symbolic link"
                } else {
                    "an existing file"
                };
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "refusing to bind {}: {what} already stands there",
                        path.display()
                    ),
                ))
            }
        }
    }

    /// Re-read `path` after binding and require that a real socket is what stands there.
    ///
    /// **This is the step that closes the defect, and [`refuse_existing_path`] alone does not.**
    /// The pre-check and the bind are two syscalls with a gap between them, and there is no
    /// `O_NOFOLLOW` for `bind(2)` to close it with. A link planted in that gap is followed — and
    /// is **still a link at the path afterwards**, measured — so reading the path a second time is
    /// what detects it. The check is cheap and runs on every bind because the window cannot be
    /// made narrower.
    ///
    /// The caller refuses on error and **removes nothing**: a path this process may not have
    /// created is not this function's to unlink.
    ///
    /// # The two clauses are one refusal and two diagnoses
    ///
    /// Under `lstat(2)` a symlink's own file type is never a socket, so the `is_socket` clause
    /// would refuse a link on its own. The `is_symlink` clause is kept because it refuses it *by
    /// its right name*: an operator reading "a symbolic link stands at the path" is being told
    /// someone planted one, and "the path is not a socket" would not tell them that. Collapsing
    /// the two would keep the refusal and lose the diagnosis.
    ///
    /// # What it cannot see
    ///
    /// A symlink at a **non-final** component: `lstat(2)` follows every component but the last, so
    /// this reports faithfully on a path that was already redirected. And nothing about a later
    /// `unlink`-and-rebind, which needs no symlink at all — peer classification names both.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidData`] naming the path and which clause refused it; whatever
    /// `lstat(2)` returns.
    fn verify_bound_socket(path: &Path) -> io::Result<()> {
        let file_type = fs::symlink_metadata(path)?.file_type();
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing the socket at {}: a symbolic link stands at the path after binding, \
                     so the socket was created wherever that link points",
                    path.display()
                ),
            ));
        }
        if !file_type.is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing the socket at {}: the path is not a socket after binding",
                    path.display()
                ),
            ));
        }
        Ok(())
    }

    impl Listener for LocalTransport {
        type Conn = UnixStream;

        fn accept(&self) -> io::Result<(Self::Conn, PeerIdentity)> {
            let (stream, _addr) = self.listener.accept()?;
            let peer = crate::peer::identity(&stream);
            Ok((stream, peer))
        }
    }

    /// The two path checks, tested directly rather than through [`LocalTransport::bind`].
    ///
    /// They live inside this module because the functions are private and are staying that way:
    /// the sibling `mod tests` at the foot of this file cannot reach them, and widening the
    /// visibility of a security check so a test can call it is the thing being avoided.
    ///
    /// **Each refusal is asserted by which clause fired, not merely that one did.** Under
    /// `lstat(2)` a symlink is never a socket, so a test that only asserted "a link is refused"
    /// would stay green after the `is_symlink` clause was deleted — the `is_socket` clause would
    /// catch it — and the mutation would pass unnoticed. Asserting on the message is what makes
    /// the two clauses independently red-able.
    #[cfg(test)]
    mod tests {
        use super::{
            LocalTransport, SOCKET_PERMISSIONS, refuse_existing_path, verify_bound_socket,
        };
        use std::io;

        /// A bound socket is accepted.
        ///
        /// The positive control for both refusal tests below: without it, a
        /// `verify_bound_socket` that refused unconditionally would satisfy them.
        #[test]
        fn verify_bound_socket_accepts_a_socket_that_was_just_bound() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("duded.sock");
            let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

            verify_bound_socket(&path).expect("a real socket at the path is what this accepts");
        }

        /// A symlink is refused, **and named as a symlink**.
        ///
        /// Red when the `is_symlink` clause is deleted: the refusal still happens, from
        /// `is_socket`, but it stops saying a link was planted.
        #[test]
        fn verify_bound_socket_refuses_a_symlink_and_names_it_as_one() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("duded.sock");
            std::os::unix::fs::symlink(dir.path().join("elsewhere"), &path).unwrap();

            let refused = verify_bound_socket(&path).expect_err("a link at the path is refused");
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
            assert!(
                refused
                    .to_string()
                    .contains("a symbolic link stands at the path"),
                "the refusal must name the link rather than only report a non-socket: {refused}"
            );
        }

        /// A path that is not a socket is refused.
        ///
        /// Red when the `is_socket` clause is deleted: a regular file passes the `is_symlink`
        /// clause, so nothing else refuses it.
        #[test]
        fn verify_bound_socket_refuses_a_path_that_is_not_a_socket() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("duded.sock");
            std::fs::write(&path, b"not a socket").unwrap();

            let refused =
                verify_bound_socket(&path).expect_err("a regular file at the path is refused");
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
            assert!(
                refused.to_string().contains("not a socket"),
                "the refusal must say what was wrong with the path: {refused}"
            );
        }

        /// A path with nothing at it is the case the pre-check must let through.
        ///
        /// The positive control for the two refusals below.
        #[test]
        fn refuse_existing_path_admits_a_clear_path() {
            let dir = tempfile::TempDir::new().unwrap();

            refuse_existing_path(&dir.path().join("duded.sock"))
                .expect("nothing stands at the path, so there is nothing to refuse");
        }

        /// A **dangling** symlink is refused and named as a link.
        ///
        /// This is the input `bind(2)` follows rather than refuses, so it is the one the
        /// pre-check exists for. `lstat(2)` is what makes it visible: `metadata`, or any
        /// `exists()`-shaped test, reports `NotFound` here and would admit it.
        #[test]
        fn refuse_existing_path_refuses_a_dangling_symlink_as_a_link() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("duded.sock");
            let target = dir.path().join("nothing-here");
            std::os::unix::fs::symlink(&target, &path).unwrap();

            assert!(
                !std::fs::exists(&path).unwrap(),
                "control: the link dangles, so a stat-based test reports the path as absent — \
                 which is why this check does not use one"
            );
            let refused = refuse_existing_path(&path).expect_err("a dangling link is refused");
            assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
            assert!(
                refused.to_string().contains("a symbolic link"),
                "the refusal must say a link was what stood there: {refused}"
            );
        }

        /// An ordinary existing file is refused too, and **not** described as a link.
        #[test]
        fn refuse_existing_path_refuses_a_plain_file_without_calling_it_a_link() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("duded.sock");
            std::fs::write(&path, b"in the way").unwrap();

            let refused = refuse_existing_path(&path).expect_err("an existing file is refused");
            assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
            assert!(
                refused.to_string().contains("an existing file"),
                "the two cases are reported apart: {refused}"
            );
        }

        /// The whole sequence on a clear path still binds, and still ends `0600`.
        ///
        /// The unit-level positive control for [`LocalTransport::bind`]: three refusal arms above
        /// are all satisfied by a `bind` that refuses everything, and this is the arm that is not.
        #[test]
        fn bind_on_a_clear_path_succeeds_and_the_socket_is_owner_only() {
            use std::os::unix::fs::PermissionsExt;

            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("duded.sock");

            let transport = LocalTransport::bind(&path).expect("a clear path binds");

            assert_eq!(transport.path(), path);
            let mode = std::fs::symlink_metadata(&path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, SOCKET_PERMISSIONS,
                "step 4 runs on the success path, after step 3"
            );
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::fs::File;
    use std::io::{self, BufReader, Read, Write};
    use std::os::windows::io::{AsHandle, BorrowedHandle};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use dudes_proto::rpc::cred::OperatorCredential;
    use interprocess::os::windows::named_pipe::{
        DuplexPipeStream, PipeListener, PipeListenerOptions, PipeMode, pipe_mode,
    };
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::U16CString;

    use super::{
        Connection, Halt, HangUp, Listener, PeerIdentity, WrittenCredential, pipe_security_sddl,
        windows_peer_identity, write_operator_credential,
    };

    type BytePipe = DuplexPipeStream<pipe_mode::Bytes>;
    type ByteListener = PipeListener<pipe_mode::Bytes, pipe_mode::Bytes>;
    type Disconnect = for<'handle> fn(BorrowedHandle<'handle>) -> io::Result<()>;

    /// One shared server-side pipe handle.
    ///
    /// `interprocess` implements byte I/O for `&PipeStream`, so the reader and writer can share
    /// this value while the halt path keeps the exact `CreateNamedPipeW` handle available.
    #[derive(Clone, Debug)]
    #[doc(hidden)]
    pub struct SharedPipe(Arc<BytePipe>);

    impl Read for SharedPipe {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            (&*self.0).read(buffer)
        }
    }

    impl Write for SharedPipe {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            (&*self.0).write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    #[derive(Clone, Debug)]
    struct PipeHalt {
        stream: Arc<BytePipe>,
        disconnect: Disconnect,
    }

    impl Halt for PipeHalt {
        fn halt(&self) {
            // An already-ended instance is the requested state; teardown has nobody to report a
            // platform error to. The injected capability performs the state-changing call.
            let _ = (self.disconnect)(self.stream.as_handle());
        }
    }

    /// One accepted local Windows connection.
    ///
    /// Public only because it is [`Listener::Conn`]. Its construction stays private to
    /// [`LocalTransport`], and callers use it through [`Connection`].
    #[doc(hidden)]
    #[derive(Debug)]
    pub struct WindowsConnection {
        stream: Arc<BytePipe>,
        disconnect: Disconnect,
    }

    impl AsHandle for WindowsConnection {
        fn as_handle(&self) -> BorrowedHandle<'_> {
            self.stream.as_handle()
        }
    }

    impl Connection for WindowsConnection {
        type Reader = BufReader<SharedPipe>;
        type Writer = SharedPipe;

        fn split(self) -> io::Result<(Self::Reader, Self::Writer)> {
            let (reader, writer, _) = self.split_with_hang_up()?;
            Ok((reader, writer))
        }

        /// The probe peeks the pipe through a weak reference, so it holds no handle open: once
        /// the connection's reader and writer are dropped it reads the client as gone.
        fn split_with_hang_up(self) -> io::Result<(Self::Reader, Self::Writer, HangUp)> {
            let probe = Arc::downgrade(&self.stream);
            let hang_up = HangUp::new(move || {
                probe
                    .upgrade()
                    .is_none_or(|stream| dudes_win::pipe_peer_gone(&stream.as_handle()))
            });
            let reader = SharedPipe(Arc::clone(&self.stream));
            let writer = SharedPipe(self.stream);
            Ok((BufReader::new(reader), writer, hang_up))
        }

        fn halt_handle(&self) -> io::Result<Box<dyn Halt>> {
            Ok(Box::new(PipeHalt {
                stream: Arc::clone(&self.stream),
                disconnect: self.disconnect,
            }))
        }
    }

    /// The Windows local transport: a byte-mode named-pipe listener with creator-only ownership.
    #[derive(Debug)]
    pub struct LocalTransport {
        listener: ByteListener,
        path: PathBuf,
        credential: Option<PathBuf>,
        disconnect: Disconnect,
        /// The `operator.cred` handle whose last compatible close deletes this daemon's file.
        _credential_lease: File,
    }

    impl LocalTransport {
        /// Create the runtime directory, mint the credential and bind the derived pipe name.
        ///
        /// The supplied halt capability comes from the daemon, which already links the confined
        /// Windows host layer. This crate keeps no process-owner dependency or unsafe block.
        pub fn bind_in_with_credential_and_halt(
            dir: impl AsRef<Path>,
            file_name: impl AsRef<Path>,
            credential: &OperatorCredential,
            disconnect: Disconnect,
        ) -> io::Result<Self> {
            let dir = dir.as_ref();
            dudes_win::create_runtime_dir_creator_only(dir)?;
            let written = write_operator_credential(dir, credential)?;
            let path = crate::local_endpoint(dir, file_name);
            let sid = dudes_win::current_user_sid()?;
            let sddl = U16CString::from_str(pipe_security_sddl(&sid)).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("the pipe security descriptor contains NUL: {error}"),
                )
            })?;
            let descriptor = SecurityDescriptor::deserialize(&sddl)?;
            let listener = PipeListenerOptions::new()
                .path(path.as_path())
                .mode(PipeMode::Bytes)
                .accept_remote(false)
                .security_descriptor(Some(descriptor))
                .create_duplex::<pipe_mode::Bytes>()?;
            let WrittenCredential {
                path: credential_path,
                lease: credential_lease,
            } = written;
            Ok(Self {
                listener,
                path,
                credential: Some(credential_path),
                disconnect,
                _credential_lease: credential_lease,
            })
        }

        /// The bound pipe name.
        #[must_use]
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Where the operator credential was written.
        #[must_use]
        pub fn credential_path(&self) -> Option<&Path> {
            self.credential.as_deref()
        }
    }

    impl Listener for LocalTransport {
        type Conn = WindowsConnection;

        fn accept(&self) -> io::Result<(Self::Conn, PeerIdentity)> {
            let stream = Arc::new(self.listener.accept()?);
            let peer = windows_peer_identity(stream.client_process_id());
            Ok((
                WindowsConnection {
                    stream,
                    disconnect: self.disconnect,
                },
                peer,
            ))
        }
    }
}

#[cfg(unix)]
pub use unix::{
    LocalTransport, OPERATOR_CREDENTIAL_PERMISSIONS, RUNTIME_DIR_PERMISSIONS, SOCKET_PERMISSIONS,
    operator_credential_permissions, runtime_dir_permissions, socket_permissions,
};
#[cfg(windows)]
pub use windows::LocalTransport;

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use dudes_proto::rpc::{JSONRPC, Method};
    use serde_json::json;

    use super::*;
    use crate::auth::Requirement;
    use crate::testing::{AdmitAll, RefuseAll};

    /// The pipe names one captured SID as both owner and sole full-access trustee.
    #[test]
    fn the_pipe_descriptor_names_the_same_concrete_sid_twice() {
        let sid = "S-1-5-21-100-200-300-400";
        assert_eq!(
            pipe_security_sddl(sid),
            "O:S-1-5-21-100-200-300-400D:P(A;;GA;;;S-1-5-21-100-200-300-400)"
        );
        for rejected in [
            format!("D:P(A;;GA;;;{sid})"),
            format!("O:OWD:P(A;;GA;;;{sid})"),
            "O:COD:P(A;;GA;;;CO)".to_owned(),
            String::new(),
        ] {
            assert_ne!(
                rejected,
                pipe_security_sddl(sid),
                "an absent owner, owner alias, creator alias or platform default is not the exact descriptor"
            );
        }
    }

    /// A failed safe pid read stays in the unreadable column.
    #[test]
    fn a_failed_windows_peer_pid_read_is_unreadable() {
        let peer = windows_peer_identity(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected pid read failure",
        )));
        assert_eq!(peer, PeerIdentity::unknown());
        assert_eq!(peer.pid, PeerPid::Unreadable);
        let reported = windows_peer_identity(Ok(4242));
        assert_eq!(reported.pid.reported(), Some(4242));
    }

    fn line(method: &str, id: i64) -> String {
        format!(r#"{{"jsonrpc":"{JSONRPC}","id":{id},"method":"{method}"}}"#)
    }

    /// A writer the test can read back after handing ownership of it to an [`Outbound`].
    ///
    /// [`Connection::Writer`] is `Send + 'static` so a subscription can outlive its call, which
    /// rules out passing a `&mut Vec<u8>`; the bytes are shared instead.
    #[derive(Clone, Debug, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Sink {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Debug)]
    struct TraceWriter {
        trace: Arc<Mutex<Vec<&'static str>>>,
        fail_write: bool,
    }

    impl Write for TraceWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.trace.lock().unwrap().push("write");
            if self.fail_write {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "injected write failure",
                ))
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.trace.lock().unwrap().push("flush");
            Ok(())
        }
    }

    fn traced_outbound(trace: &Arc<Mutex<Vec<&'static str>>>, fail_write: bool) -> Arc<Outbound> {
        Arc::new(Outbound::new(TraceWriter {
            trace: Arc::clone(trace),
            fail_write,
        }))
    }

    #[test]
    fn post_response_action_runs_after_write_and_flush() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let action_trace = Arc::clone(&trace);
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register_after_response(
            Method::SystemPing,
            Requirement::AnyRole,
            |_| Ok(json!({ "pong": true })),
            move || action_trace.lock().unwrap().push("action"),
        );

        serve_stream(
            Cursor::new(format!("{}\n", line("system.ping", 1)).into_bytes()),
            &traced_outbound(&trace, false),
            PeerIdentity::unknown(),
            PeerProvenance::Unresolvable,
            &dispatcher,
        )
        .expect("the response is written before the action runs");

        let trace = trace.lock().unwrap();
        assert_eq!(trace.last(), Some(&"action"));
        assert_eq!(trace[trace.len() - 2], "flush");
        assert!(trace[..trace.len() - 2].contains(&"write"));
    }

    #[test]
    fn post_response_action_runs_after_a_failed_write_and_preserves_the_error() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let action_trace = Arc::clone(&trace);
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register_after_response(
            Method::SystemPing,
            Requirement::AnyRole,
            |_| Ok(json!({ "pong": true })),
            move || action_trace.lock().unwrap().push("action"),
        );

        let error = serve_stream(
            Cursor::new(format!("{}\n", line("system.ping", 1)).into_bytes()),
            &traced_outbound(&trace, true),
            PeerIdentity::unknown(),
            PeerProvenance::Unresolvable,
            &dispatcher,
        )
        .expect_err("the injected write failure is returned");

        assert!(matches!(
            error,
            FrameError::Io(ref error) if error.kind() == std::io::ErrorKind::BrokenPipe
        ));
        assert_eq!(*trace.lock().unwrap(), ["write", "action"]);
    }

    #[test]
    fn post_response_action_does_not_run_when_dispatch_fails() {
        fn dispatcher(
            authorizer: impl crate::Authorizer + 'static,
            requires: Requirement,
            handler: impl Fn(crate::Call<'_>) -> Result<serde_json::Value, dudes_proto::rpc::RpcError>
            + Send
            + Sync
            + 'static,
            actions: Arc<AtomicUsize>,
        ) -> Dispatcher {
            let mut dispatcher = Dispatcher::new(authorizer);
            dispatcher.register_after_response(Method::SystemPing, requires, handler, move || {
                actions.fetch_add(1, Ordering::SeqCst);
            });
            dispatcher
        }

        let actions = Arc::new(AtomicUsize::new(0));
        let cases = [
            (
                "{not json\n".to_owned(),
                dispatcher(
                    AdmitAll,
                    Requirement::AnyRole,
                    |_| Ok(json!({})),
                    Arc::clone(&actions),
                ),
            ),
            (
                format!("{}\n", line("system.ping", 2)),
                dispatcher(
                    RefuseAll,
                    Requirement::AnyRole,
                    |_| Ok(json!({})),
                    Arc::clone(&actions),
                ),
            ),
            (
                format!("{}\n", line("system.ping", 3)),
                dispatcher(
                    AdmitAll,
                    Requirement::Operator,
                    |_| Ok(json!({})),
                    Arc::clone(&actions),
                ),
            ),
            (
                format!("{}\n", line("system.ping", 4)),
                dispatcher(
                    AdmitAll,
                    Requirement::AnyRole,
                    |_| Err(error(ErrorCode::InternalError, "injected handler failure")),
                    Arc::clone(&actions),
                ),
            ),
        ];

        for (input, dispatcher) in cases {
            serve_stream(
                Cursor::new(input.into_bytes()),
                &Arc::new(Outbound::sink()),
                PeerIdentity::unknown(),
                PeerProvenance::Unresolvable,
                &dispatcher,
            )
            .expect("dispatch failures are answered normally");
        }
        assert_eq!(actions.load(Ordering::SeqCst), 0);
    }

    struct LeaseObservingWriter {
        source: Arc<OperatorAuthoritySource>,
        observed_on_drop: Arc<Mutex<Option<usize>>>,
    }

    impl Write for LeaseObservingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for LeaseObservingWriter {
        fn drop(&mut self) {
            *self.observed_on_drop.lock().unwrap() = Some(self.source.live_leases());
        }
    }

    struct CanaryConnection {
        reader: Cursor<Vec<u8>>,
        writer: LeaseObservingWriter,
    }

    impl Connection for CanaryConnection {
        type Reader = Cursor<Vec<u8>>;
        type Writer = LeaseObservingWriter;

        fn split(self) -> std::io::Result<(Self::Reader, Self::Writer)> {
            Ok((self.reader, self.writer))
        }

        fn halt_handle(&self) -> std::io::Result<Box<dyn Halt>> {
            unreachable!("the canary calls the already-admitted connection path")
        }
    }

    #[test]
    fn unwind_revokes_authority_before_outbound_writer_drop() {
        let source = Arc::new(OperatorAuthoritySource::from_replayed_agents(false));
        let context = ConditionedConnectionContext::from_accepted_peer(
            PeerIdentity::unknown(),
            PeerProvenance::NoRecordedAgent,
            Some(&source),
        );
        assert_eq!(source.live_leases(), 1, "positive control: lease acquired");

        let observed_on_drop = Arc::new(Mutex::new(None));
        let writer = LeaseObservingWriter {
            source: Arc::clone(&source),
            observed_on_drop: Arc::clone(&observed_on_drop),
        };
        let input = format!("{}\n", line("system.ping", 1));
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register(Method::SystemPing, Requirement::AnyRole, |_| {
            panic!("injected handler panic")
        });

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            let connection = CanaryConnection {
                reader: Cursor::new(input.into_bytes()),
                writer,
            };
            let _ = serve_connection_on_context(connection, context, &dispatcher);
        }));

        assert!(panicked.is_err(), "the canary reached the unwind path");
        assert_eq!(
            *observed_on_drop.lock().unwrap(),
            Some(0),
            "the authority lease is revoked before outbound writer teardown"
        );
        assert_eq!(source.live_leases(), 0);
    }

    /// The stream loop answers each frame in order and returns cleanly at end of stream — the
    /// transport-free half of the loopback tests, driven over an in-memory pair to show that the
    /// core names no socket.
    #[test]
    fn the_stream_loop_answers_every_frame_and_ends_at_eof() {
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register(Method::SystemPing, Requirement::AnyRole, |_| {
            Ok(json!({ "pong": true }))
        });

        let input = format!("{}\n{}\n", line("system.ping", 1), line("system.ping", 2));
        let sink = Sink::default();
        serve_stream(
            Cursor::new(input.into_bytes()),
            &Arc::new(Outbound::new(sink.clone())),
            PeerIdentity::unknown(),
            PeerProvenance::Unresolvable,
            &dispatcher,
        )
        .expect("a clean end of stream");

        let answers: Vec<Response> = sink
            .text()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(answers.len(), 2, "one answer per frame");
        assert_eq!(answers[0].id, RequestId::Number(1));
        assert_eq!(answers[1].id, RequestId::Number(2));
        assert_eq!(answers[0].result.as_ref().unwrap()["pong"], true);
    }

    /// An over-cap frame is reported to the peer before the connection ends, so an oversized
    /// request is a diagnosable refusal rather than a silent disconnection.
    #[test]
    fn an_over_cap_frame_is_reported_then_ends_the_connection() {
        let dispatcher = Dispatcher::new(AdmitAll);
        let mut oversized = vec![b'x'; crate::codec::MAX_FRAME_BYTES + 1];
        oversized.push(b'\n');

        let sink = Sink::default();
        let err = serve_stream(
            Cursor::new(oversized),
            &Arc::new(Outbound::new(sink.clone())),
            PeerIdentity::unknown(),
            PeerProvenance::Unresolvable,
            &dispatcher,
        )
        .expect_err("the connection ends");
        assert!(matches!(err, FrameError::TooLarge));

        let response: Response =
            serde_json::from_str(sink.text().trim()).expect("the peer was told");
        assert_eq!(
            response.error.unwrap().code,
            ErrorCode::ParseError.code(),
            "the peer learns the frame was refused"
        );
    }

    /// A response the daemon cannot frame is answered as an error the peer can read, and the
    /// connection survives to answer the next call.
    ///
    /// Before the writer enforced the cap, an over-cap response went out whole: `write_all`
    /// succeeded, this loop returned `Ok`, and the failure landed at the *reader*, which cannot
    /// resynchronise past an oversized line. The daemon learned nothing. Refusing at the writer
    /// turns that into a value — which is what a JSON-RPC error response is for — and the
    /// substitute is constant-size, so it cannot itself be refused.
    ///
    /// One handler at two sizes, chosen by the call's params, so the control discriminates on
    /// length and not on the shape of the answer. The second call is that control **and** the
    /// proof the connection survived: without it a loop that answered an error and then hung up
    /// would pass.
    #[test]
    fn a_response_over_the_cap_is_answered_as_an_error_and_the_connection_survives() {
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register(Method::SystemPing, Requirement::AnyRole, |call| {
            let pad = call
                .params
                .and_then(|params| params["pad"].as_u64())
                .unwrap_or(0);
            Ok(json!({ "pad": "x".repeat(usize::try_from(pad).unwrap()) }))
        });

        let padded = |id: i64, pad: usize| {
            format!(
                r#"{{"jsonrpc":"{JSONRPC}","id":{id},"method":"system.ping","params":{{"pad":{pad}}}}}"#
            )
        };
        let input = format!(
            "{}\n{}\n",
            padded(1, crate::codec::MAX_FRAME_BYTES),
            padded(2, 8)
        );

        let sink = Sink::default();
        serve_stream(
            Cursor::new(input.into_bytes()),
            &Arc::new(Outbound::new(sink.clone())),
            PeerIdentity::unknown(),
            PeerProvenance::Unresolvable,
            &dispatcher,
        )
        .expect("a response that does not fit does not end the stream");

        let answers: Vec<Response> = sink
            .text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(answers.len(), 2, "one answer per frame");

        assert_eq!(answers[0].id, RequestId::Number(1));
        assert_eq!(
            answers[0].error.as_ref().map(|err| err.code),
            Some(ErrorCode::InternalError.code()),
            "the peer is told its answer could not be framed"
        );

        assert_eq!(answers[1].id, RequestId::Number(2));
        assert_eq!(
            answers[1].result.as_ref().unwrap()["pad"]
                .as_str()
                .map(str::len),
            Some(8),
            "the connection carried on and answered the next call"
        );
    }

    /// The socket mode constant is `0600` and carries no group or other bits.
    ///
    /// **This is a spelling check on a constant, not a check that anything is chmodded.** It passes
    /// unchanged if [`LocalTransport::bind`] stops setting the mode altogether — verified by
    /// deleting the `set_permissions` call, under which this test still passes and
    /// `loopback::the_bound_socket_is_owner_only` correctly fails. That loopback test, which stats
    /// the file the transport actually created, is the one enforcing the directory modes; this one only stops the
    /// constant being edited to something permissive.
    #[cfg(unix)]
    #[test]
    fn the_socket_mode_constant_is_owner_only() {
        assert_eq!(socket_permissions(), 0o600);
        assert_eq!(socket_permissions() & 0o077, 0, "no group or other bits");
    }

    /// The exact production writer gives the credential a creator-only owner and DACL on Windows.
    #[cfg(windows)]
    #[test]
    fn operator_credential_has_creator_only_owner_and_dacl() {
        use dudes_proto::rpc::cred::OPERATOR_CREDENTIAL_LEN;
        use std::os::windows::io::AsHandle as _;

        let root = tempfile::tempdir().expect("temporary parent");
        let runtime = root.path().join("runtime");
        dudes_win::create_runtime_dir_creator_only(&runtime)
            .expect("create protected runtime directory");
        let credential = OperatorCredential::new("A".repeat(OPERATOR_CREDENTIAL_LEN))
            .expect("well-formed test credential");

        let written = write_operator_credential(&runtime, &credential)
            .expect("write the operator credential once");
        let path = &written.path;

        assert_eq!(
            std::fs::read_to_string(path).expect("read credential"),
            credential.expose()
        );
        let file = std::fs::File::open(path).expect("reopen credential");
        let sid = dudes_win::current_user_sid().expect("current user SID");
        assert_eq!(
            dudes_win::read_owner(file.as_handle()).expect("read credential owner"),
            sid
        );
        assert_eq!(
            dudes_win::read_dacl(file.as_handle()).expect("read credential DACL"),
            dudes_win::Dacl::Present {
                allowed: vec![sid],
                denied: Vec::new(),
                unread: 0,
            }
        );
        assert!(
            dudes_win::is_creator_only(path).expect("read credential DACL"),
            "the credential owner or DACL trustee differs from the creating user"
        );
    }

    /// A second create in the same runtime directory is refused as a [`CredentialCreationError`]
    /// that keeps `AlreadyExists`, which is what lets the daemon lead its message with it. Not yet
    /// run: it needs the Windows CI job.
    #[cfg(windows)]
    #[test]
    fn a_second_windows_credential_is_a_credential_creation_error() {
        use dudes_proto::rpc::cred::OPERATOR_CREDENTIAL_LEN;

        let root = tempfile::tempdir().expect("temporary parent");
        let runtime = root.path().join("runtime");
        dudes_win::create_runtime_dir_creator_only(&runtime)
            .expect("create protected runtime directory");
        let credential = OperatorCredential::new("A".repeat(OPERATOR_CREDENTIAL_LEN))
            .expect("well-formed test credential");
        let _first = write_operator_credential(&runtime, &credential)
            .expect("the first create holds the credential's lease");

        let refused = write_operator_credential(&runtime, &credential)
            .expect_err("a credential is already there");
        assert_eq!(
            refused.kind(),
            std::io::ErrorKind::AlreadyExists,
            "{refused:?}"
        );
        assert!(
            refused
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<CredentialCreationError>),
            "{refused:?}"
        );
    }

    /// Every failure after `CREATE_NEW` drops the pending delete-on-close handle. No partial
    /// credential survives, and the identical construction can retry without a cleanup path.
    #[cfg(windows)]
    #[test]
    fn a_failed_windows_bind_leaves_no_partial_credential() {
        use dudes_proto::rpc::cred::OPERATOR_CREDENTIAL_LEN;
        use interprocess::os::windows::named_pipe::{PipeListenerOptions, PipeMode, pipe_mode};

        let credential = OperatorCredential::new("A".repeat(OPERATOR_CREDENTIAL_LEN))
            .expect("well-formed test credential");
        let clean_retry = |runtime: &std::path::Path, path: &std::path::Path, after: &str| {
            let retry = windows::LocalTransport::bind_in_with_credential_and_halt(
                runtime,
                "duded.sock",
                &credential,
                |_| Ok(()),
            )
            .unwrap_or_else(|error| panic!("clean retry after {after} failure: {error}"));
            drop(retry);
            assert!(
                !path.exists(),
                "retry after {after} failure did not own its credential lifetime"
            );
        };

        let write_root = tempfile::tempdir().expect("write failure root");
        let write_runtime = write_root.path().join("run");
        dudes_win::create_runtime_dir_creator_only(&write_runtime).unwrap();
        let write_path = write_runtime.join(OPERATOR_CREDENTIAL_NAME);
        let error = write_operator_credential_with(
            &write_runtime,
            &credential,
            |_file, _bytes| Err(std::io::Error::other("injected credential write failure")),
            |_file| Ok(()),
        )
        .expect_err("write failure");
        assert!(error.to_string().contains("write failure"));
        assert!(
            !write_path.exists(),
            "write failure left a partial credential"
        );
        clean_retry(&write_runtime, &write_path, "write");

        let sync_root = tempfile::tempdir().expect("sync failure root");
        let sync_runtime = sync_root.path().join("run");
        dudes_win::create_runtime_dir_creator_only(&sync_runtime).unwrap();
        let sync_path = sync_runtime.join(OPERATOR_CREDENTIAL_NAME);
        let error = write_operator_credential_with(
            &sync_runtime,
            &credential,
            std::io::Write::write_all,
            |_file| Err(std::io::Error::other("injected credential sync failure")),
        )
        .expect_err("sync failure");
        assert!(error.to_string().contains("sync failure"));
        assert!(
            !sync_path.exists(),
            "sync failure left a partial credential"
        );
        clean_retry(&sync_runtime, &sync_path, "sync");

        let pipe_root = tempfile::tempdir().expect("pipe failure root");
        let pipe_runtime = pipe_root.path().join("run");
        dudes_win::create_runtime_dir_creator_only(&pipe_runtime).unwrap();
        let endpoint = crate::local_endpoint(&pipe_runtime, "duded.sock");
        let blocker = PipeListenerOptions::new()
            .path(endpoint.as_path())
            .mode(PipeMode::Bytes)
            .accept_remote(false)
            .create_duplex::<pipe_mode::Bytes>()
            .expect("occupy the exact first pipe instance");
        windows::LocalTransport::bind_in_with_credential_and_halt(
            &pipe_runtime,
            "duded.sock",
            &credential,
            |_| Ok(()),
        )
        .expect_err("pipe construction failure");
        let pipe_path = pipe_runtime.join(OPERATOR_CREDENTIAL_NAME);
        assert!(
            !pipe_path.exists(),
            "pipe failure left a partial credential"
        );
        drop(blocker);

        clean_retry(&pipe_runtime, &pipe_path, "pipe");
    }
}
