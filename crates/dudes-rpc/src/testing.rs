//! Fakes and a loopback harness, reachable from another crate's tests and from nothing else.
//!
//! The daemon that owns the runtime tests against the same harness, so this is not hidden behind
//! `#[cfg(test)]` — a second copy over there would be a second set of rules. But it is not part of
//! the library either: the module is declared `#[cfg(any(test, feature = "testing"))]`, so a
//! dependent reaches it by naming the `testing` feature in a `[dev-dependencies]` entry and a
//! `cargo build -p dudes-rpc` does not compile it at all.
//!
//! # Why the whole module is gated
//!
//! `dudes-supervisor` fakes a `ProcessHost`, and a permissive fake of a *host* is a test that
//! spawns nothing, so reaching for it in a daemon produces an agent that never starts. Here,
//! [`AdmitAll`] fakes the [`Authorizer`], and `Dispatcher::new(AdmitAll)` is a complete, working,
//! warning-free bypass of every requirement in [`crate::methods`] in three tokens.
//!
//! The same module holds `FakeRng`, which fakes the entropy source for a credential: its
//! `agent_token()` is a counter left-padded with `A` and built through the production
//! well-formedness check, so a daemon that took it minted per-agent tokens an attacker enumerates
//! from 1 upward. The failure mode there is "an agent whose token is guessable" — the token half
//! of invariant 8. Blast radius differs per fake, and a module is gated by the widest one in it.
//!
//! Nothing here is a policy. [`AdmitAll`] exists so a test can vary the [`Authorizer`] and hold
//! everything else constant — which is the only way a refusal test can tell a working seam from a
//! broken transport. [`FakeResolver`] and [`FakeMintedTokens`] are what make the classification
//! cells table-driven unit tests rather than process-spawning integration tests, which is the
//! reason `PeerResolver` is a seam at all.
//!
//! # The gate is a compile error, and it is guarded by one check
//!
//! What enforces this is the compiler: a dependent that names `AdmitAll` without the feature fails
//! to resolve the path. `tests/no_default_fakes.rs` is what keeps that arrangement from being
//! edited away — a `default` table naming the feature, a `pub mod testing;` that lost its
//! attribute, or a non-dev dependency turning the feature on all redden it.
//!
//! What it does **not** close: a dependent is free to enable the feature for a build that is not a
//! test, and nothing outside this workspace is scanned. The gate makes reaching `AdmitAll` a line
//! in a manifest rather than a `use` statement; it does not make it impossible.

use std::sync::{Arc, Mutex};

use dudes_proto::rpc::hook::HookToken;
use dudes_proto::rpc::{Request, Response};
use dudes_types::AgentId;

use crate::auth::{
    Authorizer, MintedTokens, PeerIdentity, PeerPid, PeerProvenance, PeerResolution, PeerResolver,
    PeerRole, Presented, Refusal, TokenOwner,
};
use crate::conditioned::{ConditionedConnectionContext, OperatorAuthoritySource};
use crate::dispatch::Dispatcher;
use crate::outbound::Outbound;

/// A test-only accepted connection that retains real authority while selecting a failing column.
///
/// Production acquisition ties authority to `NoRecordedAgent`, so authority and failing
/// provenance cannot coexist there. Deletion-mutant tests need that impossible cell: otherwise
/// removing the provenance predicate is masked by the independent authority predicate. The lease
/// remains opaque, non-forgeable, and acquired only from [`OperatorAuthoritySource`].
pub struct AuthorityMutationConnection {
    context: ConditionedConnectionContext,
}

impl AuthorityMutationConnection {
    /// Acquire in the source's real pre-agent epoch and retain `provenance` for dispatch.
    #[must_use]
    pub fn acquire(source: &OperatorAuthoritySource, provenance: PeerProvenance) -> Option<Self> {
        let peer = PeerIdentity {
            uid: Some(1),
            gid: Some(1),
            pid: PeerPid::Reported {
                pid: 1,
                pidversion: None,
            },
        };
        ConditionedConnectionContext::for_authority_mutation(peer, provenance, source)
            .map(|context| Self { context })
    }

    /// Dispatch through the same router entry used by a served connection.
    #[must_use]
    pub fn dispatch(
        &self,
        dispatcher: &Dispatcher,
        request: &Request,
        outbound: &Arc<Outbound>,
    ) -> Response {
        dispatcher.dispatch_on_connection(request, &self.context, outbound)
    }
}

/// The column a [`TestServer`] resolves for every peer unless the test chooses another.
///
/// [`PeerProvenance::Unresolvable`] because that is what a daemon shipping a resolver that does
/// not walk answers, so a harness with no opinion about the column runs the shipped one's.
///
/// Gated with the only thing that reads it: both uses are inside `mod loopback`, which is
/// `#[cfg(unix)]` because the sole [`crate::server::Listener`] is. The gate is on the constant
/// rather than the constant inside the module so that a second transport widens one attribute
/// instead of moving the declaration back out.
#[cfg(unix)]
const DEFAULT_COLUMN: PeerProvenance = PeerProvenance::Unresolvable;

/// An [`Authorizer`] that admits every method and still classifies nobody.
///
/// **Test-only, and not only by promise:** the module is behind the `testing` feature, so a
/// default build of this crate does not contain this type. It returns [`PeerRole::Unproven`] rather
/// than an operator role on purpose: admission and identity are different questions, and even the
/// permissive fake must not pretend to have identified anybody.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmitAll;

impl Authorizer for AdmitAll {
    fn authorize(
        &self,
        _presented: Presented,
        _provenance: PeerProvenance,
        _method: &str,
    ) -> Result<PeerRole, Refusal> {
        Ok(PeerRole::Unproven)
    }
}

/// An [`Authorizer`] that refuses every method, including `system.ping`.
///
/// The other end of [`AdmitAll`]: a test that asserts a refusal needs to know its refusal came from
/// the seam under test, and a seam that refuses *everything* is the control that separates "this
/// method is refused" from "this transport answers nothing".
#[derive(Clone, Copy, Debug, Default)]
pub struct RefuseAll;

impl Authorizer for RefuseAll {
    fn authorize(
        &self,
        _presented: Presented,
        _provenance: PeerProvenance,
        _method: &str,
    ) -> Result<PeerRole, Refusal> {
        Err(Refusal::unauthorized("refused by the test authorizer"))
    }
}

/// A [`PeerResolver`] that answers with a scripted provenance and records what it was asked.
///
/// The scripted answer is what makes the classification table's columns selectable one at a time. The recording is
/// what keeps the scripting honest: a transport that never consulted the resolver would pass
/// every column test that only asserts the verdict, and [`FakeResolver::asked`] is how a test
/// tells "the column was resolved from this connection's peer" from "the column was ignored".
///
/// **One entry per connection, not per call**, since [`crate::serve`] asks once in the connection
/// frame — so `asked().len()` is a count of connections and a test that reads it as a count of
/// calls is reading the wrong number.
/// **Settable, because `PeerIdentity` cannot tell two connections of one process apart.** A test
/// that needs one connection refused and the next resolved has no key to script against — both
/// carry the same uid, the same pid and the same credential — so the answer is changed *between*
/// connections instead. It is read once per connection, in the connection's own frame, so a
/// change takes effect for the next connection and never mid-connection.
#[derive(Debug)]
pub struct FakeResolver {
    answer: Mutex<PeerResolution>,
    asked: Mutex<Vec<PeerIdentity>>,
}

impl FakeResolver {
    /// A resolver that answers `provenance` for every peer.
    #[must_use]
    pub fn new(provenance: PeerProvenance) -> Self {
        Self::answering(PeerResolution::Resolved(provenance))
    }

    /// A resolver that answers `answer` for every peer — including [`PeerResolution::Refuse`].
    #[must_use]
    pub fn answering(answer: PeerResolution) -> Self {
        Self {
            answer: Mutex::new(answer),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// Answer `answer` from the next connection on.
    pub fn set(&self, answer: PeerResolution) {
        *self.answer.lock().expect("the fake's lock") = answer;
    }

    /// The peers this resolver was asked about, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<PeerIdentity> {
        self.asked.lock().expect("the fake's lock").clone()
    }
}

impl PeerResolver for FakeResolver {
    fn resolve(&self, peer: &PeerIdentity) -> PeerResolution {
        self.asked.lock().expect("the fake's lock").push(*peer);
        *self.answer.lock().expect("the fake's lock")
    }
}

/// A [`MintedTokens`] over a fixed list of `(token text, owner)` pairs.
///
/// **A fake of a projection, not a projection.** The real one folds `HookTokenMinted`,
/// `AgentRetired` and the records that end a recorded child over the journal and compares blake3
/// hashes; this compares the text, because a
/// test needs to name the token it is presenting. Nothing about the comparison here is a statement
/// about how the daemon should perform it.
#[derive(Debug, Default)]
pub struct FakeMintedTokens {
    minted: Vec<(String, TokenOwner)>,
}

impl FakeMintedTokens {
    /// A projection naming no agent for any token — every presented token is classification-table row 3.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// A projection naming live `agent` for each token text given: classification-table row 2.
    #[must_use]
    pub fn minted(pairs: &[(&str, AgentId)]) -> Self {
        Self::owned(pairs, TokenOwner::Live)
    }

    /// A projection naming retired `agent`, with a recorded child still running, for each token
    /// text given: the retired-agent row.
    #[must_use]
    pub fn retired_with_child(pairs: &[(&str, AgentId)]) -> Self {
        Self::owned(pairs, TokenOwner::RetiredWithChild)
    }

    fn owned(pairs: &[(&str, AgentId)], owner: fn(AgentId) -> TokenOwner) -> Self {
        Self {
            minted: pairs
                .iter()
                .map(|(text, agent)| ((*text).to_owned(), owner(*agent)))
                .collect(),
        }
    }
}

impl MintedTokens for FakeMintedTokens {
    fn owner_of(&self, token: &HookToken) -> Option<TokenOwner> {
        self.minted
            .iter()
            .find(|(text, _)| text == token.expose())
            .map(|(_, owner)| *owner)
    }
}

#[cfg(unix)]
mod loopback {
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use crate::auth::PeerResolver;
    use crate::client::Client;
    use crate::conditioned::OperatorAuthoritySource;
    use crate::dispatch::Dispatcher;
    use crate::server::{LocalTransport, StopFlag, serve, serve_with_authority};
    use crate::testing::{DEFAULT_COLUMN, FakeResolver};

    /// A daemon-shaped server on a real Unix socket, for tests that need the transport rather than
    /// a fake of it.
    ///
    /// The runtime directory is given rather than chosen: a temporary directory is the caller's to
    /// own and to clean up, and this crate resolves no paths. What goes *inside* it is the
    /// harness's, so every test spells one path instead of two.
    #[derive(Debug)]
    pub struct TestServer {
        socket: PathBuf,
        stop: Arc<StopFlag>,
        thread: Option<JoinHandle<()>>,
    }

    /// The socket a [`TestServer`] binds inside its runtime directory.
    const SOCKET_NAME: &str = "duded.sock";

    impl TestServer {
        /// The connection cap [`TestServer::start`] uses.
        ///
        /// **A harness number, not a production one.** [`serve`] takes the cap as a required
        /// argument precisely so that no build inherits an unbounded default; what a daemon should
        /// pass depends on how many agents it supervises, since each agent's hook shim opens one
        /// connection per call, and that is the daemon's to state. This is only "more than any test
        /// in this repository opens at once", so a test that wants the cap to bite says so with
        /// [`TestServer::start_with_cap`].
        pub const CAP: NonZeroUsize = NonZeroUsize::new(64).unwrap();

        /// Create `runtime_dir` as an owner-only runtime directory, bind a socket inside it, and
        /// start serving `dispatcher` on a background thread.
        ///
        /// `runtime_dir` must not exist yet — [`LocalTransport::bind_in`] creates it `0700`, which
        /// is the point: the harness binds the way a daemon does, so the whole suite exercises that
        /// path rather than one test doing so. The socket lands at `runtime_dir/duded.sock`, and
        /// [`TestServer::socket`] is the way to find it.
        ///
        /// # Panics
        ///
        /// If the runtime directory cannot be created or the socket cannot be bound.
        #[must_use]
        pub fn start(runtime_dir: impl Into<PathBuf>, dispatcher: Dispatcher) -> Self {
            Self::start_with_cap(runtime_dir, dispatcher, Self::CAP)
        }

        /// As [`TestServer::start`], with the [`PeerResolver`] the test chooses.
        ///
        /// **This is the only way a test selects a classification-table column now.** The column is resolved in
        /// [`serve`]'s connection frame rather than inside the seam, so a classifier
        /// built over a scripted resolver would never be asked one: what decides the column is the
        /// value handed here.
        ///
        /// # Panics
        ///
        /// If the runtime directory cannot be created or the socket cannot be bound.
        #[must_use]
        pub fn start_with_resolver(
            runtime_dir: impl Into<PathBuf>,
            dispatcher: Dispatcher,
            resolver: Arc<dyn PeerResolver>,
        ) -> Self {
            Self::start_inner(runtime_dir, dispatcher, Self::CAP, resolver, None)
        }

        /// As [`TestServer::start_with_resolver`], with a controlled authority source.
        ///
        /// This is the real accepted-connection path, not a context constructor exposed to a
        /// test: on the connection's own thread, before its first request is read, the server
        /// resolves provenance and asks the source — exactly as the daemon does. A test that reads
        /// [`OperatorAuthoritySource::live_leases`] therefore polls for it rather than assuming it
        /// is already true the instant a `connect` returns.
        ///
        /// # Panics
        ///
        /// If the runtime directory cannot be created or the socket cannot be bound.
        #[must_use]
        pub fn start_with_authority(
            runtime_dir: impl Into<PathBuf>,
            dispatcher: Dispatcher,
            resolver: Arc<dyn PeerResolver>,
            authority: Arc<OperatorAuthoritySource>,
        ) -> Self {
            Self::start_inner(
                runtime_dir,
                dispatcher,
                Self::CAP,
                resolver,
                Some(authority),
            )
        }

        /// As [`TestServer::start`], with the connection cap the test chooses.
        ///
        /// # Panics
        ///
        /// If the runtime directory cannot be created or the socket cannot be bound.
        #[must_use]
        pub fn start_with_cap(
            runtime_dir: impl Into<PathBuf>,
            dispatcher: Dispatcher,
            cap: NonZeroUsize,
        ) -> Self {
            Self::start_inner(
                runtime_dir,
                dispatcher,
                cap,
                Arc::new(FakeResolver::new(DEFAULT_COLUMN)),
                None,
            )
        }

        /// The one constructor; the four above are the choices a test makes about it.
        fn start_inner(
            runtime_dir: impl Into<PathBuf>,
            dispatcher: Dispatcher,
            cap: NonZeroUsize,
            resolver: Arc<dyn PeerResolver>,
            authority: Option<Arc<OperatorAuthoritySource>>,
        ) -> Self {
            let transport = LocalTransport::bind_in(runtime_dir.into(), SOCKET_NAME)
                .expect("bind the test socket");
            let socket = transport.path().to_path_buf();
            let stop = Arc::new(StopFlag::new());
            let thread = {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    // A listener error is how a stopped server ends; the test asserts on the
                    // conversation, not on the loop's exit.
                    if let Some(authority) = authority {
                        let _ = serve_with_authority(
                            &transport,
                            &*resolver,
                            &dispatcher,
                            &authority,
                            &stop,
                            cap,
                        );
                    } else {
                        let _ = serve(&transport, &*resolver, &dispatcher, &stop, cap);
                    }
                })
            };
            Self {
                socket,
                stop,
                thread: Some(thread),
            }
        }

        /// The bound socket path.
        #[must_use]
        pub fn socket(&self) -> &Path {
            &self.socket
        }

        /// A client connected to this server.
        ///
        /// # Panics
        ///
        /// If the socket cannot be connected to.
        #[must_use]
        pub fn client(
            &self,
        ) -> Client<
            std::io::BufReader<std::os::unix::net::UnixStream>,
            std::os::unix::net::UnixStream,
        > {
            Client::connect(&self.socket).expect("connect to the test socket")
        }

        /// Stop the loop and wait for it.
        ///
        /// Sets the flag and then connects once, because a blocking accept does not observe a flag
        /// until a connection arrives.
        ///
        /// **This does not wait on the peers.** [`StopFlag::stop`] halts every connection the loop
        /// is serving, so a test that leaks a connected peer — or leaves a subscriber that stopped
        /// reading — fails on its own assertion rather than hanging here, including when this runs
        /// from [`Drop`]. `tests/lifecycle.rs` is what holds that.
        pub fn shutdown(&mut self) {
            self.stop.stop();
            let _ = std::os::unix::net::UnixStream::connect(&self.socket);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown();
        }
    }
}

#[cfg(unix)]
pub use loopback::TestServer;
