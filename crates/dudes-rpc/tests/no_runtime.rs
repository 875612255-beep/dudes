//! Invariant 9, from inside the crate: nothing under `src/`, and nothing in this crate's manifest,
//! names an async runtime, a TLS stack or a tracing subscriber.
//!
//! `cargo xtask layering` is the gate, and it is the stronger check — it walks the resolved
//! `cargo metadata --all-features` graph, so it sees a runtime arriving through any dependency at
//! any depth. This scan is the near half of the same rule, and it catches the thing the gate cannot
//! yet: **the gate's `dudes-hook` row reports `skipped` until `bins/dudes-hook` is a workspace
//! member.** What can be proved today is the transitive precondition — this crate reaches no
//! runtime, therefore nothing reaches one *through* this crate — and this file adds the source-level
//! half of it, which fails on the first line of an `async fn` rather than on the dependency edge
//! that would follow it.
//!
//! The scan lives in `tests/`, which it does not scan; the tokens below appear in this file by
//! necessity and would otherwise flag it.

use std::fs;
use std::path::{Path, PathBuf};

/// Tokens no source of this crate may contain, matched as substrings with comments included.
///
/// Both spellings of each crate name (`async-std` and `async_std`) because one is how a manifest
/// writes it and the other is how a `use` does. `async fn` and `.await` are here as well: a crate
/// that links no runtime has no business having either, and they fail earlier and more clearly than
/// the dependency edge that would have to follow.
const FORBIDDEN: &[&str] = &[
    "tokio",
    "async-std",
    "async_std",
    "smol",
    "async-io",
    "async_io",
    "rustls",
    "native-tls",
    "native_tls",
    "openssl",
    "tracing-subscriber",
    "tracing_subscriber",
    "async fn",
    ".await",
];

/// The forbidden tokens `text` contains. The one place the rule is applied, so the scan and the
/// self-test that proves the scan works cannot drift apart.
fn violations(text: &str) -> Vec<&'static str> {
    FORBIDDEN
        .iter()
        .copied()
        .filter(|token| text.contains(token))
        .collect()
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

/// Invariant 9: no source of this crate names a runtime, a TLS stack or a subscriber — not in code,
/// and not in a doc comment that a later reader would take as permission.
#[test]
fn no_source_names_a_runtime() {
    let mut files = Vec::new();
    rust_files(&crate_root().join("src"), &mut files);
    for path in &files {
        let found = violations(&fs::read_to_string(path).unwrap());
        assert!(
            found.is_empty(),
            "{} mentions {found:?}; this crate links no async runtime (invariant 9)",
            path.display()
        );
    }
    assert!(
        files.len() >= 7,
        "scanned only {} files; is src/ where it was?",
        files.len()
    );
}

/// The manifest is the direct half of the same rule, and the more binding one: an optional
/// dependency behind any feature is still an edge in the feature-unified graph the layering gate
/// walks, so "not optional either" has to be checked as text.
#[test]
fn the_manifest_names_no_runtime() {
    let manifest = fs::read_to_string(crate_root().join("Cargo.toml")).unwrap();
    let found = violations(&manifest);
    assert!(
        found.is_empty(),
        "Cargo.toml mentions {found:?}; not even behind a feature (invariant 9)"
    );
    assert!(
        manifest.contains("[dependencies]"),
        "positive control: the manifest was read and has a dependency table"
    );
}

/// The scan above passes by finding nothing, which proves nothing unless the scan can fail: a typo
/// in [`FORBIDDEN`] would sit there green forever. This plants each class of forbidden token and
/// asserts it is caught, and plants a clean source and asserts it is not.
#[test]
fn the_scan_catches_a_planted_token() {
    for (planted, expected) in [
        ("tokio = { workspace = true }", "tokio"),
        ("use tokio::net::UnixListener;", "tokio"),
        ("async-std = \"1\"", "async-std"),
        ("use async_std::task;", "async_std"),
        ("smol::block_on(fut)", "smol"),
        ("use async_io::Async;", "async_io"),
        ("rustls = \"0.23\"", "rustls"),
        ("use native_tls::TlsConnector;", "native_tls"),
        ("tracing-subscriber = \"0.3\"", "tracing-subscriber"),
        ("pub async fn serve() {}", "async fn"),
        ("let conn = listener.accept().await?;", ".await"),
    ] {
        // `contains(&expected)`, not `!is_empty()`: with overlapping tokens the wrong entry could
        // otherwise answer for this one, and the case would prove nothing about `expected`.
        assert!(
            violations(planted).contains(&expected),
            "the scan did not catch `{expected}` in `{planted}`"
        );
    }
    assert_eq!(
        violations("pub fn serve(listener: &impl Listener) -> std::io::Result<()> { Ok(()) }"),
        Vec::<&str>::new(),
        "a clean source must not be flagged"
    );
    // A tripwire, not a coverage claim: a change to the list is a prompt to check whether the new
    // entry needs a planted case of its own.
    assert_eq!(
        FORBIDDEN.len(),
        14,
        "the forbidden list changed; check whether the new entry needs a planted case above"
    );
}
