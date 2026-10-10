//! `system.*` results.
//!
//! Two of these shapes carry a property beyond their fields, and both are pinned here at compile
//! time rather than asserted in prose. They live in this header and not on the types because every
//! item doc comment in this file is exported verbatim into `schemas/rpc.schema.json`, and a
//! `compile_fail` fence is test scaffolding rather than something a schema consumer should read.
//!
//! [`AgentHealth`] is `Copy`, and it is `Copy` because the per-open-turn listing is a sibling
//! collection on [`DoctorResult`] rather than a `Vec` nested in the agent row. A
//! double move by value is the check:
//!
//! ```
//! # use dudes_proto::rpc::system::AgentHealth;
//! fn takes(_: AgentHealth) {}
//! fn twice(row: AgentHealth) {
//!     takes(row);
//!     takes(row);
//! }
//! ```
//!
//! [`OpenTurnHealth`] is not `Copy` — `boot` owns a `String` — and that is the control for the
//! check above rather than an accident of the fields. One move of it compiles:
//!
//! ```
//! # use dudes_proto::rpc::system::OpenTurnHealth;
//! fn takes(_: OpenTurnHealth) {}
//! fn once(row: OpenTurnHealth) {
//!     takes(row);
//! }
//! ```
//!
//! and a second one does not, which is what makes the pair a `Copy` check rather than two names
//! that failed to resolve:
//!
//! ```compile_fail
//! # use dudes_proto::rpc::system::OpenTurnHealth;
//! fn takes(_: OpenTurnHealth) {}
//! fn twice(row: OpenTurnHealth) {
//!     takes(row);
//!     takes(row);
//! }
//! ```

use std::path::PathBuf;

use dudes_types::{AgentId, HostBoot, Liveness, Lsn, Provider, TurnId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `system.ping` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PingResult {
    /// Daemon version.
    pub version: String,
    /// The socket path (or named pipe) the daemon answers on.
    pub socket: PathBuf,
    /// The journal's current lsn.
    pub lsn: Lsn,
}

/// `system.shutdown` acknowledgment. Completion is observed separately after teardown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShutdownResult {
    /// Run-local completion receipt. Older daemons return no completion observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<ShutdownReceipt>,
}

/// The failure a shutdown receipt names when the host's only unresolved ownership is its
/// `ContainmentUnproven` error: every agent process exited, but nothing proves that none of its
/// own children left its process group or job. `dudes stop` warns and succeeds when this is the
/// receipt's whole failure list, and fails on any other list.
pub const SHUTDOWN_CONTAINMENT_UNPROVEN: &str = "managed child containment unproven";

/// Non-secret identity and path of one daemon run's shutdown receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShutdownReceipt {
    /// Absolute path chosen by the daemon in its protected runtime directory.
    pub path: PathBuf,
    /// Opaque run identity. A receipt for another run does not prove this run completed.
    pub run: String,
}

/// `system.doctor` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorResult {
    /// As `system.ping`.
    pub ping: PingResult,
    /// The boot identity this run stamps every spawn with, or the reason it has none — one field
    /// beside `ping`.
    ///
    /// **Why an operator is shown it at all.** The host reads this once at daemon
    /// start and stamps the whole run from the cached value, and the liveness table compares it against
    /// what each recorded key carries. A run whose read failed therefore answers
    /// [`Liveness::Unknown`] to every key that carries a boot identity, for the rest of that run
    /// and with nothing an operator can do to it — the turns stay open, a standing kill defers,
    /// and the loop makes no progress on those agents. That case is nearly unreachable;
    /// nearly unreachable is exactly the class of failure worth seeing before it bites rather than
    /// diagnosing afterwards, and this field is where it is seen.
    ///
    /// **Absent means this daemon did not report one** — a reply from before the field existed
    /// (the contract grows by addition) — and never a claim about the host. The three
    /// claims a host *can* make are the three variants, and [`HostBoot`] says why collapsing any
    /// two of them is the defect rather than a simplification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<HostBoot>,
    /// One row per provider the daemon probed.
    #[serde(default)]
    pub providers: Vec<ProviderProbe>,
    /// One row per hired agent.
    #[serde(default)]
    pub agents: Vec<AgentHealth>,
    /// One row per open turn, keyed by `TurnId` — a sibling of `agents`, not nested inside it.
    /// The state that answers this question is keyed by turn end to end, so this
    /// collection is a projection of it rather than a join through `agents`, and a turn whose
    /// agent row is missing or retired is listed like any other. That is the turn most worth
    /// seeing.
    ///
    /// **Nothing may decide on this collection**. It is runtime state derived from
    /// the probe exchange and the open turns, lost on restart and re-asked on the next start; a
    /// `Done` or a `Stop` taken from it would be an `Inferred` fact reaching a verdict, which
    /// invariant 3 forbids. It is an operator's view and nothing else.
    #[serde(default)]
    pub open_turns: Vec<OpenTurnHealth>,
    /// One row per out-of-line body whose file is absent, shorter than its recorded decoded
    /// length, or differs from its recorded BLAKE3. Diagnostic only: this collection repairs
    /// nothing and changes no replay disposition.
    #[serde(default)]
    pub body_integrity: Vec<BodyIntegrity>,
}

/// Additive operator mailbox diagnostics beside the unchanged doctor result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorMailboxResult {
    /// Existing doctor fields, serialized at the same level.
    #[serde(flatten)]
    pub doctor: DoctorResult,
    /// Bounded mailbox observations. Missing on older servers.
    #[serde(default)]
    pub mailbox: MailboxReport,
}

/// Bounded observations from the daemon's mailbox worker.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MailboxReport {
    /// Whether this platform has a filesystem mailbox worker.
    pub available: bool,
    /// A fixed platform availability explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// At most 128 observations, with filename labels limited to 128 UTF-8 bytes.
    #[schemars(length(max = 128))]
    pub diagnostics: Vec<MailboxDiagnostic>,
    /// Whether further observations were omitted.
    pub truncated: bool,
}

/// One deferred mailbox file or bounded scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MailboxDiagnostic {
    /// The structural directory owner.
    pub agent: dudes_types::AgentId,
    /// A filename label, never decoded content.
    #[schemars(length(max = 128))]
    pub filename: String,
    /// A fixed diagnosis.
    pub problem: MailboxProblem,
}

/// Why mailbox observation deferred an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MailboxProblem {
    /// No original mail belongs to this marker.
    UnmatchedAcknowledgement,
    /// Marker content is malformed or exceeds the decode bound.
    MalformedMarker,
    /// Canonical filename or single-file identity could not be proved.
    AmbiguousMarker,
    /// The marker stands in another recipient's directory.
    RecipientMismatch,
    /// More raw entries exist than the complete observation bound.
    ScanOverflow,
    /// An outbox reused an already recorded mail id.
    ReusedId,
    /// The outbox item is invalid or has uncertain identity.
    OutboxCorruption,
    /// A deterministic delivery or body filename contains different bytes.
    DeliveryCollision,
}

/// Additive operator-only journal diagnostic beside the existing doctor fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorJournalResult {
    /// The existing doctor result, with its construction API unchanged.
    #[serde(flatten)]
    pub doctor: DoctorResult,
    /// The attempted lsn of a stopped writer. Absence reports no stopped state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_stopped: Option<super::JournalStoppedData>,
}

/// What `system.doctor` found for one out-of-line body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BodyIntegrity {
    /// The body path as the record names it, relative to the data directory.
    pub path: PathBuf,
    /// Why the referenced file failed its recorded integrity check.
    pub problem: BodyIntegrityProblem,
}

/// Why one referenced body failed its integrity check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyIntegrityProblem {
    /// No file exists at the recorded path.
    Missing,
    /// The file is shorter than the decoded length recorded beside it.
    Truncated,
    /// The file's BLAKE3 differs from the digest recorded beside it.
    HashMismatch,
}

/// What `system.doctor` found for one provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderProbe {
    /// The provider.
    pub provider: Provider,
    /// Where its command resolved on PATH, if anywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<PathBuf>,
    /// What `--version` said, if it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether the daemon found the program on its `PATH`: a regular file it may execute, the one
    /// the next launch runs. It proves a file, not a working turn.
    pub ok: bool,
    /// Anything the operator should know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the engine's own status command said about its sign-in, read from its exit status
    /// alone. Absent when the daemon did not report one. Diagnostic only: it carries
    /// no provenance and decides nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignIn>,
    /// The running daemon's managed login home. Absent for the legacy provider home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_home: Option<ProviderLoginHome>,
}

/// Whether the running daemon can admit a provider's dedicated login home.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProviderLoginHome {
    /// The profile passed the daemon's home and machine-policy checks.
    Ready {
        /// Absolute path selected by the running daemon.
        path: PathBuf,
    },
    /// Login and sign-in checks must not fall back to another home.
    Unavailable {
        /// A diagnostic without credential contents.
        reason: String,
    },
}

/// Whether an engine is signed in, as its status command's exit status says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignIn {
    /// The status command exited 0.
    SignedIn,
    /// The status command exited 1.
    SignedOut,
    /// Any other answer, or no answer; the row's `detail` says which.
    Unknown,
}

/// What `system.doctor` found for one agent.
///
/// `Copy`, and that derive is load-bearing rather than incidental: the per-open-turn listing is a
/// sibling collection on [`DoctorResult`] instead of a `Vec` nested in this row, precisely so this
/// row keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentHealth {
    /// The agent.
    pub agent: AgentId,
    /// The lsn of the last `SessionStart` event, or `None` for never — a wire that delivers
    /// every other event but not this one is a mis-installed shim, not a loop condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_start: Option<Lsn>,
    /// The managed turn whose unconfirmed resume caused the current fresh-session recovery.
    /// Diagnostic only: it records no provenance and is read by no launch or loop decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unconfirmed_resume: Option<TurnId>,
    /// Whether a `TurnSpawned` without `TurnEnded` is being watched by the reaper.
    #[serde(default)]
    pub adopted_child: bool,
}

/// What `system.doctor` found for one open turn: a turn the daemon recorded as spawned and has
/// not closed. The seven fields of the per-turn report, plus the one thing that asked:
/// [`key_uninterpretable`](Self::key_uninterpretable), which is decided from the host's boot
/// identity without a probe.
///
/// Why the row exists at all. Invariant 3 forbids synthesising a `Gone` out of a probe the host
/// could not answer, so a turn whose liveness answer stays `Unknown` stays open indefinitely.
/// That price is accepted on one stated condition — that the operator can
/// see the stuck turn — and there is a shipped platform on which the
/// `Unknown` answer is the ordinary outcome rather than an edge. This row is that condition, and
/// it is the whole of it: see [`DoctorResult::open_turns`] for what may read it, which is nothing.
///
/// Not `Copy`, because `boot` owns a `String`. That is the control for [`AgentHealth`]'s derive
/// rather than an accident of the fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenTurnHealth {
    /// The turn. The collection is keyed by it.
    pub turn: TurnId,
    /// The agent that recorded the turn. A label for the reader, never a join key: the row is
    /// listed whether or not that agent still has a row in `agents`.
    pub agent: AgentId,
    /// The child's pid, as recorded on `TurnSpawned`.
    pub pid: u32,
    /// The platform-native start value, opaque: the value the host read, carried as read, never
    /// converted and never rendered as a time.
    pub start_time: u64,
    /// The boot identity of the machine the child started on, where the start value is
    /// boot-relative. Opaque: compared for equality, never parsed. `None` asserts the start value
    /// is wall-clock-anchored, and never "boot-relative but unreadable".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<String>,
    /// What the host last answered about this turn's adoption key.
    pub liveness: Liveness,
    /// How long the current run of `Unknown` answers has lasted, in milliseconds.
    ///
    /// `Some` only while `liveness` is [`Liveness::Unknown`], and it is the age of the **first**
    /// `Unknown` in the current run, never of the latest one. The distinction is the
    /// whole value of the field: an age taken from the latest answer would reset on every probe
    /// and report a permanently stuck turn as a fresh one for ever. An answer that leaves
    /// `Unknown` clears the stamp, so a later run of `Unknown`s is aged from its own first answer.
    ///
    /// The supervisor's own run-local liveness state carries that first stamp. It is in-memory
    /// state and neither a record nor a replay obligation: restart clears it, and the next run
    /// reports no age until its first `Unknown` answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_for_ms: Option<u64>,
    /// This host cannot interpret the turn's adoption key **at all**, so no probe of it can ever
    /// answer anything but [`Liveness::Unknown`] while this run lasts.
    ///
    /// It is the diagonal of the liveness table — the key and this host disagree about whether a
    /// boot identity *exists*, either because the journal was written on another platform or
    /// because this boot-relative host's own read failed — decided by the one predicate
    /// `dudes_supervisor::host::key_is_interpretable` that `liveness` itself reads, so the marked
    /// set and the stuck set are the same set by construction rather than by agreement.
    ///
    /// **Not a synonym for `liveness == Unknown`, and that is the whole of its value.** A host that
    /// could not read the process table this second answers `Unknown` too and recovers by itself;
    /// this one does not recover, and no action available to the operator on this machine resolves
    /// it. `liveness` says what the last probe answered, and this says whether asking again can
    /// ever answer differently.
    ///
    /// **A property of the pair, not of the row.** The same recorded key is uninterpretable on one
    /// host and ordinary on the next, so this value is meaningful only beside the
    /// [`boot`](DoctorResult::boot) of the same reply and must never be stored as if it were a
    /// fact about the turn.
    ///
    /// `#[serde(default)]`: absent reads as `false`, which is a reply that reported no problem
    /// rather than one asserting there is none.
    #[serde(default)]
    pub key_uninterpretable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_unconfirmed_resume_defaults_to_none_and_is_omitted() {
        let without = serde_json::json!({
            "agent": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "adopted_child": false
        });
        let row: AgentHealth = serde_json::from_value(without).unwrap();
        assert_eq!(row.unconfirmed_resume, None);

        let json = serde_json::to_value(row).unwrap();
        assert_eq!(json.get("unconfirmed_resume"), None);

        let turn = TurnId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let with = AgentHealth {
            unconfirmed_resume: Some(turn),
            ..row
        };
        assert_eq!(
            serde_json::to_value(with).unwrap()["unconfirmed_resume"],
            "01ARZ3NDEKTSV4RRFFQ69G5FAW"
        );
    }

    fn open_turn() -> OpenTurnHealth {
        OpenTurnHealth {
            turn: TurnId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap(),
            agent: AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
            pid: 4321,
            start_time: 87_654_321,
            boot: Some("f0e1d2c3".to_owned()),
            liveness: Liveness::Unknown,
            unknown_for_ms: Some(2_000),
            key_uninterpretable: true,
        }
    }

    /// All eight wire names, asserted literally, and then the values through a round trip.
    ///
    /// The literal assertions are the half that carries the *names*. A symmetric serialise →
    /// deserialise trip cannot see a rename at all, because both directions move together: a
    /// `#[serde(rename)]` on any field leaves the round trip green. `OpenTurnHealth` is a wire
    /// shape and growth by addition presumes the names already on it are fixed, so
    /// every one of them is named here rather than three of them.
    ///
    /// `start_time` stays an integer — it is the value the host read, never a rendered time
    /// — and `liveness` with `unknown_for_ms` is a stuck turn's whole answer.
    #[test]
    fn an_open_turn_round_trips() {
        let row = open_turn();
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["turn"], "01ARZ3NDEKTSV4RRFFQ69G5FAW");
        assert_eq!(json["agent"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(json["pid"], 4321);
        assert_eq!(json["start_time"], 87_654_321_u64);
        assert_eq!(json["boot"], "f0e1d2c3");
        assert_eq!(json["liveness"], "unknown");
        assert_eq!(json["unknown_for_ms"], 2_000_u64);
        assert_eq!(json["key_uninterpretable"], true);
        assert_eq!(
            serde_json::from_value::<OpenTurnHealth>(json).unwrap(),
            row,
            "and every value survives the trip"
        );
    }

    /// The two optional fields are absent rather than `null` when they are `None`, so a row for a
    /// turn that is answering is not padded with the shape of one that is stuck.
    ///
    /// `key_uninterpretable` is **not** one of them: it is written whether or not it is set, so a
    /// reader never has to tell "this host can read the key" apart from "this daemon did not say".
    /// The absent-is-`false` default is for a reply from before the field, not for this one.
    #[test]
    fn absent_options_are_omitted() {
        let row = OpenTurnHealth {
            boot: None,
            liveness: Liveness::Alive,
            unknown_for_ms: None,
            key_uninterpretable: false,
            ..open_turn()
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json.get("boot"), None);
        assert_eq!(json.get("unknown_for_ms"), None);
        assert_eq!(json["key_uninterpretable"], false);
        // The control: the fields that are always present are still there, so the assertion above
        // is about `skip_serializing_if` and not about a serialisation that produced nothing.
        assert_eq!(json["liveness"], "alive");
        assert_eq!(json["pid"], 4321);
    }

    /// A `DoctorResult` that names no additive diagnostic field parses, and every one of them
    /// reads as *not reported*: the contract grows by addition, so an earlier reply
    /// still parses and none of its silences turns into a claim.
    ///
    /// `boot` is the field where that distinction has teeth. Its `None` is "this daemon did not
    /// say", and the three things a daemon *can* say are the three [`HostBoot`] variants — so a
    /// default of `HostBoot::NotApplicable` would have an old reply asserting the host anchors its
    /// start values at creation, which is a claim about the machine that nobody made.
    #[test]
    fn additive_diagnostic_collections_default_to_empty() {
        let without = serde_json::json!({
            "ping": { "version": "0.0.0", "socket": "/tmp/duded.sock", "lsn": 0 },
        });
        let result: DoctorResult = serde_json::from_value(without.clone()).unwrap();
        assert!(result.open_turns.is_empty());
        assert!(result.agents.is_empty());
        assert!(result.body_integrity.is_empty());
        assert_eq!(result.boot, None);

        // The control: the same document with the collection and the field present carries them,
        // so the assertions above measure the defaults rather than fields the parser drops.
        let mut with = without;
        with["open_turns"] = serde_json::json!([open_turn()]);
        with["boot"] = serde_json::json!({ "read": "f0e1d2c3" });
        let result: DoctorResult = serde_json::from_value(with).unwrap();
        assert_eq!(result.open_turns, vec![open_turn()]);
        assert_eq!(result.boot, Some(HostBoot::Read("f0e1d2c3".to_owned())));
    }

    /// The run's boot identity round trips under each of the three things a host can say, and an
    /// absent field is not any of them.
    ///
    /// The two failure states are asserted apart. A doctor that reported `Option<String>` would
    /// send the same document for a macOS host and for a Linux host whose `/proc` read failed —
    /// the second being the run in which every adopted turn is stuck for good — and the operator
    /// reading it could not tell which machine they were on.
    #[test]
    fn the_runs_boot_identity_distinguishes_a_failed_read_from_a_platform_without_one() {
        let doc = |boot: &HostBoot| {
            let result = DoctorResult {
                ping: PingResult {
                    version: "0.0.0".to_owned(),
                    socket: PathBuf::from("/tmp/duded.sock"),
                    lsn: Lsn::ZERO,
                },
                providers: Vec::new(),
                agents: Vec::new(),
                open_turns: Vec::new(),
                body_integrity: Vec::new(),
                boot: Some(boot.clone()),
            };
            serde_json::to_value(&result).unwrap()["boot"].clone()
        };

        let unreadable = HostBoot::Unreadable("permission denied".to_owned());
        assert_eq!(
            doc(&HostBoot::NotApplicable),
            serde_json::json!("not_applicable")
        );
        assert_eq!(
            doc(&HostBoot::Read("f0e1d2c3".to_owned())),
            serde_json::json!({ "read": "f0e1d2c3" })
        );
        assert_eq!(
            doc(&unreadable),
            serde_json::json!({ "unreadable": "permission denied" })
        );
        assert_ne!(doc(&unreadable), doc(&HostBoot::NotApplicable));
    }

    #[test]
    fn a_body_integrity_row_uses_the_published_field_names() {
        let json = serde_json::json!({
            "ping": { "version": "0.0.0", "socket": "/tmp/duded.sock", "lsn": 0 },
            "body_integrity": [{
                "path": "agents/a/mail/b.body",
                "problem": "hash_mismatch",
            }],
        });

        let result: DoctorResult = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            result.body_integrity,
            vec![BodyIntegrity {
                path: PathBuf::from("agents/a/mail/b.body"),
                problem: BodyIntegrityProblem::HashMismatch,
            }]
        );
        assert_eq!(
            serde_json::to_value(result).unwrap()["body_integrity"],
            json["body_integrity"]
        );
    }
}

#[cfg(test)]
mod mailbox_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn additive_mailbox_wrapper_keeps_both_reader_directions() {
        let old = json!({"ping":{"version":"test", "socket":"/tmp/duded.sock", "lsn":0}, "providers":[], "agents":[], "open_turns":[], "body_integrity":[]});
        let wrapper: DoctorMailboxResult = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(wrapper.mailbox, MailboxReport::default());
        let mut new = old.clone();
        new["mailbox"] = json!({"available":true,"diagnostics":[],"truncated":false});
        let plain: DoctorResult = serde_json::from_value(new.clone()).unwrap();
        assert_eq!(serde_json::from_value::<DoctorResult>(old).unwrap(), plain);
        let wrapper: DoctorMailboxResult = serde_json::from_value(new.clone()).unwrap();
        assert!(wrapper.mailbox.available);
        assert_eq!(serde_json::to_value(wrapper).unwrap(), new);
    }
    #[test]
    fn combined_doctor_schema_and_readers_preserve_both_diagnostic_siblings() {
        let value = json!({"ping":{"version":"test", "socket":"/tmp/duded.sock", "lsn":0}, "providers":[], "agents":[], "open_turns":[], "body_integrity":[], "mailbox":{"available":true,"diagnostics":[],"truncated":false}, "journal_stopped":{"lsn":55}});
        let mail: DoctorMailboxResult = serde_json::from_value(value.clone()).unwrap();
        let journal: DoctorJournalResult = serde_json::from_value(value.clone()).unwrap();
        assert!(mail.mailbox.available);
        assert_eq!(journal.journal_stopped.unwrap().lsn, dudes_types::Lsn(55));
        assert_eq!(mail.doctor, journal.doctor);
        let doc = crate::schema::schemas()
            .into_iter()
            .find(|doc| doc.file_name == "rpc.schema.json")
            .unwrap();
        let schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema", "$ref":"#/$defs/DoctorResult", "$defs":doc.schema["$defs"]});
        assert!(
            doc.schema["$defs"]["DoctorResult"]["properties"]
                .get("mailbox")
                .is_some()
        );
        assert!(
            doc.schema["$defs"]["DoctorResult"]["properties"]
                .get("journal_stopped")
                .is_some()
        );
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
    }
}

#[cfg(test)]
mod login_home_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_login_home_preserves_legacy_omission_and_explicit_refusal() {
        let legacy = json!({"provider":"codex", "command":"codex", "ok":true});
        let probe: ProviderProbe = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(probe.login_home, None);
        assert!(
            serde_json::to_value(probe)
                .unwrap()
                .get("login_home")
                .is_none()
        );
        for home in [
            json!({"state":"ready", "path":"/data/providers/codex"}),
            json!({"state":"unavailable", "reason":"managed policy is present"}),
        ] {
            let mut value = legacy.clone();
            value["login_home"] = home.clone();
            let probe: ProviderProbe = serde_json::from_value(value).unwrap();
            assert_eq!(serde_json::to_value(probe).unwrap()["login_home"], home);
        }
    }
}
