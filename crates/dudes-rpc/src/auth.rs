//! The authorisation seam: one function on the dispatch path, and the classifier that fills it.
//!
//! Invariant 8 splits identity in two, and this module keeps the halves apart in the types:
//!
//! - [`Presented`] — what the *sender* put in the call: an operator credential on the envelope, an
//!   agent token in the params. A sender controls this completely.
//! - [`PeerIdentity`] — what the daemon could *independently resolve* about the peer, and
//!   [`PeerProvenance`] — which agent, if any, that peer belongs to. A sender does not fill these
//!   in.
//!
//! [`Authorizer::authorize`] takes both, separately, and returns a [`PeerRole`] or a [`Refusal`].
//! What any presentation resolves to — including presenting nothing — is one table: the
//! peer-classification table [`classify`] implements, in this module and nowhere else. Which role may call
//! which method is a different table, each method's `Requirement`. **That table is not in this module at
//! all**: it lives at each method's registration, and this file must never grow a method name.
//! The peer-classification table *is* implemented here, in exactly one function — [`classify`] — because
//! one function is easier to audit than a check per handler, and because every cell in one `match`
//! is a thing a reader can check line by line.
//!
//! # What is here and what is not
//!
//! [`PeerClassifier`] is the real seam. It picks the classification table's **row**; its **column** arrives already
//! resolved, from [`serve`](crate::serve)'s connection frame. Two ports, filled by
//! whoever builds the seam and never by this crate:
//!
//! - [`PeerResolver`] answers *which agent's tree is this peer in*. It is called once per
//!   connection, by the transport, and its answer is what reaches every call on that connection.
//!   No implementation ships here — this crate can reach neither a process table nor a roster —
//!   and the trait's own documentation carries what a correct one owes.
//! - [`MintedTokens`] answers *which agent minted this token, and whether that agent is live or
//!   retired with a recorded child still running*. Its implementation is a projection of the
//!   journal, and this crate deliberately builds no set of its own.
//!
//! Both are `trait`s here because this crate reads no process table and folds no journal; that is
//! what keeps the classification table's cells table-driven unit tests instead of process-spawning integration
//! tests.

use std::fmt;
use std::sync::Arc;

pub use dudes_proto::PeerProvenance;
use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::hook::HookToken;
use dudes_proto::rpc::{Method, Request};
use dudes_types::AgentId;
use serde_json::Value;

/// What the process-bearing local transport reported about its peer.
///
/// A local transport that cannot
/// report a peer process is refused before any agent-callable method; a failed credential read is
/// represented as [`PeerPid::Unreadable`] and can never select a hash-only authorization cell.
///
/// # The pid is not an identity on macOS
///
/// A [`PeerPid::Reported`] pid is what the platform reported, and on macOS that is a live field
/// rather than a captured one. Placing a peer in an agent's tree means walking its ancestry, and on
/// macOS the walk's input narrows to `LOCAL_PEERTOKEN` (one of the options available) because `LOCAL_PEERTOKEN`
/// returns `EINVAL` for a dead identifier where the pid options return one silently, and XNU
/// recycles identifiers — so an ancestry walk could otherwise begin at a recycled identifier and
/// produce a silent false match.
///
/// **Reading the value once at accept is not a mitigation, and no code or comment here may claim
/// it is.** `connect(2)` establishes through the listen backlog, so a peer can touch the socket
/// before the server's `accept(2)` returns; a two-arm probe with one variable — connector-only
/// versus connector-plus-grandchild — read the connector in the first arm and the *grandchild* in
/// the second, at accept, with the uid held constant as the control. The value is chosen by the
/// peer. This type therefore carries it and concludes nothing from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PeerPid {
    /// The transport named a process.
    Reported {
        /// The process id, as reported.
        pid: i32,
        /// macOS's `pidversion` from the peer's audit token, where the platform has one.
        ///
        /// **Carried, not yet used.** It is the one recycling check available at the walk's
        /// starting hop — the hop the adoption key does not cover — but
        /// the adoption key records pid, start value and boot identity and no pidversion,
        /// so there is nothing to compare it against until that type grows a field. That is
        /// `dudes-supervisor`'s and a change to the adoption key, not this crate's.
        pidversion: Option<u32>,
    },
    /// The peer has a process and the daemon could not learn which.
    ///
    /// The least privileged answer, and the one every failure falls to. **Never the fourth
    /// column**: a read that failed says nothing about whether a process is there.
    #[default]
    Unreadable,
}

impl PeerPid {
    /// The pid, where one was reported.
    ///
    /// Deliberately *not* an `Option<i32>` round trip for deciding anything: a caller that reaches
    /// for this to key a classification-table column has thrown away the distinction this type exists to make.
    /// It is here for a resolver that needs somewhere to start walking, and for logs.
    #[must_use]
    pub const fn reported(self) -> Option<i32> {
        match self {
            Self::Reported { pid, .. } => Some(pid),
            Self::Unreadable => None,
        }
    }
}

/// What the daemon independently resolved about the peer of a connection.
///
/// `uid` and `gid` are `Option` because "this transport reports no such thing" is a real answer,
/// not a failure. A missing field must never be read as permission — under default-deny, `None` is
/// the *least* privileged answer, and that is the direction a failure to read has to fall. The pid
/// is [`PeerPid`] rather than an option for the reason recorded on that type.
///
/// **Same uid is not a boundary.** The peer credential admits a connection and cannot tell an
/// operator from an agent; nothing here concludes anything from `uid`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerIdentity {
    /// The peer's user id, where the transport reports one.
    pub uid: Option<u32>,
    /// The peer's primary group id, where the transport reports one.
    pub gid: Option<u32>,
    /// What the transport reported about the peer's process. See [`PeerPid`] before deciding
    /// anything from it.
    pub pid: PeerPid,
}

impl PeerIdentity {
    /// A peer the transport could tell us nothing about.
    ///
    /// Named rather than spelled `PeerIdentity::default()` at call sites so that "we know nothing"
    /// is a deliberate statement in the code that produces it. The pid is
    /// [`PeerPid::Unreadable`] — the least privileged answer.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            uid: None,
            gid: None,
            pid: PeerPid::Unreadable,
        }
    }
}

/// Which agent's tree a peer is in — the seam peer classification needs.
///
/// # No implementation of this ships here, and that is deliberate
///
/// The live walk cannot live in this crate: `cargo xtask layering` forbids `dudes-rpc` from
/// reaching `dudes-host` (which reads process tables) or `dudes-policy` (which `dudes-supervisor`
/// depends on, so `dudes-rpc → dudes-supervisor` is a transitive violation too). It also should
/// not land until it is known that the ppid chain survives managed-mode launches: a broken chain
/// would defeat the ancestry approach outright. The seam and its tests do not depend on that; a
/// resolver does.
///
/// # What a correct implementation does
///
/// Specified here so the crate that implements it has no room to improvise:
///
/// 1. Start at the reported pid: `SO_PEERCRED` on Linux, **`LOCAL_PEERTOKEN`** on macOS — not
///    `LOCAL_PEERPID`, which returns a dead pid silently — `GetNamedPipeClientProcessId` on
///    Windows.
/// 2. Walk the ppid chain upward. **No fixed hop bound**; any such number would have nothing
///    behind it. What the walk needs is a **visited-pid guard**, because the
///    reads are not atomic and a process reparented mid-walk can make the observed chain revisit a
///    pid — the guard turns that into a terminated walk with no match rather than a loop.
/// 3. At each hop, compare against the recorded children of every agent, a retired agent's
///    included until the journal record that ends each child, by **the adoption
///    key** — pid, start value, boot identity — never by pid alone.
/// 4. The first match names the agent. Pid 1, an unreadable parent, or a revisit is
///    [`PeerProvenance::NoRecordedAgent`]; a starting pid with no live process behind it, or a
///    platform read that refused, is [`PeerProvenance::Unresolvable`]. **Never an error and never
///    a match.**
/// 5. Where a non-escapable relation exists it is used **in addition** to ancestry and both must
///    agree; disagreement is [`PeerProvenance::NoRecordedAgent`], the fail-closed conjunction.
///
/// **Neither non-escapable arm has ever been exercised against a kernel, and no comment anywhere
/// may imply otherwise.** The claim that Linux cgroup-v2 membership and Windows job
/// objects are not escapable by an unprivileged process is itself *not verified — cited
/// from documentation, not probed*, and this repository is developed on macOS, which has neither
/// relation. On macOS ancestry stands alone and is escapable by a double fork, which is precisely
/// why ancestry is used only to *allow* and never to *deny*: escaping the relation demotes a peer
/// and can never promote one.
///
/// # An implementation may land, because the seam does not ask one for the impossible
///
/// A correct implementation is **stable for the life of a connection**. Re-resolving per call
/// would let one connection be `Agent(A)` on one call and `Operator` on the next, which is the
/// oscillation seen on macOS — four reads of the same socket returning the
/// connector, then a grandchild, then the connector again. That is a requirement
/// on the implementation.
///
/// **It is not a requirement an implementation can fail.** [`resolve`](PeerResolver::resolve)
/// is called **once per connection**, by [`serve`](crate::serve) on that connection's own thread
/// and before any request on it is read, and the answer is what reaches every call on that
/// connection. So an implementation that reads the process table on every invocation is
/// invoked once and cannot oscillate: stability is a property of the call site rather than of the
/// implementation's discipline, which is the point of resolving once.
///
/// Three statements, in the order they depend on each other:
///
/// 1. **The seam is handed a resolved value, not a resolver.** [`Authorizer::authorize`] takes a
///    [`PeerProvenance`] by value and no [`PeerIdentity`]. An `Authorizer` holding an
///    `Arc<dyn PeerResolver>` has nothing to pass it, so a per-call re-resolution does not
///    compile.
/// 2. **What still type-checks is a fabrication, not an unstable attestation.** `PeerIdentity` is
///    constructible from outside this crate — `unknown()` is `pub const fn`, the fields are `pub`,
///    and [`peer::identity`](crate::peer::identity) manufactures a real one from any socket the
///    caller owns — so an implementation *can* resolve some pid. It can never resolve *this
///    connection's* peer, which it is not handed. That is a statement about a process of the
///    implementation's own choosing, and an `Authorizer` that fabricates needs no resolver to do
///    it: it may return any [`PeerRole`] directly. The resolver adds nothing to a rogue
///    implementation's power.
/// 3. **So what this trait still owes is the specification above and not stability.** Steps 1–5
///    are the whole of it; no type in this crate checks them, and
///    the exhaustive provenance tests keep step 4's fail-closed direction from drifting.
///
/// Where the walk itself lives is a layering question this crate does not decide for its
/// dependents: it cannot reach a process table or a roster from here, so the implementation is the
/// daemon's.
pub trait PeerResolver: fmt::Debug + Send + Sync {
    /// Which agent's tree `peer` is in, or that the resolution never answered.
    ///
    /// Never returns an error. A resolution that *completed* answers [`PeerResolution::Resolved`]
    /// carrying a column of the table — including [`PeerProvenance::Unresolvable`], which is a
    /// column rather than a fault. A resolution that *did not complete* answers
    /// [`PeerResolution::Refuse`] and the connection is dropped before any request context
    /// exists. Which non-answers may reach `Refuse` is a closed list this crate does not own; see
    /// [`PeerResolution::Refuse`].
    fn resolve(&self, peer: &PeerIdentity) -> PeerResolution;
}

/// What one [`PeerResolver`] answered for one accepted connection.
///
/// # Why a second value and not a fifth column
///
/// Each of the classification table's four columns says something *about the peer*, and every one of them is a
/// completed resolution: [`PeerProvenance::Unresolvable`] means "a read this resolution needed
/// refused, so the least-privileged cell is the answer", which is still an answer the classifier
/// can score. A resolution that never finished says nothing about the peer at all, and the one
/// thing it must not do is borrow the cell that admits an operator credential ([`classify`] row 1
/// × `Unresolvable` → [`PeerRole::Operator`]). So it is a second value beside the table rather
/// than a new column inside it, and the type is what makes "did not answer" impossible to spell
/// as "answered nothing".
///
/// **Never serialized, and not a wire type.** It is produced and consumed inside one connection
/// frame, so it lives here rather than in the outbound contract crate; nothing under `schemas/`
/// changes, because nothing here crosses a wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerResolution {
    /// The resolution completed and reached this column.
    Resolved(PeerProvenance),
    /// The resolution never answered, and this connection is refused.
    ///
    /// **Payload-free on purpose.** A reason carried here would be a reason the refusing site
    /// could branch on, and there is exactly one thing to do with a connection whose provenance
    /// does not exist: drop it before a context is built. *Which* resolutions may produce this is
    /// a closed list owned by the implementing daemon and not by this crate — a resolution that
    /// answered a column may not refuse.
    Refuse,
}

/// Which agent minted a presented token — the classification table's row 2, the retired-agent row, or row 3.
///
/// # This is a projection, and this crate does not build it
///
/// The valid-token set is the fold of `HookTokenMinted`, `AgentRetired` and the records that end
/// a recorded child (`TurnEnded`, `Detached`) over the journal. By
/// invariant 1 that makes it a **projection `dudes replay` must rebuild**, which is what
/// `dudes_supervisor::World` is for. It is a port here rather than a set for two reasons that are
/// not style:
///
/// - Two independent folds are two things that can disagree, and the one that is wrong is the one
///   nobody replays.
/// - A set built inside this module could not know whether a *previous run's* hash is still valid,
///   and set membership over every hash minted for a live agent — never "the newest" — is the
///   whole of the validity check
///
/// The token itself is never journaled; the record carries blake3 of the 32 bytes. Hashing is
/// therefore the implementation's, not this crate's, which is also why this crate needs no hash
/// dependency and holds no hash of anything.
pub trait MintedTokens: fmt::Debug + Send + Sync {
    /// The agent this token was minted for, and which of its two token rows that minting reaches.
    ///
    /// `None` covers both of the classification table row 3's cases — "a token matching no minting, or a retired
    /// agent's that has no recorded child" — because the table gives them one answer and a caller
    /// that could tell them apart would have an oracle for which agents exist.
    fn owner_of(&self, token: &HookToken) -> Option<TokenOwner>;
}

/// The agent a minted token names, as [`MintedTokens`] answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenOwner {
    /// Row 2: the agent is not retired.
    Live(AgentId),
    /// The retired-agent row: the agent is retired, and at least one of its recorded children has no
    /// recorded end yet.
    RetiredWithChild(AgentId),
}

/// What a call presents about itself: the half of identity the sender controls.
///
/// Two halves, carried separately because the contract treats them separately: the operator
/// credential rides the [`Request`] envelope and the agent token is a per-call
/// parameter on the agent-callable methods (never a header, never argv,
/// never a connection-level handshake, because the agent-side clients open one connection per call
/// and a connection-level binding would bind state to nothing reusable).
///
/// # Extraction is lenient for the token and not for the credential
///
/// The two halves are not symmetric, and the asymmetry is where they are parsed rather than a
/// choice made here.
///
/// - **The token is lenient.** It is a field of a free-form `params` [`Value`], so
///   `Presented::token_of` reads it with `HookToken::new(..).ok()`: text that is not well formed
///   presents as `None`. That is the direction it has to fall — a malformed token is an *unproven*
///   identity, invariant 8 says unproven identity is refused, and it must reach the seam and be
///   refused there rather than short-circuit into a different error before the seam ever runs.
///   Refusing it earlier with a distinct code would also hand a peer an oracle separating
///   "malformed" from "wrong".
/// - **The credential is not.** It rides the [`Request`] envelope as a typed field, and
///   `OperatorCredential`'s `Deserialize` runs its well-formedness check, so malformed text fails
///   the *whole envelope* into `-32600 InvalidRequest` at
///   [`Dispatcher::handle_line`](crate::Dispatcher::handle_line) and never
///   reaches the seam at all (`dudes_proto::rpc`'s `a_malformed_credential_fails_the_envelope`
///   pins both arms). [`Presented::from_request`]'s `.ok()` is therefore unreachable from the
///   wire: every credential on a parsed `Request` is already well formed, and the fallback exists
///   only so a rebuild that somehow failed would present nothing rather than panic.
///
/// So the credential half *does* separate "malformed" from "wrong" — malformed is `-32600`, wrong
/// is classification-table row 4 — and no comment here may claim otherwise. The disclosure is what a schema
/// already public in `schemas/rpc.schema.json` states: how long the credential is and which
/// alphabet it uses. What it does not disclose is whether a well-formed one is *this daemon's*,
/// which is the fact worth keeping and the one row 4 keeps.
#[derive(Debug, Default)]
pub struct Presented {
    /// The per-agent token this call carried, if it carried a well-formed one.
    ///
    /// Nothing in this crate validates it. Validity is [`MintedTokens`].
    pub token: Option<HookToken>,
    /// The operator credential this call carried on its envelope, if it carried a well-formed one.
    ///
    /// Well-formed is not recognised: whether it is *the* credential is [`PeerClassifier`]'s
    /// constant-time comparison.
    pub cred: Option<OperatorCredential>,
}

impl Presented {
    /// What `request` presents.
    ///
    /// Reads only the envelope and the params object; it does not know any method's schema, which
    /// is what keeps the seam one function rather than one per handler.
    ///
    /// This consumes nothing: the credential is rebuilt from its text rather than moved out of the
    /// request, because [`Request`] is borrowed on the dispatch path and
    /// [`OperatorCredential`] has no `Clone` by design. A credential already on a [`Request`] is
    /// well formed, so the rebuild cannot fail; if it somehow did, the call presents nothing and
    /// is refused, which is the direction a failure has to fall.
    #[must_use]
    pub fn from_request(request: &Request) -> Self {
        Self {
            token: Self::token_of(request.params.as_ref()),
            cred: request
                .cred
                .as_ref()
                .and_then(|cred| OperatorCredential::new(cred.expose()).ok()),
        }
    }

    /// What a params object presents. The envelope half is absent, so this presents no credential.
    #[must_use]
    pub fn from_params(params: Option<&Value>) -> Self {
        Self {
            token: Self::token_of(params),
            cred: None,
        }
    }

    /// The well-formed token in `params`, if there is one.
    fn token_of(params: Option<&Value>) -> Option<HookToken> {
        params
            .and_then(|params| params.get("token"))
            .and_then(Value::as_str)
            .and_then(|text| HookToken::new(text).ok())
    }
}

/// Which of the classification table's four rows a call presents.
///
/// The table's rows are exclusive — each names one thing "the call presents" — so a call that
/// presents both a credential and a token is not a row at all. [`PeerClassifier::row`] picks one,
/// fail-closed, and says which; this type is what it picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentedRow {
    /// Row 1: a valid operator credential.
    OperatorCredential,
    /// Row 2: a token whose hash is a non-retired minting for this agent.
    MintedToken(AgentId),
    /// The retired-agent row, split out of row 3: a token whose hash is a minting for this retired
    /// agent while it still has a recorded child.
    RetiredToken(AgentId),
    /// Row 3: a token matching no minting, or a retired agent's that has no recorded child.
    UnmintedToken,
    /// Row 4: nothing this daemon recognises.
    Nothing,
}

/// What a call resolved to. The three names are `Unproven`, `Operator` and
/// `Agent{token}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRole {
    /// The peer proved nothing. This holds in every provenance column.
    ///
    /// Not a refusal of the connection: it is a role, and each method's `Requirement` gives it exactly one method.
    /// Named `Unproven` and not `Unknown`, which the fixed vocabulary has already spent on
    /// `Liveness::Unknown`, a property of a recorded child process rather than of a caller.
    Unproven,
    /// The peer presented the operator credential from a peer the daemon could not place inside
    /// any agent's tree.
    Operator,
    /// The peer presented a token minted for this agent, from a peer the daemon placed inside that
    /// same agent's tree.
    Agent {
        /// The agent, resolved. A handler reads this and never re-derives an identity from a
        /// params field naming one.
        agent: AgentId,
    },
}

impl PeerRole {
    /// The role's name, for logs and messages. Never the token and never the credential.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Unproven => "unproven",
            Self::Operator => "operator",
            Self::Agent { .. } => "agent",
        }
    }
}

/// What a method requires of the role the seam resolved.
///
/// This is the *vocabulary* the method table is written in, not the table. Which requirement a
/// given method carries is stated once, at that method's
/// [`Dispatcher::register`](crate::Dispatcher::register) call, so the cells of the classification
/// table live only in [`classify`] and this module keeps the promise it makes at the top of the
/// file.
///
/// The variants are a **set, not a ladder**. `Operator` and `Agent` are
/// not comparable — an agent may append to its own memory and an operator may not — so
/// [`Requirement::OperatorOrAgent`] means "either", and no variant is a threshold that another
/// clears.
/// There is deliberately no `Default`: [`Dispatcher::register`](crate::Dispatcher::register) takes
/// this by value, so the widest requirement is one someone has to type on purpose rather than one a
/// new handler can arrive at by omission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Requirement {
    /// Every role, [`PeerRole::Unproven`] included.
    AnyRole,
    /// [`PeerRole::Operator`].
    Operator,
    /// [`PeerRole::Agent`].
    Agent,
    /// [`PeerRole::Operator`] or [`PeerRole::Agent`].
    OperatorOrAgent,
}

impl Requirement {
    /// Whether `role` satisfies this requirement.
    #[must_use]
    pub const fn admits(self, role: &PeerRole) -> bool {
        matches!(
            (self, role),
            (Self::AnyRole, _)
                | (Self::Operator | Self::OperatorOrAgent, PeerRole::Operator)
                | (Self::Agent | Self::OperatorOrAgent, PeerRole::Agent { .. })
        )
    }

    /// What to tell a caller this requirement refused.
    ///
    /// Names the requirement, never the role the peer *did* resolve to: what the daemon worked out
    /// about a peer is not something the peer is owed, and a message that varied with the answer
    /// would be an oracle for probing the seam.
    #[must_use]
    pub const fn refusal(self) -> &'static str {
        match self {
            Self::AnyRole => "this method admits every role",
            Self::Operator => "this method requires an operator",
            Self::Agent => "this method requires an agent",
            Self::OperatorOrAgent => "this method requires an operator or an agent",
        }
    }
}

/// The seam's refusal. Becomes
/// [`ErrorCode::Unauthorized`](dudes_proto::rpc::ErrorCode::Unauthorized) (`-32001`) on the wire.
///
/// The reason travels to the caller, so it says what rule refused and never what the daemon knows
/// about the peer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct Unauthorized {
    /// What to tell the caller.
    pub reason: String,
}

impl Unauthorized {
    /// A refusal carrying `reason`.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Why the seam refused a call — the two error codes that classification produces.
///
/// The table needs both and the refusal type has to say both: a refusal that is always `-32001`
/// would leave row 3's `-32004 Refused{gate: token}` with no way to reach the wire.
/// A refusal type that cannot spell a mandated cell makes that cell untestable, and an untestable
/// cell is an unimplemented one.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// `-32001 Unauthorized`.
    #[error("{0}")]
    Unauthorized(#[from] Unauthorized),
    /// `-32004 Refused{gate: token}` — classification-table row 3, in every provenance column, and the retired-agent
    /// row outside its owner's tree.
    ///
    /// A separate code because the table makes it one, and it is the honest answer: the call
    /// presented a token that names no live agent of this daemon, which is a different fact from
    /// "you may not call this".
    #[error("the presented token matches no minting for a live agent")]
    Token,
}

impl Refusal {
    /// A `-32001` refusal carrying `reason`.
    pub fn unauthorized(reason: impl Into<String>) -> Self {
        Self::Unauthorized(Unauthorized::new(reason))
    }
}

/// What every `-32001` cell of the classification table tells the caller.
///
/// **One string for every such cell, deliberately.** Four of the table's cells refuse, and a
/// message that varied with the cell would tell a peer whether the daemon placed it in an agent's
/// tree and whose token it was holding — so the peer learns that it may not call this, and
/// nothing else.
///
/// # The credential oracle is structural, and message discipline does not reach it
///
/// **A peer can tell a right credential from a wrong one in one call, and no wording of this
/// string changes that.** The two paths differ in *whether an answer comes back at all*:
///
/// - A **wrong** credential is classification-table row 4 — nothing this daemon recognises — in every
///   provenance column, so it resolves to `Ok(PeerRole::Unproven)`. `system.ping` carries
///   [`Requirement::AnyRole`], `AnyRole` admits every role including `Unproven`, and the call
///   **answers** — with a `PingResult` carrying this build's version, where the daemon has
///   registered [`system::ping`](crate::system::ping).
/// - The **right** credential from a peer the daemon placed in an agent's tree — row 1, column 1,
///   the disqualification of an operator credential from inside an agent's tree — is `Err` at the seam, and the seam runs before the method is looked
///   up. So it is `-32001` on *every* method, `system.ping` included.
///
/// Answered versus refused on one liveness call is a cleaner discriminator than any message text,
/// and it is a consequence of the table's shape plus `system.ping`'s `Requirement`, not of anything written here.
/// Both arms are pinned by a test so the fact cannot quietly stop being true.
///
/// This is why [`Requirement::refusal`] stays distinct from this string rather than collapsing
/// into it: the method's requirement is its `Requirement`, which is public, and an operator who forgot their
/// credential is owed "this method requires an operator" to diagnose. Collapsing it would cost a
/// diagnostic and close nothing, because what leaks is not in the text.
const REFUSED: &str = "this peer may not call this method";

/// The one place a cell of the classification table is decided.
///
/// The cells are not restated in prose anywhere in this crate: they live in this function, and it is
/// the only place they run. Each method's `Requirement` is stated at its
/// [`Dispatcher::register`](crate::Dispatcher::register) call.
///
/// # What totality actually rests on
///
/// Not "no wildcard": rows 3 and 4 *are* wildcards in the column position, because the table gives
/// each of them one answer for all three columns and writing three identical arms would be three
/// places to get one rule wrong. Exhaustiveness holds because **rows 1 and 2 and the retired-agent row
/// enumerate every column explicitly**, so a fourth column added to [`PeerProvenance`] fails to
/// compile in those three rows rather than falling into row 3's or row 4's `_`. Generalising any
/// of their column lists to `_` would compile, would keep this function total, and would silently
/// end the property — so the enumeration is load-bearing and is not a style to be tidied.
///
/// A sixth *row* is caught unconditionally: [`PresentedRow`] is matched on in every arm.
///
/// Pure: no I/O, no clock, no state. Resolving what the call presents and resolving the peer's
/// provenance both happen before it, so this function is a lookup and can be read line by line
/// against [`PresentedRow`] and [`PeerProvenance`].
///
/// # Errors
///
/// [`Refusal::Unauthorized`] or [`Refusal::Token`], per the cell.
pub fn classify(row: PresentedRow, provenance: PeerProvenance) -> Result<PeerRole, Refusal> {
    use PeerProvenance as Col;
    use PresentedRow as Row;

    match (row, provenance) {
        // Row 1 — a valid operator credential.
        (Row::OperatorCredential, Col::NoRecordedAgent | Col::Unresolvable) => {
            Ok(PeerRole::Operator)
        }
        (Row::OperatorCredential, Col::Agent(_)) => Err(Refusal::unauthorized(REFUSED)),

        // Row 2 — a token whose hash is a non-retired minting for `agent`.
        (Row::MintedToken(agent), Col::Agent(owner)) if agent == owner => {
            Ok(PeerRole::Agent { agent })
        }
        (Row::MintedToken(_), Col::Agent(_) | Col::NoRecordedAgent | Col::Unresolvable) => {
            Err(Refusal::unauthorized(REFUSED))
        }

        // The retired-agent row — a token minted for `agent`, retired while a recorded child still runs.
        (Row::RetiredToken(agent), Col::Agent(owner)) if agent == owner => {
            Ok(PeerRole::Agent { agent })
        }
        (Row::RetiredToken(_), Col::Agent(_) | Col::NoRecordedAgent | Col::Unresolvable) => {
            Err(Refusal::Token)
        }

        // Row 3 — a token matching no minting, or a retired agent's that has no recorded child.
        (Row::UnmintedToken, _) => Err(Refusal::Token),

        // Row 4 — nothing.
        (Row::Nothing, _) => Ok(PeerRole::Unproven),
    }
}

/// The one authorisation function every dispatch passes through.
///
/// One function, not a check per handler: a per-handler check is a rule that a new handler can forget, and the whole point
/// of a seam is that forgetting is not available.
///
/// The two inputs stay separate on purpose. `presented` is what the sender controls; `provenance`
/// is what the daemon resolved. An implementation that merges them into one "identity" before
/// deciding has thrown away the distinction invariant 8 is made of.
///
/// **The provenance arrives resolved, and this seam cannot resolve it again.** It is resolved once
/// per connection in [`serve`](crate::serve)'s frame and handed here by value; no
/// [`PeerIdentity`] reaches an implementation, so there is nothing for one to re-resolve. See
/// [`PeerResolver`] for what that does and does not close.
pub trait Authorizer: fmt::Debug + Send + Sync {
    /// Resolve this call to a role, or refuse it.
    ///
    /// `method` is the name as it arrived, before this crate has checked that any such method
    /// exists. That order is deliberate, and what it buys depends on the implementation: under
    /// [`PingOnly`] a peer may call nothing but `system.ping` and learns nothing else, while under
    /// [`PeerClassifier`] a peer that presents nothing resolves to `Unproven` — a role, not a
    /// closed door — and reaches the lookup for every name. So the enumeration residual is real
    /// under classification, and the [dispatch module](crate::dispatch) states it once, where the
    /// order that produces it lives. It is not restated here.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the call may not proceed.
    fn authorize(
        &self,
        presented: Presented,
        provenance: PeerProvenance,
        method: &str,
    ) -> Result<PeerRole, Refusal>;
}

/// Peer classification, wired to a token projection and handed a resolved provenance.
///
/// This is the seam that classifies a peer. It holds the daemon's operator credential, compares a
/// presented one against it in constant time, asks [`MintedTokens`] which agent a presented token
/// belongs to, and hands that answer — with the provenance its caller resolved — to [`classify`],
/// which is where every cell is decided.
///
/// # It picks the row; it is given the column
///
/// [`PeerClassifier::row`] is a function of the presented value, `self.credential` and
/// `self.tokens`: it takes no provenance and holds no [`PeerResolver`]. So whatever chooses the
/// provenance moves a call **along a row** and never **between rows** — and rows 3 and 4 are
/// column wildcards, while in the three rows where the column decides, the peer has already
/// presented a credential this daemon itself recognised. `crates/dudes-rpc/tests/peer_classification_cells.rs`
/// pins that property, with a control over those three rows that fails on a `classify` ignoring
/// its second argument.
///
/// **`row()` is evaluated on every call, and that is a precondition rather than a style note**.
/// [`MintedTokens::owner_of`] stops answering [`TokenOwner::Live`] for a
/// retired agent, and answers `None` once that agent's last recorded child has ended. So a
/// retirement demotes row 2 → the retired-agent row → row 3 on calls over a connection that is
/// already open. An implementation that resolved the [`PeerRole`] once per connection instead
/// would keep the suite green and silently break agent-token revocation.
///
/// # The method name is not an input
///
/// [`Authorizer::authorize`] receives one and this implementation ignores it, which is not an
/// oversight: the classification table is a function of what the call presents and what the peer resolves to, and
/// nothing else. Which roles a *method* admits is that method's `Requirement`, checked after lookup at the method's
/// own registration. A classifier that read the method name would be a second copy of those requirements.
#[derive(Debug)]
pub struct PeerClassifier {
    credential: OperatorCredential,
    tokens: Arc<dyn MintedTokens>,
}

impl PeerClassifier {
    /// A classifier holding this daemon's operator credential.
    ///
    /// The credential is **given, never minted here**: this crate has no randomness dependency and
    /// is not gaining one. The 32 bytes are minted beside the agent token's, in the one place a
    /// daemon's randomness lives, and are handed here and to
    /// [`LocalTransport::bind_in_with_credential`](crate::LocalTransport::bind_in_with_credential),
    /// which is what writes the file.
    #[must_use]
    pub fn new(credential: OperatorCredential, tokens: Arc<dyn MintedTokens>) -> Self {
        Self { credential, tokens }
    }

    /// Which of the classification table's four rows this call presents.
    ///
    /// # A call presenting both
    ///
    /// The table's rows are exclusive, so a call carrying **both** a credential and a token is
    /// outside it and an implementation has to choose. The choice here is the token.
    ///
    /// **It is not the narrower choice, and no comment here may say it is.** From a peer the
    /// daemon placed in agent A's tree, a minted token resolves to `Agent(A)` where the credential
    /// alone would have been row 1 column 1 — refused. So token-first is *wider* in that cell, not
    /// narrower; and `Operator` and `Agent` are not comparable in the first place, which
    /// [`Requirement`] already says.
    ///
    /// The reason precedence is safe is simpler and does not depend on which cell is wider: **a
    /// caller holding both secrets can present either one alone.** Both calls are already
    /// available to it, so precedence cannot change what that caller can *reach* — it only decides
    /// which of two calls it could have made anyway this one is. What precedence would change is
    /// the reach of a caller holding exactly one secret, and such a caller presents one thing and
    /// never lands here.
    ///
    /// # The consequence: token-first makes row 3's refusal evadable
    ///
    /// Row 3 (`-32004 Refused{gate: token}`) is the signal a *stolen* credential is meant to be
    /// visible through, and this choice removes its positive direction for exactly the attacker it
    /// watches for. A peer holding a stolen operator credential **and** a valid token for the tree
    /// it is calling from presents both, resolves as `Agent(A)`, and produces no row-1 refusal and
    /// no row-3 refusal — an ordinary agent call. Credential-first would have produced the row 1
    /// column 1 refusal instead.
    ///
    /// That is accepted here rather than closed, because closing it means either refusing a call
    /// that presents both — a fifth answer the table does not have — or credential-first, which
    /// costs an honest agent whose client attached a credential its own methods. But any rule
    /// built on "a row 1 or row 3 refusal separates theft from ordinary use" has to
    /// carry this exception, and the place to state it is here rather than there.
    ///
    /// The choice is pinned by tests in both directions: the narrower cell *and* the wider one.
    ///
    /// A well-formed credential that is **not** this daemon's is classification-table row 4: the call presented
    /// nothing this daemon recognises. There is no row for a wrong credential and no
    /// `RefusalGate` variant that could spell one.
    fn row(&self, presented: &Presented) -> PresentedRow {
        if let Some(token) = presented.token.as_ref() {
            return match self.tokens.owner_of(token) {
                Some(TokenOwner::Live(agent)) => PresentedRow::MintedToken(agent),
                Some(TokenOwner::RetiredWithChild(agent)) => PresentedRow::RetiredToken(agent),
                None => PresentedRow::UnmintedToken,
            };
        }
        match presented.cred.as_ref() {
            Some(cred) if *cred == self.credential => PresentedRow::OperatorCredential,
            _ => PresentedRow::Nothing,
        }
    }
}

impl Authorizer for PeerClassifier {
    fn authorize(
        &self,
        presented: Presented,
        provenance: PeerProvenance,
        _method: &str,
    ) -> Result<PeerRole, Refusal> {
        classify(self.row(&presented), provenance)
    }
}

/// The seam a daemon runs before it has a credential, a roster or a resolver: every peer is
/// [`PeerRole::Unproven`], and the one method that admits `Unproven` is
/// admitted.
///
/// It is **not** the classifier and does not implement the classification table. It is the policy that is *correct
/// while empty*: it needs no operator credential, no token projection and no provenance, and it
/// grants nothing that peer classification would later have to take away. A daemon that has
/// finished starting up uses [`PeerClassifier`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PingOnly;

impl Authorizer for PingOnly {
    fn authorize(
        &self,
        _presented: Presented,
        _provenance: PeerProvenance,
        method: &str,
    ) -> Result<PeerRole, Refusal> {
        if method == Method::SystemPing.name() {
            Ok(PeerRole::Unproven)
        } else {
            Err(Refusal::unauthorized(
                "this peer may call only system.ping; this daemon has no classifier yet",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use dudes_proto::rpc::hook::HOOK_TOKEN_LEN;
    use serde_json::json;

    use super::*;
    use crate::testing::FakeMintedTokens;

    fn agent(text: &str) -> AgentId {
        AgentId::parse(text).expect("a ulid")
    }

    fn alpha() -> AgentId {
        agent("01ARZ3NDEKTSV4RRFFQ69G5FAV")
    }

    fn credential() -> OperatorCredential {
        OperatorCredential::new("C".repeat(43)).expect("a credential")
    }

    /// The default seam admits the one method it is meant to and refuses the rest — asserted with
    /// both arms, because "refuses everything" and "admits everything" are both wrong and only one
    /// arm cannot tell them apart.
    #[test]
    fn ping_only_admits_ping_and_refuses_everything_else() {
        let column = PeerProvenance::Unresolvable;
        assert!(matches!(
            PingOnly.authorize(Presented::default(), column, "system.ping"),
            Ok(PeerRole::Unproven)
        ));
        for method in ["system.shutdown", "agent.hire", "hook.event", "nope"] {
            assert!(
                PingOnly
                    .authorize(Presented::default(), column, method)
                    .is_err(),
                "`{method}` must be refused by the default seam"
            );
        }
    }

    /// Presenting a token changes nothing under the default seam: it verifies no token, so a token
    /// must not become a way past the very seam that is holding the hole open.
    #[test]
    fn a_presented_token_does_not_widen_the_default_seam() {
        let token = "a".repeat(HOOK_TOKEN_LEN);
        let presented = Presented::from_params(Some(&json!({ "token": token })));
        assert!(
            presented.token.is_some(),
            "positive control: the token was extracted, so the refusal below is about the seam \
             and not about a token that never arrived"
        );
        assert!(
            PingOnly
                .authorize(presented, PeerProvenance::Unresolvable, "hook.event")
                .is_err()
        );
    }

    /// Extraction is lenient by design: a malformed token presents as nothing, so it reaches the
    /// seam as an unproven identity and is refused there (invariant 8).
    #[test]
    fn a_malformed_token_presents_as_nothing() {
        for params in [
            json!({ "token": "too-short" }),
            json!({ "token": 7 }),
            json!({ "raw": {} }),
            json!([]),
        ] {
            assert!(
                Presented::from_params(Some(&params)).token.is_none(),
                "{params} must present no token"
            );
        }
        assert!(Presented::from_params(None).token.is_none());
    }

    /// A peer the transport could say nothing about is representable, and it is the least
    /// privileged answer rather than an absent one.
    #[test]
    fn an_unknown_peer_is_representable_as_unreadable() {
        let unknown = PeerIdentity::unknown();
        assert_eq!(unknown.uid, None);
        assert_eq!(unknown.gid, None);
        assert_eq!(unknown.pid, PeerPid::Unreadable);
        assert_eq!(unknown, PeerIdentity::default());
        assert_eq!(unknown.pid.reported(), None);
        assert_eq!(
            PeerPid::Reported {
                pid: 42,
                pidversion: None
            }
            .reported(),
            Some(42),
            "positive control: a reported pid is reported"
        );
    }

    /// A call presenting both a credential and a token resolves on the token — from a peer in no
    /// agent's tree, where the token is unminted and the cell it reaches is **narrower**.
    ///
    /// Both arms: the same credential alone reaches `Operator` from the same peer, so what the
    /// token changed is the row and not the column.
    ///
    /// This is the flattering direction, and on its own it is the evidence for a claim the seam
    /// does not make — see [`presenting_both_from_inside_an_agents_tree_is_wider_not_narrower`]
    /// for the other one.
    #[test]
    fn presenting_both_a_credential_and_a_token_resolves_on_the_token() {
        let classifier = PeerClassifier::new(credential(), Arc::new(FakeMintedTokens::none()));
        let column = PeerProvenance::NoRecordedAgent;
        let token = HookToken::new("b".repeat(HOOK_TOKEN_LEN)).unwrap();

        let both = Presented {
            token: Some(token),
            cred: Some(credential()),
        };
        assert_eq!(
            classifier.authorize(both, column, "loop.pause"),
            Err(Refusal::Token),
            "the token names the row"
        );

        let credential_only = Presented {
            token: None,
            cred: Some(credential()),
        };
        assert_eq!(
            classifier.authorize(credential_only, column, "loop.pause"),
            Ok(PeerRole::Operator),
            "positive control: the same credential from the same peer is an operator"
        );
    }

    /// The other direction of the same precedence, and the one that says what it costs: from a
    /// peer the daemon **placed inside agent A's tree**, presenting a minted token beside the
    /// daemon's own credential is `Agent(A)` — where the credential alone is classification-table row 1 column
    /// 1, refused.
    ///
    /// So token-first is *wider* in this cell, not narrower, and no comment may call it the
    /// fail-closed choice. Its consequence is that a peer holding a stolen credential and a valid
    /// token for the tree it calls from produces no row-1 refusal — the refusal a stolen
    /// credential is meant to be visible through.
    ///
    /// The control is the credential-alone arm from the *same* peer and the same classifier: it
    /// refuses, so the `Ok` above is the token's doing and not a classifier that admits everything.
    #[test]
    fn presenting_both_from_inside_an_agents_tree_is_wider_not_narrower() {
        let text = "h".repeat(HOOK_TOKEN_LEN);
        let classifier = PeerClassifier::new(
            credential(),
            Arc::new(FakeMintedTokens::minted(&[(&text, alpha())])),
        );
        let column = PeerProvenance::Agent(alpha());

        let both = Presented {
            token: Some(HookToken::new(&text).unwrap()),
            cred: Some(credential()),
        };
        assert_eq!(
            classifier.authorize(both, column, "loop.pause"),
            Ok(PeerRole::Agent { agent: alpha() }),
            "the token names the row, and this row admits where row 1 would not"
        );

        let credential_only = Presented {
            token: None,
            cred: Some(credential()),
        };
        assert_eq!(
            classifier.authorize(credential_only, column, "loop.pause"),
            Err(Refusal::unauthorized(REFUSED)),
            "control: the same credential from the same peer is classification-table row 1 column 1 — refused, which is what makes the arm above wider rather than narrower"
        );
    }

    /// A well-formed credential that is not this daemon's presents nothing this daemon recognises
    /// — classification-table row 4 — rather than reaching row 1 or inventing a refusal the contract cannot
    /// spell.
    #[test]
    fn an_unrecognised_credential_presents_nothing() {
        let classifier = PeerClassifier::new(credential(), Arc::new(FakeMintedTokens::none()));
        let wrong = OperatorCredential::new("D".repeat(43)).unwrap();
        assert_eq!(
            classifier.row(&Presented {
                token: None,
                cred: Some(wrong)
            }),
            PresentedRow::Nothing
        );
        assert_eq!(
            classifier.row(&Presented {
                token: None,
                cred: Some(credential())
            }),
            PresentedRow::OperatorCredential,
            "positive control: the daemon's own credential is recognised"
        );
    }

    /// The classifier's row resolution asks the projection rather than deciding for itself: a live
    /// agent the projection names becomes row 2, a retired one with a recorded child becomes
    /// The retired-agent row, and a token it does not name becomes row 3.
    #[test]
    fn the_row_of_a_token_is_whatever_the_projection_says() {
        let text = "e".repeat(HOOK_TOKEN_LEN);
        let presented = || Presented {
            token: Some(HookToken::new(&text).unwrap()),
            cred: None,
        };
        let known = PeerClassifier::new(
            credential(),
            Arc::new(FakeMintedTokens::minted(&[(&text, alpha())])),
        );
        assert_eq!(known.row(&presented()), PresentedRow::MintedToken(alpha()));
        let retired = PeerClassifier::new(
            credential(),
            Arc::new(FakeMintedTokens::retired_with_child(&[(&text, alpha())])),
        );
        assert_eq!(
            retired.row(&presented()),
            PresentedRow::RetiredToken(alpha())
        );
        let unknown = PeerClassifier::new(credential(), Arc::new(FakeMintedTokens::none()));
        assert_eq!(
            unknown.row(&Presented {
                token: Some(HookToken::new(&text).unwrap()),
                cred: None
            }),
            PresentedRow::UnmintedToken,
            "the same token is row 3 when the projection names no agent for it"
        );
    }

    /// A credential on the envelope is what `Presented` reads it from, and params alone present
    /// none — the two halves of invariant 8's presented side stay in the two places the contract
    /// puts them.
    #[test]
    fn the_credential_comes_from_the_envelope_and_the_token_from_the_params() {
        let text = "F".repeat(43);
        let request = Request {
            jsonrpc: dudes_proto::rpc::JSONRPC.to_owned(),
            id: dudes_proto::rpc::RequestId::Number(1),
            method: "loop.pause".to_owned(),
            params: Some(json!({ "token": "g".repeat(HOOK_TOKEN_LEN) })),
            v: None,
            cred: Some(OperatorCredential::new(&text).unwrap()),
        };
        let presented = Presented::from_request(&request);
        assert_eq!(
            presented.cred.as_ref().map(OperatorCredential::expose),
            Some(text.as_str())
        );
        assert!(presented.token.is_some(), "positive control");
        assert!(
            Presented::from_params(request.params.as_ref())
                .cred
                .is_none(),
            "params carry no credential"
        );
    }
}
