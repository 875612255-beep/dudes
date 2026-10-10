//! The constant-time comparison is named in **exactly one module**, and both secrets route through
//! it.
//!
//! # Why this is a source scan and not a unit test
//!
//! Constant-time equality is not observable from a unit test: a `derive(PartialEq)` and a
//! `subtle::ConstantTimeEq` return the same answers, and a timing measurement inside `cargo test`
//! measures the runner. So what can be checked is *where the primitive is used*, and this is a
//! weaker check than a behavioural one — named as weaker rather than passed off as equal.
//!
//! What it does catch is the realistic regression. There are two secrets on this wire and there
//! will be more; each new one is a chance to reach for `#[derive(PartialEq)]` because it compiles.
//! Routing both through one module means a secret that stops being constant-time has to *delete a
//! call*, which shows up in a diff, rather than quietly not add one.
//!
//! # The scan reads code, because prose satisfied it
//!
//! `secret.rs` documents the property it implements, and both `ConstantTimeEq` and `ct_eq` appear
//! in its doc comments as well as in its code. A scan over the whole file text was therefore
//! satisfied by the documentation: deleting the real `use` and the real `ct_eq` call — leaving
//! `a == b`, a byte-at-a-time comparison — left this suite green, because the sentences describing
//! the deleted call were still there.
//!
//! So [`code_text`] drops every line whose first non-whitespace is `//` before the scan runs, and
//! [`the_scan_catches_a_planted_comparison`] plants the prose-only shape and asserts the scan is
//! *not* satisfied by it, with the whole text as the positive control. The residual is that the
//! cut is line-based rather than lexical: a comment appended to the end of a line of code still
//! counts, and a scan that tokenised Rust would be a different and much larger thing.

use std::fs;
use std::path::{Path, PathBuf};

/// The module allowed to name `subtle`'s comparison, relative to `src/`.
const THE_PRIMITIVE: &str = "rpc/secret.rs";

/// The shapes that mean "this code compares bytes in constant time itself".
const NAMES_SUBTLE: &[&str] = &["ConstantTimeEq", "ct_eq"];

/// The secrets on this wire: the file that holds each, relative to `src/`, and its type name. Each
/// must route through the primitive rather than implement one.
const SECRETS: &[(&str, &str)] = &[
    ("rpc/hook.rs", "HookToken"),
    ("rpc/cred.rs", "OperatorCredential"),
];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

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

fn names_subtle(text: &str) -> bool {
    NAMES_SUBTLE.iter().any(|name| text.contains(name))
}

/// `text` with CRLF line endings normalised to LF.
///
/// A needle that spans a line break matches or misses on line endings alone — green on two legs
/// of a three-OS matrix and wrong on the third, where nobody looks, which is the exact failure
/// these scans exist to prevent. It reached CI once, on the derive-list assertion below.
///
/// A checkout attribute governs only what git writes: [`source_text`] is handed a *path*, and a
/// `\r\n` source can reach it from a tree git never checked out. So the normalisation belongs at
/// the read and stays whatever the repository's `.gitattributes` does or does not say.
///
/// Two of the three cuts in this family survive CRLF by accident: `str::lines` strips a trailing
/// `\r`, and `"\n#[cfg(test)]"` is still a substring of `"\r\n#[cfg(test)]"`. Normalising at the
/// read is what stops the next needle from depending on that.
fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The source at `path`, read with line endings normalised.
fn source_text(path: &Path) -> String {
    lf(&fs::read_to_string(path).unwrap())
}

/// `text` with every whole-line comment removed, so the scan reads what the compiler reads.
///
/// A line whose first non-whitespace characters are `//` is prose — doc comment (`///`, `//!`) or
/// plain — and describing a constant-time comparison is not performing one. See this file's header
/// for why that distinction is the difference between a check and a green suite.
fn code_text(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One module names the constant-time primitive; every other source under `src/` does not.
#[test]
fn the_constant_time_comparison_is_named_in_exactly_one_module() {
    let src = crate_root().join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);

    let mut naming: Vec<PathBuf> = files
        .iter()
        .filter(|path| names_subtle(&code_text(&source_text(path))))
        .map(|path| path.strip_prefix(&src).unwrap().to_path_buf())
        .collect();
    naming.sort();

    assert_eq!(
        naming,
        vec![PathBuf::from(THE_PRIMITIVE)],
        "only {THE_PRIMITIVE} may name a constant-time comparison"
    );
    assert!(
        files.len() >= 12,
        "scanned only {} files; is src/ where it was?",
        files.len()
    );
}

/// Both secret types route their equality and their `Debug` through that module.
///
/// The positive control for the scan above: if `secret.rs` were the only file naming `subtle`
/// because nothing used it any more, that test would still pass and this one would not.
#[test]
fn every_secret_routes_through_the_shared_shape() {
    for (name, type_name) in SECRETS {
        let text = source_text(&crate_root().join("src").join(name));
        assert!(
            text.contains("secret::eq("),
            "{name} must compare through the shared constant-time helper"
        );
        assert!(
            text.contains("secret::redacted("),
            "{name} must redact through the shared helper"
        );
        assert!(
            text.contains("secret::well_formed("),
            "{name} must validate through the shared helper"
        );
        // The *secret* has no `Display`; its error type does, and that one carries no text of the
        // value it refused. Naming the type rather than the trait is what keeps the two apart.
        assert!(
            !text.contains(&format!("impl fmt::Display for {type_name}")),
            "{type_name} is a secret and must have no Display: expose() is the only way out"
        );
        // The whole derive list, matched exactly. `Eq` and nothing else: no `Clone` (a secret
        // whose copies nobody counted), no derived `PartialEq` (which would be a byte-at-a-time
        // comparison), no derived `Debug` (which would print the value).
        assert!(
            text.contains(&format!("#[derive(Eq)]\npub struct {type_name}(String);")),
            "{type_name} must derive Eq and nothing else"
        );
    }
}

/// The scan can fail: planted text is caught, and text that merely mentions the words in prose is
/// not mistaken for a second implementation.
#[test]
fn the_scan_catches_a_planted_comparison() {
    assert!(names_subtle("        self.0.as_bytes().ct_eq(other)"));
    assert!(names_subtle("use subtle::ConstantTimeEq;"));
    assert!(
        !names_subtle("    /// Constant-time over the bytes, so a comparison does not leak."),
        "a doc comment describing the property is not an implementation of it"
    );

    // The shape that made this scan pass on a file with no constant-time comparison left in it:
    // the call deleted, the sentences about the call kept. Both arms, because the claim is that
    // the *cut* is what separates them — the raw text names the primitive and the code does not.
    let prose_only = "\
//! `ConstantTimeEq` is named exactly once in this crate, below.
    /// `ct_eq` is defined for equal-length slices.
    pub(super) fn eq(a: &str, b: &str) -> bool {
        a == b
    }
";
    assert!(
        names_subtle(prose_only),
        "positive control: the raw text does name the primitive, twice"
    );
    assert!(
        !names_subtle(&code_text(prose_only)),
        "a deleted call whose documentation survived must not satisfy the scan"
    );
    let real = "\
    use subtle::ConstantTimeEq;
    pub(super) fn eq(a: &str, b: &str) -> bool {
        a.as_bytes().ct_eq(b.as_bytes()).into()
    }
";
    assert!(
        names_subtle(&code_text(real)),
        "positive control: a real call survives the cut, so the cut is not removing everything"
    );
    assert_eq!(
        code_text("fn f() {}\n"),
        "fn f() {}",
        "a file with no comment lines keeps its code"
    );

    // The derive check above is exact, so the control is that an inexact list fails it.
    let planted = "#[derive(Clone, Eq)]\npub struct HookToken(String);";
    assert!(!planted.contains("#[derive(Eq)]\npub struct HookToken(String);"));
}

/// A Windows checkout is read the same as a Unix one — and the needle really does miss without it.
///
/// This is the failure that reached CI: `every_secret_routes_through_the_shared_shape` matched a
/// needle spanning a line break against raw file text, so it passed on macOS and Linux and failed
/// on `windows-latest` alone. Deleting the `\r\n` normalisation in [`lf`] turns the second
/// assertion red, which is what makes this a control rather than a restatement.
#[test]
fn the_derive_needle_reads_a_crlf_checkout_the_same_as_an_lf_one() {
    let needle = "#[derive(Eq)]\npub struct HookToken(String);";
    let crlf = "#[derive(Eq)]\r\npub struct HookToken(String);\r\n";

    assert!(
        !crlf.contains(needle),
        "control: the needle must miss un-normalised, or normalising proves nothing"
    );
    assert!(
        lf(crlf).contains(needle),
        "the same source, line endings normalised, must match"
    );

    // The comment cut is line-based, and `str::lines` already strips a trailing carriage return —
    // so this arm holds with or without `lf`. It is here to pin that, not to test `lf`: it is why
    // only the derive assertion broke on Windows and the two scans either side of it did not.
    assert_eq!(
        code_text("// prose\r\nfn f() {}\r\n"),
        "fn f() {}",
        "the line-based cut is CRLF-safe on its own"
    );
}
