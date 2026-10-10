//! Peer classification, one assertion per cell — the grid itself, on every platform.
//!
//! # Why this file is a grid and not sixteen prose tests
//!
//! The classification table is a grid, and a reader has to be able to check this against it without
//! reconstructing anything. So the grid is written out once, in row and column
//! order, and every named test below is a *second* look at a cell a reader
//! is most likely to want to find by name.
//!
//! # Two positive controls, not one
//!
//! Rows 1 and 2 are admitted by different mechanisms, and one control cannot cover both:
//!
//! - [`an_agent_token_from_that_agents_own_descendant_is_honoured`] — row 2, column 1. Without it
//!   the suite passes when everything is refused.
//! - [`an_operator_credential_from_an_unowned_peer_is_operator`] — row 1, column 3. Without it, the
//!   operator-credential refusal could be implemented as "refuse every operator credential" and the suite would still be
//!   green.
//!
//! Both controls live here, with the paragraph that explains why there are two of them. A control
//! separated from its explanation is one refactor away from being deleted as redundant.
//!
//! # What a named column cannot tell you
//!
//! Every column here is *named by the test*. That is the point of the seam — the alternative is a
//! process-spawning suite that is slow and flaky by construction — but it means
//! nothing in this file is evidence about the ancestry walk itself, which does not exist yet, or
//! about what macOS reports for a peer that handed its socket to a grandchild. Those are
//! `PeerResolver`'s, and its own documentation says what it owes. Nor is anything here evidence
//! that the column a *connection* is decided under is the one its peer resolved to: the column
//! reaches this argument from `serve`'s connection frame, and `tests/seam.rs` is what holds that.
//!
//! # The property beside the grid, and why it has a control
//!
//! [`the_column_cannot_move_a_call_between_rows`] states what makes it safe for a seam's owner to
//! let a *test* choose the column: `PeerClassifier::row` reads no provenance, so a column moves a
//! call along a row and never between rows. Its control is the half that makes it mean anything —
//! rows 1 and 2 and the retired-agent row must decide differently across the columns, or the
//! invariance is a property of a `classify` that ignores its second argument.
//!
//! # No `cfg`, deliberately — that is what this file is for
//!
//! Nothing below binds a socket. Every test calls [`classify`], or a [`PeerClassifier`] built over
//! one scripted port, so the whole file is platform-independent by construction and carries **no
//! crate-level `cfg`**. It must not acquire one: these cells were invisible on the Windows leg of
//! the matrix for exactly as long as they shared a file with the wire arm, which needs
//! `testing::TestServer` and therefore `#[cfg(unix)]`. That arm is `tests/peer_classification.rs`, and the
//! reason it is a separate file is this sentence.

use std::sync::Arc;

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::hook::{HOOK_TOKEN_LEN, HookToken};
use dudes_proto::rpc::{ErrorCode, RefusalGate, RefusedData};
use dudes_rpc::auth::{
    Authorizer, PeerClassifier, PeerProvenance, PeerRole, Presented, PresentedRow, Refusal,
    classify,
};
use dudes_rpc::testing::FakeMintedTokens;
use dudes_types::AgentId;

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

fn token(text: &str) -> HookToken {
    assert_eq!(
        text.len(),
        HOOK_TOKEN_LEN,
        "the fixture is a well-formed token"
    );
    HookToken::new(text).expect("a token")
}

fn credential() -> OperatorCredential {
    OperatorCredential::new(CREDENTIAL).expect("a credential")
}

/// A classifier holding this daemon's credential and a projection that mints [`ALPHA_TOKEN`] for
/// alpha and nothing else.
///
/// It holds no resolver: the column is [`Authorizer::authorize`]'s second argument, resolved by
/// whoever calls it, so a test names one rather than scripting one.
fn classifier() -> PeerClassifier {
    PeerClassifier::new(
        credential(),
        Arc::new(FakeMintedTokens::minted(&[(ALPHA_TOKEN, alpha())])),
    )
}

/// What a cell resolves to, flattened so the grid below can be read as a grid.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Operator,
    Agent(AgentId),
    Unproven,
    /// `-32001 Unauthorized`.
    Unauthorized,
    /// `-32004 Refused{gate: token}`.
    RefusedToken,
}

impl Verdict {
    fn of(result: &Result<PeerRole, Refusal>) -> Self {
        match result {
            Ok(PeerRole::Operator) => Self::Operator,
            Ok(PeerRole::Agent { agent }) => Self::Agent(*agent),
            Ok(PeerRole::Unproven) => Self::Unproven,
            Err(Refusal::Unauthorized(_)) => Self::Unauthorized,
            Err(Refusal::Token) => Self::RefusedToken,
        }
    }
}

/// The classification table's three columns, in the table's order — with column 3 present **twice**, because
/// two provenances land there and the table gives them one answer.
fn columns() -> [(&'static str, PeerProvenance); 4] {
    [
        (
            "col 1 · ancestry reaches agent A's recorded child",
            PeerProvenance::Agent(alpha()),
        ),
        (
            "col 2 · ancestry reaches a different agent's child",
            PeerProvenance::Agent(bravo()),
        ),
        (
            "col 3 · walked cleanly to no recorded agent child",
            PeerProvenance::NoRecordedAgent,
        ),
        (
            "col 3 · unresolvable, because the process was gone",
            PeerProvenance::Unresolvable,
        ),
    ]
}

/// Every cell of the classification table, in row
/// and column order, with the retired-agent row beside row 3.
///
/// The grid is `classify` alone: what a call presents has already been resolved to a row, and the
/// peer to a column, so this asserts the table and nothing else. That the *classifier* reaches the
/// right row and applies the column it is given is
/// [`the_classifier_reaches_the_row_and_applies_the_column_it_is_given`]; that the verdicts reach
/// the wire is the end-to-end arm in `tests/peer_classification.rs`.
#[test]
fn every_cell_of_the_classification_table_resolves_as_specified() {
    let rows: [(&str, PresentedRow, [Verdict; 4]); 5] = [
        (
            "row 1 · a valid operator credential",
            PresentedRow::OperatorCredential,
            [
                Verdict::Unauthorized,
                Verdict::Unauthorized,
                Verdict::Operator,
                Verdict::Operator,
            ],
        ),
        (
            "row 2 · a token whose hash is a non-retired minting for A",
            PresentedRow::MintedToken(alpha()),
            [
                Verdict::Agent(alpha()),
                Verdict::Unauthorized,
                Verdict::Unauthorized,
                Verdict::Unauthorized,
            ],
        ),
        (
            "The retired-agent row · a token minted for A, retired while a recorded child still runs",
            PresentedRow::RetiredToken(alpha()),
            [
                Verdict::Agent(alpha()),
                Verdict::RefusedToken,
                Verdict::RefusedToken,
                Verdict::RefusedToken,
            ],
        ),
        (
            "row 3 · a token matching no minting, or a retired agent's with no recorded child",
            PresentedRow::UnmintedToken,
            [
                Verdict::RefusedToken,
                Verdict::RefusedToken,
                Verdict::RefusedToken,
                Verdict::RefusedToken,
            ],
        ),
        (
            "row 4 · nothing",
            PresentedRow::Nothing,
            [
                Verdict::Unproven,
                Verdict::Unproven,
                Verdict::Unproven,
                Verdict::Unproven,
            ],
        ),
    ];

    for (row_name, row, expected) in rows {
        for ((column_name, provenance), expected) in columns().into_iter().zip(expected) {
            assert_eq!(
                Verdict::of(&classify(row, provenance)),
                expected,
                "{row_name} / {column_name}"
            );
        }
    }
}

/// **The property that licenses a test choosing the column: a resolver moves a call along a row
/// and never between rows.**
///
/// `PeerClassifier::row` is a function of the presented value, the daemon's credential and the
/// token projection. It takes no provenance, so whatever produces the column cannot change which
/// row a call is in — and the two rows that could admit a peer that proved nothing are column
/// wildcards: row 4 (`Nothing`) is `Unproven` in every column and row 3 (an unminted or
/// retired-agent token with no recorded child) is `-32004` in every column. In the three rows
/// where the column *does* decide, the peer has already presented a credential this daemon itself
/// recognised: the retired-agent row needs a token this daemon minted, as row 2 does.
///
/// **The control is the other half of the same test, and without it this passes on nothing.** A
/// `classify` that ignored its second argument entirely would satisfy the invariance above
/// perfectly, and it is exactly the mutant under which the property is false. So rows 1 and 2 and
/// The retired-agent row must each yield **more than one distinct verdict** across the same columns.
#[test]
fn the_column_cannot_move_a_call_between_rows() {
    let distinct = |row: PresentedRow| {
        let mut verdicts: Vec<Verdict> = Vec::new();
        for (_, provenance) in columns() {
            let verdict = Verdict::of(&classify(row, provenance));
            if !verdicts.contains(&verdict) {
                verdicts.push(verdict);
            }
        }
        verdicts
    };

    for row in [PresentedRow::UnmintedToken, PresentedRow::Nothing] {
        assert_eq!(
            distinct(row).len(),
            1,
            "{row:?} is a column wildcard: no resolver can change what it decides, and its \
             verdicts across the five columns were {:?}",
            distinct(row)
        );
    }

    for row in [
        PresentedRow::OperatorCredential,
        PresentedRow::MintedToken(alpha()),
        PresentedRow::RetiredToken(alpha()),
    ] {
        assert!(
            distinct(row).len() > 1,
            "control: {row:?} must decide differently in different columns, or the invariance \
             above is a property of a `classify` that ignores its second argument"
        );
    }
}

/// The two provenances that share the classification table's third column decide identically in every row.
///
/// They are separate variants because classification needs the journal to tell them apart — only
/// "unresolvable because the process was gone" is a laundering signal — and a distinction that is
/// invisible in the verdict is exactly the kind that drifts into being a difference.
#[test]
fn unresolvable_and_no_recorded_agent_decide_identically() {
    for row in [
        PresentedRow::OperatorCredential,
        PresentedRow::MintedToken(alpha()),
        PresentedRow::RetiredToken(alpha()),
        PresentedRow::UnmintedToken,
        PresentedRow::Nothing,
    ] {
        assert_eq!(
            Verdict::of(&classify(row, PeerProvenance::NoRecordedAgent)),
            Verdict::of(&classify(row, PeerProvenance::Unresolvable)),
            "{row:?} must decide the same in both halves of column 3"
        );
        assert_ne!(
            PeerProvenance::NoRecordedAgent,
            PeerProvenance::Unresolvable,
            "positive control: they are still different events"
        );
    }
}

/// **The positive control.** A token minted for alpha, presented from a peer the daemon placed
/// inside alpha's own tree, is honoured — and honoured *as alpha*.
///
/// Without this the whole suite passes when the classifier refuses everything, which is the
/// failure mode a table of refusals is most likely to have.
#[test]
fn an_agent_token_from_that_agents_own_descendant_is_honoured() {
    let resolved = classifier().authorize(
        Presented {
            token: Some(token(ALPHA_TOKEN)),
            cred: None,
        },
        PeerProvenance::Agent(alpha()),
        "memory.append",
    );
    assert_eq!(
        resolved,
        Ok(PeerRole::Agent { agent: alpha() }),
        "row 2, column 1: the cell that makes an agent callable at all"
    );
}

/// **The second positive control.** A valid operator credential from a peer in no agent's tree is
/// an operator.
///
/// Without it, the operator-credential refusal could be implemented as "refuse every operator credential" and every refusal
/// test above would still pass.
#[test]
fn an_operator_credential_from_an_unowned_peer_is_operator() {
    let resolved = classifier().authorize(
        Presented {
            token: None,
            cred: Some(credential()),
        },
        PeerProvenance::NoRecordedAgent,
        "loop.pause",
    );
    assert_eq!(resolved, Ok(PeerRole::Operator), "row 1, column 3");
}

/// The disqualification: a peer the daemon can place inside *any* agent's tree is refused the operator table
/// whatever it presents.
///
/// **Corroboration, not a boundary.** It is escapable — one `fork()` +
/// `setsid()` escapes it, and a narrower variant is one spawn that changes no attribute of the attacker's own
/// process, so nothing in the attacker's own `ps` output looks unusual. On macOS the *absence* of
/// this refusal is therefore evidence of nothing. It is kept because it costs one comparison the
/// daemon makes for ancestry anyway and still catches an attacker that steals the credential without
/// leaving its tree.
#[test]
fn an_operator_credential_from_an_owned_peer_is_refused() {
    for (owner, which) in [
        (alpha(), "its own agent's tree"),
        (bravo(), "a sibling's tree"),
    ] {
        let resolved = classifier().authorize(
            Presented {
                token: None,
                cred: Some(credential()),
            },
            PeerProvenance::Agent(owner),
            "loop.pause",
        );
        assert_eq!(
            Verdict::of(&resolved),
            Verdict::Unauthorized,
            "row 1: a credential presented from {which} is refused"
        );
    }
}

/// Half one: a sibling that steals alpha's token and calls from inside its *own* tree is
/// classification-table row 2 column 2, and is refused.
#[test]
fn a_stolen_token_from_a_sibling_is_refused() {
    let resolved = classifier().authorize(
        Presented {
            token: Some(token(ALPHA_TOKEN)),
            cred: None,
        },
        PeerProvenance::Agent(bravo()),
        "memory.append",
    );
    assert_eq!(Verdict::of(&resolved), Verdict::Unauthorized);
}

/// Half two: a sibling that forks out of its tree first to hide is column 3, and is
/// refused there too.
///
/// This is what makes an escapable relation safe to build an allow-rule on: escaping cannot
/// promote a peer, only demote it.
#[test]
fn a_stolen_token_from_an_unowned_peer_is_refused() {
    for provenance in [
        PeerProvenance::NoRecordedAgent,
        PeerProvenance::Unresolvable,
    ] {
        let resolved = classifier().authorize(
            Presented {
                token: Some(token(ALPHA_TOKEN)),
                cred: None,
            },
            provenance,
            "memory.append",
        );
        assert_eq!(
            Verdict::of(&resolved),
            Verdict::Unauthorized,
            "{provenance:?} is column 3, and a token is not honoured there"
        );
    }
}

/// Classification-table row 3 is the one cell with its own error code, and the contract can now spell it: a
/// token matching no minting is `-32004 Refused{gate: token}`, not `-32001`.
///
/// Asserted through the seam's own refusal type *and* through the wire error it becomes, because
/// a `Refusal::Token` that mapped to `-32001` would satisfy the first alone.
#[test]
fn an_unminted_token_is_refused_with_gate_token() {
    let resolved = classifier().authorize(
        Presented {
            token: Some(token(UNMINTED_TOKEN)),
            cred: None,
        },
        PeerProvenance::Agent(alpha()),
        "memory.append",
    );
    assert_eq!(resolved, Err(Refusal::Token));

    let wire = dudes_rpc::dispatch::refused(&Refusal::Token);
    assert_eq!(wire.code, ErrorCode::Refused.code(), "-32004, not -32001");
    assert_ne!(
        wire.code,
        ErrorCode::Unauthorized.code(),
        "positive control: the two codes are different numbers"
    );
    let data: RefusedData = serde_json::from_value(wire.data.expect("typed data")).unwrap();
    assert_eq!(data.gate, RefusalGate::Token);
}

/// The transport-without-a-process column is absent from the wire-visible provenance
/// vocabulary. Parsing the old spelling is the independent control: this fails while such a
/// variant exists. A transport that cannot report a peer pid is refused before classification.
#[test]
fn the_classification_table_has_no_pidless_column() {
    let former_column = serde_json::json!({"kind": "no_pid"});
    assert!(
        serde_json::from_value::<PeerProvenance>(former_column).is_err(),
        "a process-less transport must be refused before classification, not represented as a column"
    );
}

/// A token the daemon minted still needs the
/// independently resolved ancestry of its owning agent. `Agent(alpha)` is the admitting control;
/// holding the row constant while moving to `Unresolvable` must refuse it.
#[test]
fn unresolvable_hook_peer_with_a_minted_token_is_refused() {
    let row = PresentedRow::MintedToken(alpha());
    assert_eq!(
        Verdict::of(&classify(row, PeerProvenance::Agent(alpha()))),
        Verdict::Agent(alpha()),
        "control: the minted token is honoured for its owning ancestry"
    );
    assert_eq!(
        Verdict::of(&classify(row, PeerProvenance::Unresolvable)),
        Verdict::Unauthorized,
        "an unresolved process cannot be promoted by a token hash"
    );
}

/// The classifier picks the row from the projection and applies the column it was handed — so the
/// grid above is being applied rather than approximated.
///
/// **The classifier is not asked to resolve anything.** The
/// column is resolved once per connection, in the transport's own frame, so there is no
/// resolver here to have asked: what a classifier owes is that its second argument is the
/// value it decides with. The end-to-end half — that the value reaching this argument is the one
/// the transport resolved from this connection's peer — is
/// `tests/seam.rs::the_provenance_serve_resolved_reaches_the_seam`.
///
/// Both arms over one variable. The presentation, the credential and the projection are held
/// constant and only the column moves, so the difference between the two verdicts can only be the
/// argument.
#[test]
fn the_classifier_reaches_the_row_and_applies_the_column_it_is_given() {
    let seam = classifier();
    let presented = || Presented {
        token: Some(token(ALPHA_TOKEN)),
        cred: None,
    };

    assert_eq!(
        seam.authorize(presented(), PeerProvenance::Agent(alpha()), "memory.append"),
        Ok(PeerRole::Agent { agent: alpha() }),
        "the row came from the projection and the column from the argument"
    );
    assert_eq!(
        Verdict::of(&seam.authorize(
            presented(),
            PeerProvenance::NoRecordedAgent,
            "memory.append"
        )),
        Verdict::Unauthorized,
        "control: the same presentation and the same classifier, one column over, is refused — so \
         the argument is read and not ignored"
    );
}

/// The retired-agent row through the classifier. Mapping the retired owner to row 2 fails the refusal arm,
/// and mapping it to row 3 fails the admitting arm.
#[test]
fn a_retired_agents_token_is_honoured_only_from_its_own_tree() {
    let seam = PeerClassifier::new(
        credential(),
        Arc::new(FakeMintedTokens::retired_with_child(&[(
            ALPHA_TOKEN,
            alpha(),
        )])),
    );
    let presented = || Presented {
        token: Some(token(ALPHA_TOKEN)),
        cred: None,
    };
    assert_eq!(
        seam.authorize(presented(), PeerProvenance::Agent(alpha()), "hook.event"),
        Ok(PeerRole::Agent { agent: alpha() }),
        "from inside its own recorded child's tree"
    );
    for provenance in [
        PeerProvenance::Agent(bravo()),
        PeerProvenance::NoRecordedAgent,
        PeerProvenance::Unresolvable,
    ] {
        assert_eq!(
            seam.authorize(presented(), provenance, "hook.event"),
            Err(Refusal::Token),
            "{provenance:?} is outside alpha's tree, and the refusal is row 3's, not row 2's"
        );
    }
}
