//! **Exactly one** function in this crate's library mints a role, and it is the one that
//! implements the classification in `auth.rs`.
//!
//! # What this guards
//!
//! Peer classification is implemented in **one function**, not per handler: the mint exists, there
//! is one of it, and it is in `auth.rs`.
//!
//! So a second classifier — a handler that decided for itself, a convenience constructor that
//! resolved an operator, a "temporary" bypass in `server.rs` — fails here **however the
//! path is written**: a bypass in another module of this crate would spell it
//! `Ok(crate::auth::PeerRole::Operator)`. The scan covers **every qualification of the path** by
//! walking back from the role name rather than enumerating spellings.
//!
//! What that walk-back turns on is **textual adjacency to `Ok(`** — narrower than "the shape", and
//! far narrower than "a second classifier". [`mints`] names the three shapes adjacency lets
//! through and [`the_scan_catches_a_planted_mint`] asserts each of them is *not* caught, so the
//! boundary is pinned by tests rather than described here: widening the scan reddens the suite and
//! takes the prose with it.
//!
//! # The fake that must not move
//!
//! The method surface's tests need an admitting arm, and that arm needs an `Authorizer` that
//! resolves a peer to `Operator`. Such a fake exists — `tests/common/mod.rs` — and it must stay
//! **in the test target**, which is not part of the library and is not compiled into anything that
//! depends on this crate. `AdmitAsOperator` is therefore still forbidden under `src/`, by name.
//!
//! # Absence claims are worth what their control is worth
//!
//! [`the_scan_catches_a_planted_mint`] plants each shape and asserts the same function finds it —
//! including the path-qualified one, whose absence is what let the promise above go unmet.
//! Without that, a mistake in [`mints`] would sit here green for ever. A control that plants only
//! the shapes the scan already knows tests the scan against itself.
//!
//! The scan lives in `tests/`, which it does not read; the tokens below appear in this file by
//! necessity and would otherwise flag it.
//!
//! The **file name** is the one thing that did not change: it is kept so the retarget reads as one
//! diff against the check it replaces rather than as a deletion plus an unrelated new file.

use std::fs;
use std::path::{Path, PathBuf};

/// The shape that *mints* an operator: `Ok(` immediately followed by a path ending in `Operator`.
///
/// **Not a list of spellings.** It was one — `Ok(PeerRole::Operator` and `Ok(Self::Operator` — and
/// a planted `Ok(crate::auth::PeerRole::Operator)` in a file outside `auth.rs`, which is how a
/// module in this crate spells it, walked straight past a scan whose module doc promised to catch
/// exactly that. Two literals cannot enumerate a path: `crate::auth::`, `super::`, `auth::` and
/// `dudes_rpc::auth::` are all the same mint. So [`mints`] matches the *shape* instead — see there
/// for what it still does not catch.
const MINT_OPENS: &str = "Ok(";
/// The role name a minting path ends in.
const MINTED_ROLE: &str = "Operator";

/// The name of the test-target fake that must never move into the library.
const TEST_ONLY_FAKE: &str = "AdmitAsOperator";

/// The one file under `src/` allowed to contain the shape [`mints`] counts.
const THE_CLASSIFIER: &str = "auth.rs";

/// How many times the mint may appear in that file.
///
/// One. Classification resolves to `Operator` in exactly one cell — row 1, column 3 — and
/// the token check has to live in one function. A second occurrence is either a second
/// cell the table does not have or a second place the decision is taken, and both are findings.
const MINTS_ALLOWED: usize = 1;

/// The marker that ends the library half of a source file.
const TEST_MODULE: &str = "\n#[cfg(test)]";

/// The source at `path`, read with CRLF line endings normalised to LF.
///
/// A needle that spans a line break matches or misses on line endings alone — green on two legs
/// of a three-OS matrix and wrong on the third, where nobody looks. A checkout attribute governs
/// only what git writes: this function is handed a *path*, and a `\r\n` source can reach it from a
/// tree git never checked out. So the normalisation belongs at the read and stays whatever the
/// repository's `.gitattributes` does or does not say. The cut below happens to survive CRLF;
/// normalising here is what keeps the next needle from having to.
fn source_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

/// The part of `text` that is compiled into a dependent.
///
/// A `#[cfg(test)]` module is not part of the library and is not compiled into anything that
/// depends on this crate, so an assertion inside one that *names* the operator role is not a mint
/// — and the classifier's own tests necessarily name it. Cutting there keeps the claim about the
/// thing the claim is about. [`the_scan_catches_a_planted_mint`] plants a mint on each side of the
/// marker, so a cut in the wrong place fails rather than hides.
fn library_text(text: &str) -> &str {
    match text.find(TEST_MODULE) {
        Some(at) => &text[..at],
        None => text,
    }
}

/// How many times `text` contains `token`.
fn count(text: &str, token: &str) -> usize {
    text.matches(token).count()
}

/// The total number of minting shapes in `text`. One place, so the scan and the control that
/// proves the scan works cannot drift apart.
///
/// For each occurrence of [`MINTED_ROLE`] that ends a path segment, walk backwards over the path
/// characters to where the path starts and ask whether [`MINT_OPENS`] is what precedes it. So
/// every qualification of the same mint is one shape, and the three things that must stay legal
/// are all excluded by the same rule:
///
/// - **matching** the role — `(Self::Operator | ..., PeerRole::Operator)` — is preceded by `(` or
///   `, `, never by `Ok(`;
/// - **declaring** the variant — `    Operator,` — is preceded by whitespace;
/// - `OperatorOrAgent` is not a path segment ending in `Operator` at all, which is why the
///   trailing character is checked before anything else.
///
/// # What it still does not catch
///
/// The boundary is **textual adjacency to [`MINT_OPENS`]**, not expression shape. The walk-back
/// halts at the first character that cannot be part of a path, and the mint counts only if the
/// text ending there is exactly `Ok(`. So anything that puts a character between `Ok(` and the
/// path is missed, however ordinary it looks. Three such shapes, each measured against this tree
/// by planting it in `server.rs` and running the suite, and each pinned as an assertion in
/// [`the_scan_catches_a_planted_mint`] rather than left as prose:
///
/// - **A turbofish** — `Ok::<PeerRole, Refusal>(PeerRole::Operator)`. The walk-back halts at `(`
///   and the text before it ends `>(`. This one is one expression on one line, and it is
///   rustfmt-stable and clippy-clean: **every gate in the pre-push set passes it.** So a mint
///   is not excluded merely for being written as one expression.
/// - **A separated `Ok(`** — `Ok(` and the path parted by whitespace or a line break. The
///   walk-back halts at the whitespace. `cargo fmt --check` rejects this one, because rustfmt
///   rejoins it into the adjacent form the scan does catch; the fmt gate, not this scan, is what
///   stands between it and a merge.
/// - **A mint split across statements** — `let role = PeerRole::Operator;` then `Ok(role)`. There
///   is no `Ok(` before the path at all.
///
/// **The direction here is fail-open**: a shape this function misses passes silently. That is why
/// the three shapes above are assertions: a fail-open residual left in prose is a claim nothing can
/// falsify.
///
/// Telling *construction* from *matching* in the general case needs a parser, and a bare
/// `PeerRole::Operator` is legal in both this crate's `match` arms and its `Debug`-adjacent code.
/// The claim this file makes is therefore about the **adjacent** one-expression shape, which is
/// what every mint in this crate is and what a bypass added for convenience would be; it is not a
/// proof that no other shape exists.
fn mints(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut found = 0;
    for (at, _) in text.match_indices(MINTED_ROLE) {
        // `OperatorOrAgent` and friends: the role must end the path segment.
        let after = at + MINTED_ROLE.len();
        if bytes
            .get(after)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            continue;
        }
        // Walk back over `A-Za-z0-9_:` to the first character of the path.
        let mut start = at;
        while start > 0 {
            let b = bytes[start - 1];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b':' {
                start -= 1;
            } else {
                break;
            }
        }
        if text[..start].ends_with(MINT_OPENS) {
            found += 1;
        }
    }
    found
}

/// Every `.rs` file under `dir`, at any depth.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// One file under `src/` mints the operator role, it is the classifier, and it does so once.
#[test]
fn exactly_one_function_in_the_library_mints_an_operator() {
    let mut files = Vec::new();
    rust_files(&crate_root().join("src"), &mut files);

    let mut minting = Vec::new();
    for path in &files {
        let whole = source_text(path);
        let found = mints(library_text(&whole));
        if found > 0 {
            minting.push((path.clone(), found));
        }
        // By name, over the *whole* file: the fake must not appear in `src/` at all, test module
        // included, because a `#[cfg(test)]` fake in the library is one `cfg` away from shipping.
        // `src/testing.rs` is now exactly that shape — `#[cfg(any(test, feature = "testing"))]`,
        // one feature from a shipping build — which is why this prohibition is by *name* and
        // reaches only the operator-minting fake rather than gated fakes in general.
        // `tests/no_default_fakes.rs` guards the gate on that module; this line guards the fake
        // that is not allowed under `src/` behind any gate at all.
        assert_eq!(
            count(&whole, TEST_ONLY_FAKE),
            0,
            "{} names {TEST_ONLY_FAKE}; a fake that mints a role must live in the test target",
            path.display()
        );
    }

    assert_eq!(
        minting.len(),
        1,
        "exactly one source under src/ may mint an operator; found {minting:?}"
    );
    let (path, found) = &minting[0];
    assert_eq!(
        path.file_name().unwrap(),
        THE_CLASSIFIER,
        "the mint belongs to the classifier, not to {}",
        path.display()
    );
    assert_eq!(
        *found,
        MINTS_ALLOWED,
        "{} mints an operator {found} times; the classification table has one such cell and one such function",
        path.display()
    );

    assert!(
        files.len() >= 12,
        "scanned only {} files; is src/ where it was?",
        files.len()
    );
}

/// The scan above passes by counting, which proves nothing unless the scan can count wrong. Each
/// shape is planted and asserted caught; the shapes that must stay legal are asserted not caught.
#[test]
fn the_scan_catches_a_planted_mint() {
    for planted in [
        "        Ok(PeerRole::Operator)\n    }",
        "            Ok(Self::Operator)",
        // The spelling a module of this crate outside `auth.rs` uses. Every qualification of the same path is the same mint.
        "        Ok(crate::auth::PeerRole::Operator)",
        "        Ok(super::PeerRole::Operator)",
        "        Ok(auth::PeerRole::Operator)",
        "    Ok(dudes_rpc::auth::PeerRole::Operator)\n",
    ] {
        assert_eq!(mints(planted), 1, "the scan did not catch `{planted}`");
    }
    assert_eq!(
        mints("Ok(PeerRole::Operator) ... Ok(Self::Operator)"),
        2,
        "a second mint in the same file is counted, which is what the one-mint rule rests on"
    );
    assert_eq!(
        count("pub struct AdmitAsOperator;", TEST_ONLY_FAKE),
        1,
        "the test-target fake is still caught by name"
    );
    assert_eq!(
        mints("            Ok(PeerRole::Unproven)"),
        0,
        "the role a default-deny seam produces must not be flagged"
    );
    assert_eq!(
        mints("            (Self::Operator | Self::OperatorOrAgent, PeerRole::Operator)"),
        0,
        "matching on the role is not minting it: the requirement check must stay legal"
    );
    assert_eq!(
        mints("            Self::Operator => \"operator\","),
        0,
        "a match arm naming the role is not minting it either"
    );
    assert_eq!(
        mints("    OperatorOrAgent,\n    Ok(Self::OperatorOrAgent)"),
        0,
        "a longer variant that starts with the role's name is a different variant"
    );
    assert_eq!(
        mints("    Operator,\n    Agent { agent: AgentId },"),
        0,
        "declaring the variant is not minting it either"
    );

    // The residual, pinned rather than described. Each of these is a *real* bypass: planted in
    // `server.rs` against this tree, each one left the suite green, and the first and third were
    // clean under `cargo fmt --check` and `cargo clippy -- -D warnings` as well. The old doc
    // called the residual "a mint that is not written as one expression", which is wrong about the
    // first two — both are one expression. The boundary is adjacency to `Ok(`, and these three
    // assertions are what stop that claim from going stale in silence: widen `mints` and they fail.
    assert_eq!(
        mints("        Ok::<PeerRole, Refusal>(PeerRole::Operator)"),
        0,
        "residual: a turbofish leaves `>(` before the path, so the walk-back's `Ok(` test fails"
    );
    assert_eq!(
        mints("        Ok(\n            PeerRole::Operator,\n        )"),
        0,
        "residual: a separated `Ok(` halts the walk-back at whitespace — `cargo fmt --check` \
         rejects this one, so the fmt gate covers what this scan does not"
    );
    assert_eq!(
        mints("        let role = PeerRole::Operator;\n        Ok(role)"),
        0,
        "residual: a mint split across statements has no `Ok(` before the path at all"
    );
    assert_eq!(
        mints("        Ok(PeerRole::Operator)"),
        1,
        "positive control: the same needle on the adjacent spelling of the same mint is caught, \
         so the three zeros above read as a boundary and not as a scan that counts nothing"
    );

    // The `#[cfg(test)]` cut, planted on both sides: a mint above the marker is counted and one
    // below it is not, so a cut in the wrong place — or no cut at all — fails here.
    let split = "fn f() { Ok(PeerRole::Operator) }\n#[cfg(test)]\nmod tests { Ok(Self::Operator) }";
    assert_eq!(mints(split), 2, "positive control: both halves contain one");
    assert_eq!(
        mints(library_text(split)),
        1,
        "only the library half is the library's"
    );
    assert_eq!(
        library_text("fn f() {}\n"),
        "fn f() {}\n",
        "a file with no test module is not truncated"
    );
}
