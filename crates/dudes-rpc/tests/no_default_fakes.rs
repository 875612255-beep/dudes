//! The permissive fakes are not in a default build of this crate, and no build in this workspace
//! asks for them.
//!
//! # What is actually enforced, and by what
//!
//! The enforcement is the **compiler**, not this file. `src/lib.rs` declares the module
//! `#[cfg(any(test, feature = "testing"))]`, so `cargo build -p dudes-rpc` does not compile
//! `src/testing.rs` and a dependent that writes `dudes_rpc::testing::AdmitAll` without the feature
//! fails to resolve the path. Measured on this tree: a scratch crate depending on `dudes-rpc` with
//! default features and naming `AdmitAll` fails with `E0433`, and the same crate compiles once
//! `features = ["testing"]` is added.
//!
//! This file exists because that arrangement is a few edits away from being undone, and each of
//! them is silent — the suite stays green, `clippy` stays clean, and the bypass is back in every
//! daemon that links the crate. **Four of those edits have a check here; the residuals list below
//! names the ones that do not**, and it is a list of what has been measured rather than a count
//! that closes:
//!
//! 1. the attribute is dropped from the declaration in `src/lib.rs`;
//! 2. a `default` table is added to `Cargo.toml` naming the feature;
//! 3. a dependent turns the feature on from a **non-dev** dependency table;
//! 4. **any other package in this workspace reaches the feature from its own `default`** — a
//!    `default = ["dudes-rpc/testing"]` in a sibling's manifest, or a `default` that names a
//!    feature that names it, in that manifest or in a further sibling's.
//!
//! Each is a check below, and each check has its own `*_catches_a_planted_regression` control that
//! plants the regression and asserts the same function the check runs reports it. One control per
//! scan rather than one for the file: a single control going red would say only that *something* is
//! unproven, and telling one silent regression from another is this file's whole job.
//!
//! Nine mutations were also run against real manifests in this tree rather than against strings —
//! the attribute deleted, a `default` table added, and the feature turned on from a shipping
//! dependency in six spellings (inline, wrapped over three lines, the `[dependencies.dudes-rpc]`
//! sub-table and its `workspace.` and `target.'cfg(unix)'.` forms, a quoted key, and a
//! `package = "dudes-rpc"` rename). Each reddened its own check and nothing else, and the two
//! arrangements that must stay legal — a dev-dependency sub-table and a commented-out activation —
//! stayed green.
//!
//! [`classify_header`] asks whether a header has a `dependencies` **segment** rather than whether it
//! *ends* in `dependencies`, because `[dependencies.dudes-rpc]` ends in the package name.
//!
//! **Every one of those nine used a double-quoted features list, and none of them put a
//! `[features]` table in a manifest other than this crate's** — which is why three more spellings
//! sat here unmodelled while the comments claimed otherwise, and why they now have plants of their
//! own. All three were first watched **passing** against the version of this file that preceded
//! them: a shipping dependency spelt with TOML literal strings,
//! `dudes-rpc = { workspace = true, features = ['testing'] }`, and a `[features]` table in
//! `bins/duded/Cargo.toml` whose `default` reached `dudes-rpc/testing` directly and then through
//! one named hop. Each was confirmed a real activation and not a typo before being called a hole:
//! `cargo tree -p duded -e features -i dudes-rpc` reported `dudes-rpc feature "testing"` under
//! `duded feature "default"` for all three. **A fix is only as good as the plants added with it.**
//!
//! # Why it is a manifest scan and not a build
//!
//! The direct check would run `cargo build` over a fixture crate and assert the failure. That
//! needs a nested cargo invocation inside a test — a second target directory, a lockfile, and a
//! network posture — for a property the three text conditions above already decide. The idiom here
//! is `tests/no_runtime.rs`: read the artefact that owns the rule, and pin the scan with plants.
//!
//! # What it does not close
//!
//! - **Only this workspace is read.** A crate outside it may depend on `dudes-rpc` with
//!   `features = ["testing"]` and nothing here sees it. The gate turns reaching `AdmitAll` into a
//!   line in a manifest instead of a `use` statement; it does not make it impossible.
//! - **`cargo build --workspace` is not the only build.** `cargo test --workspace` needs
//!   dev-dependencies, so resolver 3 unifies the feature onto the `dudes-rpc` library for that
//!   whole invocation — anything built as part of a test run has it. That is the point of a
//!   dev-dependency and not a hole; the claim is about what links into a shipped artefact.
//! - **Renaming the feature or the module** escapes every needle here: the scans are textual, like
//!   the others in this directory. That direction is safe — a rename fails these tests loudly
//!   rather than passing silently.
//! - **The dependency scan has residuals of its own**, and [`shipping_activations`] owns that
//!   list rather than this header repeating it. Read it there; the short version is that a
//!   `--features dudes-rpc/testing` on a command line is written in no manifest at all.
//! - **The `default`-feature walk has residuals of its own**, and [`default_route`] owns that list
//!   for the same reason. The short version is that it resolves the name before a `/` against
//!   workspace package names, so a renamed dependency escapes it where it does not escape the
//!   dependency scan.
//! - **A feature of this crate that is not spelt `testing` defeats both**, and it is the cheapest
//!   bypass measured here. [`names_feature`] owns that one.
//!
//! **None of these scans is fail-closed, and no comment here should say one is.** Each is a set of
//! spellings that were measured, and a spelling nobody modelled passes silently. That is why the
//! list above is written as residuals rather than as a proof, and why this file makes no claim about
//! what a matcher cannot miss.
//!
//! **The residuals list is what the next reader trusts instead of re-deriving the coverage, and it
//! can never be proven complete.** Some live activations are facts about the resolved feature graph
//! rather than about the text of a manifest, which no reader of `Cargo.toml` files closes by
//! reading harder. So the standard this file holds itself to is: **everything known to be open is
//! named, with the measurement that found it.** A list that grows under review is working as
//! intended; a list that claims to be finished is the defect.
//!
//! This file names the feature and the module by necessity and reads only `Cargo.toml` files and
//! `src/lib.rs`, so it does not scan itself.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// This crate's package name, as a dependency key in someone else's manifest.
const PACKAGE: &str = "dudes-rpc";

/// The feature that turns the fakes on.
const FEATURE: &str = "testing";

/// The declaration in `src/lib.rs` that the gate has to sit on.
const DECLARATION: &str = "pub mod testing;";

/// The attribute that must immediately precede it.
///
/// `cfg(test)` is not decoration: `src/dispatch.rs`, `src/server.rs` and `src/auth.rs` reach the
/// fakes from their own `#[cfg(test)]` modules, so a bare `#[cfg(feature = "testing")]` does not
/// compile. The exact text is pinned rather than parsed — a looser gate is a different rule and
/// should be a diff someone reads, not something a regex waves through.
const GATE: &str = "#[cfg(any(test, feature = \"testing\"))]";

/// The floor under the number of manifests [`workspace_manifests`] must find.
///
/// The workspace has 13: the root, eight crates, two bins, `xtask`, and `fuzz` (its own workspace,
/// read anyway because a build is a build). The floor stays under that because it is a floor: a walk that has
/// stopped seeing files must fail rather than read as clean, and a tree that grows a crate must not
/// fail for growing one.
const MIN_MANIFESTS: usize = 9;

/// The source at `path`, with CRLF line endings normalised to LF.
///
/// Needles here span line breaks — [`declares_ungated`] looks at the text *before* a declaration —
/// and a `\r` left in place makes those match or miss on line endings alone: green on two legs of a
/// three-OS matrix and wrong on the third. The other scans in this directory normalise at the read
/// for the same reason.
fn text_at(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The workspace root: two levels above `crates/dudes-rpc`.
fn workspace_root() -> PathBuf {
    let root = crate_root().join("..").join("..");
    assert!(
        root.join("Cargo.toml").is_file(),
        "no workspace manifest above {}; this scan reads the checkout's layout",
        crate_root().display()
    );
    root
}

// ---------------------------------------------------------------------------------------------
// 1. The declaration in `src/lib.rs`
// ---------------------------------------------------------------------------------------------

/// How many times `text` declares the module **without** [`GATE`] immediately before it.
///
/// The attribute is required to be adjacent, ignoring only whitespace: an attribute belongs to the
/// item it precedes, and anything between the two that is not whitespace means the reader is
/// looking at a different arrangement than this test is claiming. Comments therefore go *above* the
/// attribute, which is where `src/lib.rs` puts them.
fn declares_ungated(text: &str) -> usize {
    text.match_indices(DECLARATION)
        .filter(|(at, _)| !text[..*at].trim_end().ends_with(GATE))
        .count()
}

/// How many times `text` declares the module at all, gated or not.
///
/// Counted so the check cannot pass by the declaration having been renamed out from under it: zero
/// declarations satisfies "none of them is ungated" perfectly.
fn declarations(text: &str) -> usize {
    text.matches(DECLARATION).count()
}

// ---------------------------------------------------------------------------------------------
// 2. The `[features]` table
// ---------------------------------------------------------------------------------------------

/// The `[features]` table of `manifest`, entry by entry, as `(feature name, entry text)`.
///
/// Keyed on each entry's **full dotted path** — the table header's segments followed by the entry
/// key's — rather than on the header alone, so two of the spellings of the same table give one
/// answer: a `[features]` header with `default = […]` under it, and a `features.default = […]`
/// written at the top of the manifest with no header at all. Cargo reads a manifest as TOML and
/// both build the same table; a reader that followed only the header would miss the second, which
/// is a one-line edit.
///
/// **There is a third, and it is not covered.** An inline table in the preamble —
/// `features = { default = ["dudes-rpc/testing"] }`, one line at the top of a manifest — is the
/// same `[features]` table to cargo and reaches this function as the one-segment path
/// `["features"]`, which the match below discards because it accepts only `[table, name]`. The
/// value is never split into entries, so [`default_route`] finds no `default` to start from.
/// Measured in `bins/duded/Cargo.toml`: `cargo tree -p duded -e features -i dudes-rpc` reported
/// `dudes-rpc feature "testing"`, every check in this file stayed green, and a line returning
/// `dudes_rpc::testing::AdmitAll` added to `bins/duded/src/lib.rs` compiled — the same line gives
/// `error[E0433]: cannot find 'testing' in 'dudes_rpc'` on the clean tree. It is listed with the
/// rest on [`default_route`].
///
/// An empty result is a different answer from a table with no `default` in it, and both are
/// load-bearing below: a missing `[features]` table means the feature does not exist, and a missing
/// `default` entry is exactly how "off unless asked for" is expressed.
fn feature_entries(manifest: &str) -> Vec<(String, String)> {
    dotted_entries(manifest)
        .into_iter()
        .filter_map(|(path, text)| match path.as_slice() {
            [table, name] if table.as_str() == "features" => Some((name.clone(), text)),
            _ => None,
        })
        .collect()
}

/// Whether the `[features]` table declares an entry named `key`.
fn declares_feature(manifest: &str, key: &str) -> bool {
    feature_entries(manifest)
        .iter()
        .any(|(name, _)| name == key)
}

/// The text of the `default` entry of the `[features]` table, if there is one.
fn default_entry(manifest: &str) -> Option<String> {
    feature_entries(manifest)
        .into_iter()
        .find(|(name, _)| name == "default")
        .map(|(_, text)| text)
}

/// The package name `manifest` declares, if it declares one.
///
/// The workspace root declares none: `[workspace.package]` is metadata members inherit, not a
/// package, and its path is three segments rather than two. Matched on the full dotted path for the
/// reason [`feature_entries`] is — `package.name = "…"` with no header is the same declaration.
fn package_name(manifest: &str) -> Option<String> {
    dotted_entries(manifest)
        .into_iter()
        .find_map(|(path, text)| match path.as_slice() {
            [table, field] if table.as_str() == "package" && field.as_str() == "name" => {
                quoted_values(&text).into_iter().next()
            }
            _ => None,
        })
}

// ---------------------------------------------------------------------------------------------
// 3. Dependency tables across the workspace
// ---------------------------------------------------------------------------------------------

/// What a `[...]` header means to this scan.
#[derive(Debug, PartialEq, Eq)]
enum Table {
    /// A dependency table a shipped build reads; its lines are the entries.
    Shipping,
    /// A single dependency written as its own table — `[dependencies.dudes-rpc]`. The whole body is
    /// one entry and the string is the key.
    ShippingEntry(String),
    /// Anything else, including `[dev-dependencies]` and `[build-dependencies]`.
    Other,
}

/// `line` with a trailing `#` comment removed, quotes respected.
///
/// A commented-out dependency is not a dependency, so dropping the comment is the correct reading
/// and not a concession. It also keeps a `#` inside a version string from truncating the line.
fn without_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (at, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, '#') => return &line[..at],
            _ => {}
        }
    }
    line
}

/// How far `line` opens more brackets and braces than it closes, ignoring anything inside quotes.
///
/// Quote-aware because a `cfg` predicate carries brackets nowhere but a string —
/// `[target.'cfg(unix)'.dependencies]` is balanced either way, but a version requirement or a path
/// containing one would not be, and a skewed depth silently stops the walk seeing headers.
fn balance(line: &str) -> i32 {
    let mut quote: Option<char> = None;
    let mut depth = 0;
    for c in without_comment(line).chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// `text` with one layer of matching quotes removed.
fn unquote(text: &str) -> &str {
    let text = text.trim();
    for q in ['"', '\''] {
        if let Some(inner) = text.strip_prefix(q).and_then(|rest| rest.strip_suffix(q)) {
            return inner;
        }
    }
    text
}

/// A header's dotted segments, unquoted, with a quoted segment kept whole.
///
/// `target.'cfg(unix)'.dependencies` is three segments, not four: the dot inside the quoted `cfg`
/// predicate is part of a key rather than a separator, and splitting on every dot would classify
/// that table as `Other` and skip a real dependency table on Unix.
fn segments(header: &str) -> Vec<String> {
    let inner = header
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                current.push(c);
            }
            (Some(open), c2) if c2 == open => {
                quote = None;
                current.push(c2);
            }
            (None, '.') => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out.iter().map(|s| unquote(s).to_owned()).collect()
}

/// What a table header means for this scan.
///
/// The rule is one segment, not a suffix: a header is a shipping dependency table when one of its
/// **segments** is exactly `dependencies`. `dev-dependencies` and `build-dependencies` are
/// different segments and never match, so they need no special case, and a header that merely
/// *ends* in something is not what decides. `[dependencies.dudes-rpc]` ends with `dudes-rpc`, so a
/// suffix test would read it as "not a dependency table" and skip every line under it, leaving
/// the sub-table spelling undetected.
///
/// When a segment follows `dependencies`, the header *is* one entry and that segment is its key.
fn classify_header(header: &str) -> Table {
    let segments = segments(header);
    let Some(at) = segments.iter().position(|s| s == "dependencies") else {
        return Table::Other;
    };
    match segments.get(at + 1) {
        None => Table::Shipping,
        Some(key) => Table::ShippingEntry(key.clone()),
    }
}

/// A manifest split into `(header, body lines)`, the preamble first under an empty header.
///
/// A line counts as a header only at bracket depth zero, so an array element inside a multi-line
/// `features = [ … ]` cannot be mistaken for one.
fn sections(manifest: &str) -> Vec<(String, Vec<&str>)> {
    let mut out: Vec<(String, Vec<&str>)> = vec![(String::new(), Vec::new())];
    let mut depth = 0;
    for line in manifest.lines() {
        let code = without_comment(line).trim();
        if depth == 0 && code.starts_with('[') && code.ends_with(']') {
            out.push((code.to_owned(), Vec::new()));
        } else {
            out.last_mut().expect("the preamble section").1.push(line);
            depth += balance(line);
        }
    }
    out
}

/// The dotted key an entry line declares, segment by segment and unquoted, or `None` for a line
/// that is not an entry.
///
/// Every segment rather than the first: under a dependency table the first is what names the
/// package — `dudes-rpc.features = ["testing"]` is an entry for `dudes-rpc` — while at the top of a manifest the whole path is what names the table,
/// and `features.default = […]` is the `[features]` table's `default` entry written without a
/// header. Unquoted, so `"dudes-rpc" = { … }` is the same key as the bare spelling.
fn entry_path(line: &str) -> Option<Vec<String>> {
    let code = without_comment(line).trim();
    let at = code.find('=')?;
    let key = code[..at].trim();
    if key.is_empty() {
        return None;
    }
    Some(segments(key))
}

/// The entries of a table body as `(dotted key, entry text)`, each accumulated until its brackets
/// balance.
///
/// An entry is not always one line — `windows-sys` in the workspace root spans five, and the
/// wrapped form is the one `cargo fmt` leaves alone — so a line that opens a bracket is joined with
/// what follows until it closes. A wrapped feature list is the same shape as a wrapped dependency,
/// which is why one function serves both readers.
fn entries(body: &[&str]) -> Vec<(Vec<String>, String)> {
    let mut out = Vec::new();
    let mut open: Option<String> = None;
    let mut depth = 0;
    for line in body {
        let code = without_comment(line).trim();
        match open {
            None => {
                if entry_path(code).is_some() {
                    depth = balance(code);
                    open = Some(code.to_owned());
                }
            }
            Some(ref mut text) => {
                text.push(' ');
                text.push_str(code);
                depth += balance(code);
            }
        }
        if depth <= 0
            && let Some(text) = open.take()
            && let Some(path) = entry_path(&text)
        {
            out.push((path, text));
        }
    }
    out
}

/// Every entry in `manifest` under its full dotted path: the header's segments, then the key's.
///
/// The preamble contributes its keys unprefixed, which is what makes a headerless
/// `features.default = […]` and a `[features]` table's `default` the same path.
fn dotted_entries(manifest: &str) -> Vec<(Vec<String>, String)> {
    let mut out = Vec::new();
    for (header, body) in sections(manifest) {
        let prefix = if header.is_empty() {
            Vec::new()
        } else {
            segments(&header)
        };
        for (key, text) in entries(&body) {
            let mut path = prefix.clone();
            path.extend(key);
            out.push((path, text));
        }
    }
    out
}

/// Every quoted string in `text` after its first `=`, in either of TOML's two string forms.
///
/// The values of an entry, in other words — the key is on the other side of the `=`, so a quoted
/// key is not read as a value. Nothing is decoded: the bytes between the quotes are the answer, so
/// an escape sequence is not resolved to the character it stands for. That is a residual, named
/// where it bites in [`default_route`].
fn quoted_values(text: &str) -> Vec<String> {
    let Some(at) = text.find('=') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut open: Option<char> = None;
    let mut current = String::new();
    for c in text[at + 1..].chars() {
        match open {
            None => {
                if c == '"' || c == '\'' {
                    open = Some(c);
                    current.clear();
                }
            }
            Some(quote) if c == quote => {
                out.push(std::mem::take(&mut current));
                open = None;
            }
            Some(_) => current.push(c),
        }
    }
    out
}

/// The package an entry actually refers to: its `package = "…"` rename if it has one, else its key.
///
/// Cargo's rename form means the key is not the package. `myrpc = { package = "dudes-rpc",
/// features = ["testing"] }` links this crate under another name.
fn entry_package(key: &str, text: &str) -> String {
    renamed_package(text).unwrap_or_else(|| key.to_owned())
}

/// The value of a `package = "…"` field in `text`, if there is one.
///
/// Either of TOML's string forms, because cargo accepts either: `package = 'dudes-rpc'` renames the
/// same crate as `package = "dudes-rpc"`. Reading only the basic-string form was the same defect
/// [`names_feature`] carried, and it compounded — a renamed entry that also spelt its features list
/// with literal strings escaped twice over.
fn renamed_package(text: &str) -> Option<String> {
    const FIELD: &str = "package";
    for (at, _) in text.match_indices(FIELD) {
        let preceded_by_word = text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
        if preceded_by_word {
            continue;
        }
        let rest = text[at + FIELD.len()..].trim_start();
        let Some(value) = rest.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        for quote in ['"', '\''] {
            if let Some(quoted) = value.strip_prefix(quote)
                && let Some(end) = quoted.find(quote)
            {
                return Some(quoted[..end].to_owned());
            }
        }
    }
    None
}

/// Whether a dependency entry's text turns [`FEATURE`] on, matched as the quoted string a features
/// list holds so a `package = "testing-…"` rename is not a hit.
///
/// **Both of TOML's string forms.** `features = ["testing"]` and `features = ['testing']` are the
/// same activation to cargo, and a manifest using the second shows it:
/// `cargo tree -p duded -e features -i dudes-rpc` reported `dudes-rpc feature "testing"` while this
/// scan stayed green. Every other helper in this file is quote-agnostic — the comment
/// stripper, the balancer, [`unquote`], [`segments`] — because this repository's own manifests
/// write `[target.'cfg(unix)'.dependencies]`; this decision uses both needles.
///
/// Coarse on purpose: a quoted `testing` anywhere in the entry counts, rather than only one inside
/// a `features` array. The error that direction makes is a **false positive** on an entry that
/// quotes the word for some other reason, which reddens the suite on innocent text.
///
/// **It is not fail-closed**: a needle written as a literal sees the spellings it was written for and
/// no others. A feature name carrying a TOML escape sequence for one of its own letters is still a
/// miss, because nothing here decodes one; that residual is listed with the rest on
/// [`shipping_activations`].
///
/// # The cheapest bypass in this file, and it defeats both scans
///
/// [`FEATURE`] is matched as a **literal string**, so a second feature of this crate that enables
/// it under another name is not this needle's word. Two lines: `fake = ["testing"]` in
/// `crates/dudes-rpc/Cargo.toml`, and `features = ["fake"]` on a shipping dependent. This function
/// sees only `"fake"`; [`default_route`] never runs, because it starts at `default` and is not
/// invoked from a dependency entry's feature list at all. Measured: `cargo tree -p duded -e
/// features -i dudes-rpc` reported `testing` under `fake` under `duded`, all 12 checks here stayed
/// green, and `AdmitAll` linked into `duded` — with the clean-tree `E0433` as the other arm.
///
/// It is **cheaper than the rename residual [`default_route`] names**: it survives `--locked`,
/// because it adds no dependency edge for a lockfile to record, and it leaves the dependent's
/// source untouched. Nothing enforces the single-feature arrangement it breaks — the prose in
/// `crates/dudes-rpc/Cargo.toml` asking that the table stay as it is has no check behind it, and
/// this file cannot supply one by reading harder for a name it does not know.
fn names_feature(entry: &str) -> bool {
    ['"', '\'']
        .into_iter()
        .any(|quote| entry.contains(&format!("{quote}{FEATURE}{quote}")))
}

/// Every entry in `manifest` that a shipped build reads, links [`PACKAGE`], and turns [`FEATURE`]
/// on — as the entry's own text.
///
/// # What this covers, stated as what was measured rather than as a claim
///
/// Both TOML spellings of a dependency, in every dependency table a linked build reads, under both
/// ways of naming the package. Each of these was planted in a real manifest in this workspace and
/// watched reddening [`no_shipping_dependency_asks_for_the_fakes`]:
///
/// - inline, one line and wrapped over three — a dependency entry is not always one line
///   (`windows-sys` in the workspace root spans five), and the wrapped form is `cargo fmt`-stable,
///   so entries are accumulated until their brackets balance;
/// - the dotted sub-table, `[dependencies.dudes-rpc]` and its `workspace.` and `target.'cfg(…)'.`
///   forms;
/// - the dotted inline key, `dudes-rpc.features = ["testing"]`;
/// - a quoted key, `"dudes-rpc" = { … }`;
/// - the `package = "dudes-rpc"` rename under any key at all;
/// - both of TOML's string forms, `features = ["testing"]` and `features = ['testing']`, under a
///   plain key and under a rename.
///
/// # What it does not cover, which is why it is not called fail-closed
///
/// Multi-line handling does not decide the direction, and the scan is not fail-closed. It is
/// fail-**open** at its edges — a spelling it does not model
/// passes silently — and the list above is a list of measurements, not a proof of coverage. These
/// residuals are known and none is closable by reading manifests harder:
///
/// - **`--features dudes-rpc/testing` on a cargo command line.** Nothing is written in any
///   manifest, so no manifest scan can see it. A CI job or a script is where that would appear.
/// - **A manifest outside this workspace.** Only this tree is walked.
/// - **A vendored or patched copy under another package name**, where the string `dudes-rpc`
///   appears nowhere in the entry.
/// - **A feature name spelt with TOML escape sequences**, such as a `\u` escape standing for one of
///   its letters. [`names_feature`] and [`quoted_values`] compare the bytes between the quotes and
///   decode nothing, so that name is not this feature's name to either of them.
/// - **A second feature of this crate that enables [`FEATURE`] under another name.** A dependent
///   asking for `fake` asks for the fakes, and no needle here holds the word `fake`. Measured, and
///   the cheapest bypass in this file; [`names_feature`] carries the reproduction.
///
/// **No direction here is fail-closed.** [`names_feature`] over-reports rather than missing on the
/// two spellings it models, which is not the same property, and the sentence that called it
/// fail-closed is what let the third spelling through.
fn shipping_activations(manifest: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (header, body) in sections(manifest) {
        match classify_header(&header) {
            Table::Other => {}
            Table::ShippingEntry(key) => {
                let text = body
                    .iter()
                    .map(|line| without_comment(line).trim())
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if entry_package(&key, &text) == PACKAGE && names_feature(&text) {
                    found.push(format!("{header} {text}"));
                }
            }
            Table::Shipping => {
                for (path, text) in entries(&body) {
                    // The package is named by the first segment: `dudes-rpc.features = […]` is an
                    // entry for `dudes-rpc`, whatever follows the dot.
                    if let Some(key) = path.first()
                        && entry_package(key, &text) == PACKAGE
                        && names_feature(&text)
                    {
                        found.push(text);
                    }
                }
            }
        }
    }
    found
}

/// Every `Cargo.toml` under the workspace root, skipping build output and dot directories.
fn workspace_manifests(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {}: {error}", root.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            workspace_manifests(&path, out);
        } else if name == "Cargo.toml" {
            out.push(path);
        }
    }
}

/// Every `Cargo.toml` under the workspace root, with the [`MIN_MANIFESTS`] floor asserted.
///
/// The floor is asserted here rather than after a scan has returned its verdict, so that a walk
/// which has stopped seeing files fails as itself rather than as a clean scan.
fn manifest_paths() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut paths = Vec::new();
    workspace_manifests(&root, &mut paths);
    assert!(
        paths.len() >= MIN_MANIFESTS,
        "scanned only {} manifests under {}; is the workspace where it was?",
        paths.len(),
        root.display()
    );
    paths
}

// ---------------------------------------------------------------------------------------------
// 4. `default` features across the workspace
// ---------------------------------------------------------------------------------------------

/// A manifest reduced to what the `default`-feature walk needs: what package it is, and its
/// `[features]` table.
#[derive(Debug)]
struct Member {
    /// The package this manifest declares.
    package: String,
    /// Its `[features]` table, as `(feature name, entry text)`.
    features: Vec<(String, String)>,
}

/// Every workspace manifest that declares a package, as the feature graph [`default_route`] walks.
fn workspace_members() -> Vec<Member> {
    let members: Vec<Member> = manifest_paths()
        .iter()
        .filter_map(|path| {
            let manifest = text_at(path);
            package_name(&manifest).map(|package| Member {
                package,
                features: feature_entries(&manifest),
            })
        })
        .collect();
    assert!(
        members.iter().any(|member| member.package == PACKAGE),
        "no member named {PACKAGE} among the {} that declare a package; the walk resolves the name \
         before a `/` against these, so it would answer \"no route\" for every manifest in the tree",
        members.len()
    );
    members
}

/// The route from `package`'s `default` feature to [`PACKAGE`]'s [`FEATURE`], if there is one.
///
/// # Why a walk, and why it is not the `[features]` check
///
/// [`the_feature_is_off_by_default`] reads one manifest — this crate's — and asserts it has no
/// `default` entry at all. That is sound for the guarded crate and transfers to no other, where a
/// `default` table is ordinary: `crates/dudes-types` ships one today. Cargo turns a package's
/// `default` on for every plain `cargo build` of it, and a feature entry may name another crate's
/// feature, so `default = ["dudes-rpc/testing"]` in a sibling's manifest links the fakes into that
/// build, and `default = ["extra"]` with `extra = ["dudes-rpc/testing"]` does it one hop further
/// along. Both were planted in `bins/duded/Cargo.toml` and confirmed real with
/// `cargo tree -p duded -e features -i dudes-rpc`, which reported `dudes-rpc feature "testing"`
/// under `duded feature "default"`, while every check in this file stayed green.
///
/// A node is a `(package, feature)` pair, so the walk crosses manifests as readily as it crosses
/// entries: a `default` that names a sibling's feature, which names the fakes, is the same search
/// and needs no second rule. `seen` makes a feature cycle terminate rather than recurse — cargo
/// rejects one, but this scan reads text cargo has not necessarily accepted.
///
/// # What it does not close
///
/// - **A dependency rename.** Cargo's `dep/feature` syntax names the *dependency key*, so
///   `myrpc/testing` under a `myrpc = { package = "dudes-rpc" }` entry is this feature, and the
///   walk resolves the name before the `/` against workspace **package** names. [`entry_package`]
///   resolves renames for the dependency scan; nothing resolves them here. **Reproduce it in a
///   third crate, not in the dependent**: the same two lines in any member the shipped binary
///   links that does not already name this crate — measured in `crates/dudes-journal/Cargo.toml`,
///   where `cargo tree -p duded -e features -i dudes-rpc` reported `dudes-rpc feature "testing"`,
///   the 12 checks here stayed green after `cargo update -w`, and `AdmitAll` linked into `duded`.
///   Written in the dependent's own manifest it is not a bypass at all: replacing the entry makes
///   `cargo check -p duded` fail with 17 × `E0433`, because the source names `dudes_rpc`, and
///   adding a second entry makes `cargo metadata` refuse the package outright. `--locked` delays
///   this one until the lockfile is next regenerated. It delays neither of the two below.
/// - **An inline `[features]` table in the preamble.** `features = { default = […] }` on one line
///   reaches [`feature_entries`] as a one-segment path and is discarded unread, so this walk has no
///   `default` to start from. Measured in `bins/duded/Cargo.toml`, with the linking `AdmitAll` and
///   the clean-tree `E0433` as the two arms; the mechanism is written out on [`feature_entries`].
/// - **A second feature of this crate that enables [`FEATURE`] under another name**, which this
///   walk never sees because it starts at `default` and a dependency entry's feature list does not
///   reach it. [`names_feature`] owns that one; it is the cheapest of all of them and it survives
///   `--locked`.
/// - **Escape sequences**, exactly as on [`shipping_activations`]: [`quoted_values`] returns the
///   bytes between the quotes and decodes nothing.
/// - **A manifest outside this workspace**, as everywhere else in this file.
///
/// It is not fail-closed either. It models the spellings below, each of which has a plant.
fn default_route(members: &[Member], package: &str) -> Option<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut route = Vec::new();
    reaches_fakes(members, package, "default", &mut seen, &mut route).then_some(route)
}

/// Depth-first from `(package, feature)`, pushing each node onto `route` and unwinding a dead end
/// so that what comes back is the route to the fakes and not everything visited on the way.
fn reaches_fakes(
    members: &[Member],
    package: &str,
    feature: &str,
    seen: &mut BTreeSet<(String, String)>,
    route: &mut Vec<String>,
) -> bool {
    if !seen.insert((package.to_owned(), feature.to_owned())) {
        return false;
    }
    route.push(format!("{package}/{feature}"));
    if package == PACKAGE && feature == FEATURE {
        return true;
    }
    let values = members
        .iter()
        .find(|member| member.package == package)
        .and_then(|member| {
            member
                .features
                .iter()
                .find(|(name, _)| name == feature)
                .map(|(_, text)| quoted_values(text))
        })
        .unwrap_or_default();
    for value in values {
        let next = match value.split_once('/') {
            // `dep?/feature` is the weak form: it turns the feature on only if something else
            // pulls the dependency in. Something else in a shipped build is still a shipped build.
            Some((dependency, name)) => {
                (dependency.trim_end_matches('?').to_owned(), name.to_owned())
            }
            // `dep:foo` enables an optional dependency, not a feature of one.
            None if value.starts_with("dep:") => continue,
            None => (package.to_owned(), value),
        };
        if reaches_fakes(members, &next.0, &next.1, seen, route) {
            return true;
        }
    }
    route.pop();
    false
}

/// The plants below as a workspace: each manifest's package and `[features]` table.
fn planted_members(manifests: &[&str]) -> Vec<Member> {
    manifests
        .iter()
        .map(|manifest| Member {
            package: package_name(manifest).expect("every plant declares a package"),
            features: feature_entries(manifest),
        })
        .collect()
}

/// [`default_route`] over the plants, joined so an assertion can name the whole route.
fn planted_route(manifests: &[&str], package: &str) -> Option<String> {
    default_route(&planted_members(manifests), package).map(|route| route.join(" -> "))
}

/// A plant manifest for a package that is not the guarded one, with the given `[features]` body.
fn sibling(features: &str) -> String {
    format!("[package]\nname = \"sibling\"\n\n[features]\n{features}\n")
}

// ---------------------------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------------------------

/// `src/lib.rs` declares the module once, behind the gate.
#[test]
fn the_module_is_declared_behind_the_gate() {
    let lib = text_at(&crate_root().join("src").join("lib.rs"));
    assert_eq!(
        declarations(&lib),
        1,
        "src/lib.rs must declare `{DECLARATION}` exactly once; a renamed declaration would make \
         the ungated count below vacuously zero"
    );
    assert_eq!(
        declares_ungated(&lib),
        0,
        "src/lib.rs declares `{DECLARATION}` without `{GATE}` immediately before it, so a default \
         build of this crate contains AdmitAll"
    );
}

/// The feature exists and nothing turns it on by default.
#[test]
fn the_feature_is_off_by_default() {
    let manifest = text_at(&crate_root().join("Cargo.toml"));
    assert!(
        declares_feature(&manifest, FEATURE),
        "Cargo.toml declares no `{FEATURE}` feature; the gate in src/lib.rs then never opens and \
         the test suite cannot compile"
    );
    assert!(
        default_entry(&manifest).is_none(),
        "Cargo.toml's [features] table has a `default` entry ({:?}); this crate is meant to have \
         none, so that \"off unless asked for\" is a property of the table's shape",
        default_entry(&manifest)
    );
    // No "and that entry does not name the feature" assertion here. There was one, and it was
    // dead: it can only run when the assertion above passed, and that one passing means there is
    // no `default` entry for it to read. The reachability question is asked where it can fail —
    // over every manifest in the tree, in `no_default_feature_in_this_workspace_reaches_the_fakes`
    // — because this assertion is sound for the guarded crate and transfers to no other one.
}

/// No build in this workspace turns the feature on outside a dev-dependency.
#[test]
fn no_shipping_dependency_asks_for_the_fakes() {
    let mut offenders: BTreeSet<String> = BTreeSet::new();
    for path in manifest_paths() {
        for entry in shipping_activations(&text_at(&path)) {
            offenders.insert(format!("{}: {entry}", path.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "these manifests link {PACKAGE} with `{FEATURE}` on, which puts AdmitAll in a shipped \
         build; a test that needs the fakes asks for them from [dev-dependencies]: {offenders:#?}"
    );
}

/// No package in this workspace reaches the fakes from its own `default` feature.
///
/// The dependency scan's sibling: a dependency entry is not the only line that turns a feature on,
/// and a `default` table in a manifest that is not this crate's is ordinary rather than suspicious,
/// so it has to be read rather than assumed absent.
#[test]
fn no_default_feature_in_this_workspace_reaches_the_fakes() {
    let members = workspace_members();
    let mut offenders: BTreeSet<String> = BTreeSet::new();
    for member in &members {
        if let Some(route) = default_route(&members, &member.package) {
            offenders.insert(route.join(" -> "));
        }
    }
    assert!(
        offenders.is_empty(),
        "these `default` features turn `{FEATURE}` on for a plain `cargo build` of the package \
         they start from, which puts AdmitAll in a shipped build; a target that needs the fakes \
         asks for them from [dev-dependencies]: {offenders:#?}"
    );
}

/// The declaration scan can fail, in the shape it exists to catch.
///
/// One control per scan rather than one for all three: a single function covering them would say
/// only that *something* is unproven when it goes red, and this is the file whose whole job is
/// telling one silent regression from another.
#[test]
fn the_declaration_scan_catches_a_planted_regression() {
    let gated = format!("pub mod tail;\n// a comment\n{GATE}\n{DECLARATION}\n");
    assert_eq!(
        declarations(&gated),
        1,
        "positive control: the gated form is a declaration"
    );
    assert_eq!(
        declares_ungated(&gated),
        0,
        "the real arrangement — a comment above the attribute, the attribute against the item — \
         must stay legal"
    );
    assert_eq!(
        declares_ungated(&format!("pub mod tail;\n{DECLARATION}\n")),
        1,
        "a declaration with no attribute at all is the regression"
    );
    assert_eq!(
        declares_ungated(&format!("{GATE}\n// a comment\n{DECLARATION}\n")),
        1,
        "a comment between the attribute and the item is a different arrangement, and this test \
         claims the adjacent one"
    );
    assert_eq!(
        declares_ungated(&format!("#[cfg(feature = \"{FEATURE}\")]\n{DECLARATION}\n")),
        1,
        "the bare feature gate is caught: it is the one that does not compile, because the \
         crate's own unit tests reach the fakes through `crate::testing`"
    );
    assert_eq!(
        declarations("pub mod tail;\npub mod system;\n"),
        0,
        "a tree with no declaration is reported as such rather than passing the ungated count"
    );
}

/// The `[features]` scan can fail, in the shape it exists to catch.
#[test]
fn the_feature_scan_catches_a_planted_regression() {
    let bare = format!("[features]\n{FEATURE} = []\n\n[dependencies]\nserde = \"1\"\n");
    assert!(
        declares_feature(&bare, FEATURE),
        "positive control: the feature is declared"
    );
    assert!(
        default_entry(&bare).is_none(),
        "no `default` entry is the shape this crate ships"
    );
    let defaulted = format!("[features]\ndefault = [\"{FEATURE}\"]\n{FEATURE} = []\n");
    assert!(
        default_entry(&defaulted).is_some(),
        "a `default` table of any kind is the regression this check reports; whether it *reaches* \
         the feature is `the_default_feature_scan_catches_a_planted_regression`'s question"
    );
    assert!(
        !declares_feature("[dependencies]\nserde = \"1\"\n", FEATURE),
        "a manifest with no [features] table declares no feature"
    );
    assert!(
        !declares_feature(
            &format!("[features]\n{FEATURE} = []\n[dependencies]\ndefault = \"1\"\n"),
            "default"
        ),
        "the table cut holds: a key of that name in the next table is not a feature"
    );
    // A manifest whose last line has no newline: the table runs to the end and is still read.
    assert!(
        declares_feature(
            &format!("[dependencies]\nserde = \"1\"\n[features]\n{FEATURE} = []"),
            FEATURE
        ),
        "a table that ends at end-of-file with no trailing newline is still a table"
    );
}

/// The dependency scan can fail, in the shape it exists to catch — including the multi-line
/// spelling, which is the one a single-line scan would wave through.
#[test]
fn the_dependency_scan_catches_a_planted_regression() {
    let dev_only = format!(
        "[dependencies]\n{PACKAGE}.workspace = true\n\n[dev-dependencies]\n\
         {PACKAGE} = {{ path = \".\", features = [\"{FEATURE}\"] }}\n"
    );
    assert!(
        shipping_activations(&dev_only).is_empty(),
        "this crate's own arrangement — plain in [dependencies], featured in [dev-dependencies] — \
         must stay legal, or the suite cannot compile"
    );
    let shipped =
        format!("[dependencies]\n{PACKAGE} = {{ workspace = true, features = [\"{FEATURE}\"] }}\n");
    assert_eq!(
        shipping_activations(&shipped).len(),
        1,
        "a normal dependency turning the feature on is the regression"
    );
    let moved =
        format!("[dependencies]\n{PACKAGE} = {{ path = \".\", features = [\"{FEATURE}\"] }}\n");
    assert_eq!(
        shipping_activations(&moved).len(),
        1,
        "the same line moved out of [dev-dependencies] is caught, which is what stops the \
         self-reference from quietly becoming a real dependency"
    );
    let targeted = format!(
        "[target.'cfg(unix)'.dependencies]\n{PACKAGE} = {{ workspace = true, features = \
         [\"{FEATURE}\"] }}\n"
    );
    assert_eq!(
        shipping_activations(&targeted).len(),
        1,
        "a platform-specific dependency table is a shipping one"
    );
    let workspace_table = format!(
        "[workspace.dependencies]\n{PACKAGE} = {{ path = \"crates/{PACKAGE}\", features = \
         [\"{FEATURE}\"] }}\n"
    );
    assert_eq!(
        shipping_activations(&workspace_table).len(),
        1,
        "the workspace table is the widest one: every `workspace = true` inherits it"
    );
    let wrapped = format!(
        "[dependencies]\n{PACKAGE} = {{ workspace = true, features = [\n    \"{FEATURE}\",\n] }}\n"
    );
    assert_eq!(
        shipping_activations(&wrapped).len(),
        1,
        "a wrapped entry is accumulated to its closing brace, so the multi-line spelling — which \
         `cargo fmt` leaves alone — is not a way through"
    );
    assert!(
        shipping_activations(&format!(
            "[dependencies]\n{PACKAGE}.workspace = true\nother = {{ features = [\"{FEATURE}\"] }}\n"
        ))
        .is_empty(),
        "another package's `{FEATURE}` feature is not this one's"
    );
    assert!(
        shipping_activations(&format!(
            "[build-dependencies]\n{PACKAGE} = {{ workspace = true, features = [\"{FEATURE}\"] }}\n"
        ))
        .is_empty(),
        "a build script is not the shipped artefact"
    );
    assert!(
        shipping_activations(&format!(
            "[dependencies]\n# {PACKAGE} = {{ workspace = true, features = [\"{FEATURE}\"] }}\n"
        ))
        .is_empty(),
        "a commented-out dependency is not a dependency"
    );
}

/// The spellings that are the same dependency written another way.
///
/// **A naive scan misses every one of these**, so each is planted in a manifest, with the inline
/// spelling as the control that the check itself reddens. They are the reason the header rule is
/// "one segment is `dependencies`" rather than "the header ends in `dependencies`", and the reason an entry resolves through `package =`.
#[test]
fn the_dependency_scan_catches_the_other_spellings_of_the_same_entry() {
    // The dotted sub-table. `[dependencies.dudes-rpc]` ends with the package name, so a
    // suffix test would read it as "not a dependency table" and skipped its whole body.
    for header in [
        "[dependencies.dudes-rpc]",
        "[workspace.dependencies.dudes-rpc]",
        "[target.'cfg(unix)'.dependencies.dudes-rpc]",
        "[dependencies.\"dudes-rpc\"]",
    ] {
        let planted =
            format!("{header}\npath = \"crates/{PACKAGE}\"\nfeatures = [\"{FEATURE}\"]\n");
        assert_eq!(
            shipping_activations(&planted).len(),
            1,
            "the sub-table spelling under {header} is the same dependency"
        );
    }
    assert!(
        shipping_activations(&format!(
            "[dev-dependencies.{PACKAGE}]\npath = \".\"\nfeatures = [\"{FEATURE}\"]\n"
        ))
        .is_empty(),
        "the dev-dependency sub-table is this crate's own arrangement written the other way, and \
         must stay legal"
    );
    // A sub-table's body ends at the next header, not at the end of the file.
    assert!(
        shipping_activations(&format!(
            "[dependencies.{PACKAGE}]\npath = \".\"\n\n[features]\n{FEATURE} = []\n"
        ))
        .is_empty(),
        "a later table's contents do not belong to the entry above it"
    );

    // A quoted key is the same key.
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies]\n\"{PACKAGE}\" = {{ workspace = true, features = [\"{FEATURE}\"] }}\n"
        ))
        .len(),
        1,
        "a quoted key is the same package as the bare spelling"
    );

    // The dotted inline key: one entry per field, all for the same package.
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies]\n{PACKAGE}.workspace = true\n{PACKAGE}.features = [\"{FEATURE}\"]\n"
        ))
        .len(),
        1,
        "a dotted inline key is an entry for the package before the dot"
    );

    // The rename: the key is not the package.
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies]\nmyrpc = {{ workspace = true, package = \"{PACKAGE}\", features = \
             [\"{FEATURE}\"] }}\n"
        ))
        .len(),
        1,
        "`package = ` names the crate that is linked, whatever the key says"
    );
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies.myrpc]\npackage = \"{PACKAGE}\"\nfeatures = [\"{FEATURE}\"]\n"
        ))
        .len(),
        1,
        "the rename works the same way in a sub-table"
    );
    assert!(
        shipping_activations(&format!(
            "[dependencies]\n{PACKAGE} = {{ package = \"other-crate\", features = [\"{FEATURE}\"] }}\n"
        ))
        .is_empty(),
        "the reverse rename is another crate: our key, someone else's package"
    );
    assert!(
        shipping_activations(&format!(
            "[dependencies]\nsub-package = {{ workspace = true, features = [\"{FEATURE}\"] }}\n"
        ))
        .is_empty(),
        "`package` at the end of a longer word is not the rename field"
    );
}

/// TOML's other string form, which is the same activation to cargo and was not to this scan.
///
/// The negative arm is the last assertion **in this test**, not a neighbour: a shipping dependency
/// on this crate that asks for no features at all must produce no offender. That is what tells a
/// matcher which reads quotes from one which answers yes to everything, and it has to run in the
/// same process to mean anything here. With [`names_feature`] stubbed to `true`, a version of this
/// test without these arms would **pass** — its dev-dependency and reverse-rename arms are decided before the
/// needle is consulted — while three other checks reddened. A control that a mutation leaves green
/// is not this test's control, whatever its name says.
///
/// `dudes-rpc = { workspace = true, features = ['testing'] }` in `bins/duded/Cargo.toml` is a real
/// activation, not only a string: `cargo tree -p duded -e features -i dudes-rpc` reported `dudes-rpc feature "testing"` for it.
#[test]
fn the_dependency_scan_catches_the_literal_string_spelling() {
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies]\n{PACKAGE} = {{ workspace = true, features = ['{FEATURE}'] }}\n"
        ))
        .len(),
        1,
        "a literal-string features list activates the feature exactly as a basic-string one does"
    );
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies.{PACKAGE}]\npath = \".\"\nfeatures = ['{FEATURE}']\n"
        ))
        .len(),
        1,
        "the sub-table spelling reaches the same matcher, so it is caught with either quote"
    );
    assert_eq!(
        shipping_activations(&format!(
            "[dependencies]\nmyrpc = {{ workspace = true, package = '{PACKAGE}', features = \
             ['{FEATURE}'] }}\n"
        ))
        .len(),
        1,
        "the rename field reads a literal string too, so the two spellings do not compound"
    );
    assert!(
        shipping_activations(&format!(
            "[dependencies]\n{PACKAGE} = {{ package = 'other-crate', features = ['{FEATURE}'] }}\n"
        ))
        .is_empty(),
        "the reverse rename is still another crate when it is written with literal strings"
    );
    assert!(
        shipping_activations(&format!(
            "[dev-dependencies]\n{PACKAGE} = {{ path = \".\", features = ['{FEATURE}'] }}\n"
        ))
        .is_empty(),
        "and a dev-dependency stays legal in either quote: the matcher did not become one that \
         rejects everything"
    );
    assert!(
        shipping_activations(&format!(
            "[dependencies]\n{PACKAGE} = {{ workspace = true }}\n"
        ))
        .is_empty(),
        "the negative arm this test owns: a shipping dependency on this crate asking for no \
         features is the ordinary case and must report nothing, which is what a matcher answering \
         yes to everything would fail"
    );
}

/// The `default`-feature walk can fail, in the shapes it exists to catch.
///
/// Every route below is one a manifest can plant, and the other scans in this file stay green on
/// the first two.
#[test]
fn the_default_feature_scan_catches_a_planted_regression() {
    let direct = sibling(&format!("default = [\"{PACKAGE}/{FEATURE}\"]"));
    assert_eq!(
        planted_route(&[&direct], "sibling"),
        Some(format!("sibling/default -> {PACKAGE}/{FEATURE}")),
        "a sibling's `default` naming the feature is the regression; the needle the dependency \
         scan uses cannot see this line, because the quote sits before the package name"
    );
    let quoted = sibling(&format!("default = ['{PACKAGE}/{FEATURE}']"));
    assert_eq!(
        planted_route(&[&quoted], "sibling"),
        Some(format!("sibling/default -> {PACKAGE}/{FEATURE}")),
        "and with the other string form, which is where the same defect lived twice"
    );
    let hop = sibling(&format!(
        "default = [\"extra\"]\nextra = [\"{PACKAGE}/{FEATURE}\"]"
    ));
    assert_eq!(
        planted_route(&[&hop], "sibling"),
        Some(format!(
            "sibling/default -> sibling/extra -> {PACKAGE}/{FEATURE}"
        )),
        "one named hop is the same activation written in two lines"
    );
    let weak = sibling(&format!("default = [\"{PACKAGE}?/{FEATURE}\"]"));
    assert_eq!(
        planted_route(&[&weak], "sibling"),
        Some(format!("sibling/default -> {PACKAGE}/{FEATURE}")),
        "the weak form turns the feature on whenever anything else pulls the dependency in, and \
         something else in a shipped build is still a shipped build"
    );
    let headerless =
        format!("package.name = \"sibling\"\nfeatures.default = [\"{PACKAGE}/{FEATURE}\"]\n");
    assert_eq!(
        planted_route(&[&headerless], "sibling"),
        Some(format!("sibling/default -> {PACKAGE}/{FEATURE}")),
        "the same table written as dotted keys with no header is the same table to cargo"
    );
    let wrapped = sibling(&format!("default = [\n  \"{PACKAGE}/{FEATURE}\",\n]"));
    assert_eq!(
        planted_route(&[&wrapped], "sibling"),
        Some(format!("sibling/default -> {PACKAGE}/{FEATURE}")),
        "a `default` list wrapped over lines is the form `cargo fmt` leaves alone, and one entry \
         accumulator serves the feature reader and the dependency reader alike"
    );
    let helper =
        format!("[package]\nname = \"helper\"\n\n[features]\nx = [\"{PACKAGE}/{FEATURE}\"]\n");
    let via = sibling("default = [\"helper/x\"]");
    assert_eq!(
        planted_route(&[&via, &helper], "sibling"),
        Some(format!(
            "sibling/default -> helper/x -> {PACKAGE}/{FEATURE}"
        )),
        "a hop that leaves the manifest is the same route: a node is a package and a feature"
    );
    let own = format!(
        "[package]\nname = \"{PACKAGE}\"\n\n[features]\ndefault = [\"{FEATURE}\"]\n{FEATURE} = []\n"
    );
    assert_eq!(
        planted_route(&[&own], PACKAGE),
        Some(format!("{PACKAGE}/default -> {PACKAGE}/{FEATURE}")),
        "in the guarded crate's own manifest the bare name is the feature, and this walk says so \
         without needing to be told which manifest it is reading"
    );
}

/// The arrangements a `default` table is allowed to have, which this workspace ships today.
///
/// The other arm of the control: a scan that answered "reachable" for these would redden the suite
/// on `crates/dudes-types`, whose `default = ["std"]` is ordinary and must stay legal.
#[test]
fn the_default_feature_scan_leaves_the_legal_arrangements_alone() {
    assert_eq!(
        planted_route(
            &[&sibling("default = [\"std\"]\nstd = [\"serde/std\"]")],
            "sibling"
        ),
        None,
        "the shape `crates/dudes-types` ships is not an activation of anything here"
    );
    assert_eq!(
        planted_route(
            &[&sibling(&format!(
                "default = [\"{FEATURE}\"]\n{FEATURE} = []"
            ))],
            "sibling"
        ),
        None,
        "a sibling's own feature that happens to be called `{FEATURE}` is not this crate's; the \
         package half of a node is what tells them apart"
    );
    assert_eq!(
        planted_route(
            &[&sibling(&format!(
                "default = []\nfakes = [\"{PACKAGE}/{FEATURE}\"]"
            ))],
            "sibling"
        ),
        None,
        "a feature that is not reachable from `default` is opt-in, which is the whole arrangement \
         this file defends rather than a hole in it"
    );
    assert_eq!(
        planted_route(
            &[&sibling(&format!("default = [\"dep:{PACKAGE}\"]"))],
            "sibling"
        ),
        None,
        "`dep:` turns an optional dependency on, not a feature of one"
    );
    assert_eq!(
        planted_route(
            &[&sibling(&format!("default = [\"other-crate/{FEATURE}\"]"))],
            "sibling"
        ),
        None,
        "another crate's feature of the same name is another crate's"
    );
    assert_eq!(
        planted_route(&[&sibling("std = [\"serde/std\"]")], "sibling"),
        None,
        "a `[features]` table with no `default` entry has no route to walk"
    );
    assert_eq!(
        planted_route(
            &[&sibling("default = [\"a\"]\na = [\"b\"]\nb = [\"a\"]")],
            "sibling"
        ),
        None,
        "a cycle terminates: cargo rejects one, but this scan reads text cargo has not accepted"
    );
}

/// The header classifier every plant above runs through.
#[test]
fn the_header_classifier_separates_shipping_tables_from_the_rest() {
    for shipping in [
        "[dependencies]",
        "[workspace.dependencies]",
        "[target.'cfg(unix)'.dependencies]",
        "[target.'cfg(target_os = \"linux\")'.dependencies]",
    ] {
        assert_eq!(
            classify_header(shipping),
            Table::Shipping,
            "{shipping} is read when this crate is linked into something"
        );
    }
    for (header, key) in [
        ("[dependencies.dudes-rpc]", "dudes-rpc"),
        ("[workspace.dependencies.dudes-rpc]", "dudes-rpc"),
        ("[target.'cfg(unix)'.dependencies.dudes-rpc]", "dudes-rpc"),
        ("[dependencies.\"dudes-rpc\"]", "dudes-rpc"),
    ] {
        assert_eq!(
            classify_header(header),
            Table::ShippingEntry(key.to_owned()),
            "{header} is one entry, keyed on the segment after `dependencies`"
        );
    }
    for other in [
        "[dev-dependencies]",
        "[dev-dependencies.dudes-rpc]",
        "[target.'cfg(windows)'.dev-dependencies]",
        "[build-dependencies]",
        "[build-dependencies.dudes-rpc]",
        "[features]",
        "[package]",
        "[package.metadata]",
    ] {
        assert_eq!(
            classify_header(other),
            Table::Other,
            "{other} is not a table a shipped build reads"
        );
    }
    // The quoted `cfg` predicate holds a dot in this repository's own manifests, and splitting on
    // it would classify a real Unix dependency table as `Other`.
    assert_eq!(
        segments("[target.'cfg(unix)'.dependencies]"),
        vec!["target", "cfg(unix)", "dependencies"],
        "a quoted segment is one segment"
    );
}
