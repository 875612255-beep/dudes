//! `cargo xtask blake3-pure` — the hash that ships stays on `blake3`'s assembly path.
//!
//! `blake3`'s `pure` feature selects the crate's portable Rust implementation instead of its
//! hand-written SIMD assembly. Both compute the same digest, so a build that turns it on fails
//! nothing and looks green: it just hashes slower everywhere invariant 3's evidence is produced —
//! `LoopDecided.input_digest`, `body_blake3`, `HookTokenMinted.token_hash`, the `PayloadDigest` on
//! a quarantined append. Nothing else in the tree would notice. This gate is the detector.
//!
//! It exists because the portable build is deliberately reachable here — the comment at the pin in
//! `Cargo.toml` gives the invocation — so that `x86_64-pc-windows-msvc` can be type-checked from a
//! host with no MSVC assembler. Two vectors could carry that into a real build, and this gate
//! reads both:
//!
//! - **The resolved feature graph.** `features = ["pure"]` on a dependency edge, a dev-dependency
//!   that unifies into the ordinary build, or the `[target.'cfg(windows)'.dependencies]` form —
//!   which under `resolver = "3"` stays out of every other platform's graph, and so out of every
//!   check that does not name that platform, while still shipping to Windows release users.
//! - **The environment.** `CARGO_FEATURE_PURE` exported in a shell, which is the opt-in itself and
//!   is invisible to the feature graph. A gate that watched only manifests would be blind to the
//!   one mechanism this repository actually uses.
//!
//! # What the measurement covers
//!
//! `cargo metadata --locked` with **default features** — the selection `cargo build`, `cargo test`
//! and `dist` use; passing `--all-features` (as [`crate::metadata::cargo_metadata`] does for the
//! layering gate) would describe a build nobody ships.
//!
//! Unfiltered, its `resolve` is the union over every workspace member, every dependency kind
//! (dev- and build- included) and every target platform: a superset of any one build's selection,
//! so a clean reading is a statement about all of them and the error direction is a false red, not
//! a false green. `--target <triple>` adds `--filter-platform`, which narrows the packages and
//! edges in `resolve` to that platform but **not** the feature set on a node: Cargo unifies
//! features across platforms, so a `[target.'cfg(windows)'.dependencies]` edge turning `pure` on
//! reddens this gate under `--target aarch64-apple-darwin` too — measured, not assumed. The
//! targeted run names the platform that ships; it is not the only reading that catches that form.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The environment variable `blake3`'s build script reads to decide whether to assemble its SIMD
/// sources. Cargo sets it when the feature is on; a shell that exports it reaches the same code
/// without appearing anywhere in the feature graph, which is why this gate reads the environment
/// too.
const OPT_IN: &str = "CARGO_FEATURE_PURE";

/// The dependency whose resolved feature set is being measured.
const PACKAGE: &str = "blake3";

/// The feature that must not be enabled.
const FEATURE: &str = "pure";

/// The flag that narrows the measurement to one platform.
const TARGET: &str = "--target";

/// Read `--target <triple>` (or `--target=<triple>`) out of the command's arguments.
///
/// Optional: with no target the reading is the cross-platform union, which is the wider claim. An
/// unrecognised argument is an error rather than a word this command ignores — a mistyped flag on
/// a gate that then passes anyway is the shape of failure the gate exists to prevent.
pub(crate) fn target(args: &[String]) -> Result<Option<String>> {
    let mut found = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let value = if arg == TARGET {
            Some(
                rest.next()
                    .with_context(|| format!("{TARGET} needs a triple"))?
                    .clone(),
            )
        } else {
            arg.strip_prefix(&format!("{TARGET}=")).map(str::to_owned)
        };
        match value {
            Some(value) if value.is_empty() => bail!("{TARGET} needs a triple"),
            Some(value) => found = Some(value),
            None => bail!("unexpected argument `{arg}`; usage: blake3-pure [{TARGET} <triple>]"),
        }
    }
    Ok(found)
}

/// `cargo xtask blake3-pure [--target <triple>]`.
pub(crate) fn run(target: Option<&str>) -> Result<()> {
    let selection = match target {
        Some(triple) => format!("--filter-platform {triple}"),
        None => "all platforms".to_owned(),
    };
    let enabled = features(&metadata(target)?)?;
    verdict(&enabled, std::env::var_os(OPT_IN).as_deref())
        .with_context(|| format!("gate run over {selection}"))?;
    println!(
        "blake3-pure: {PACKAGE} resolves to features [{}] ({selection}); `{FEATURE}` is not \
         enabled and {OPT_IN} is not set",
        enabled.join(", ")
    );
    Ok(())
}

/// The gate's decision: the resolved feature set of [`PACKAGE`], and whatever [`OPT_IN`] holds in
/// the environment this process was given.
///
/// Both arms live here rather than one here and one at the call site, and the function is separate
/// from [`run`] so they can be exercised without a workspace to resolve. Everything above this
/// point only *reads* a graph, and a reader that is right while the decision is wrong is exactly
/// the shape of failure this gate exists to prevent: a check that stays green over a tree that has
/// leaked is worse than no check.
fn verdict(enabled: &[String], opt_in: Option<&OsStr>) -> Result<()> {
    if let Some(value) = opt_in {
        bail!(
            "{OPT_IN}={} is set in this process's environment, so every cargo command run from \
             this shell builds {PACKAGE} without its assembly. That variable is the cross-check \
             opt-in: it belongs on the one invocation that needs it, never exported.",
            value.to_string_lossy()
        );
    }
    if enabled.iter().any(|feature| feature == FEATURE) {
        bail!(
            "{PACKAGE} resolves with `{FEATURE}` enabled: features [{}]. A build from this graph \
             hashes with the portable implementation.",
            enabled.join(", ")
        );
    }
    Ok(())
}

/// Run `cargo metadata` for the default feature selection, optionally filtered to one platform.
///
/// Deliberately not [`crate::metadata::cargo_metadata`]: that one passes `--all-features`, which
/// answers a different question than "what does an ordinary build resolve to".
fn metadata(target: Option<&str>) -> Result<Value> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command.args(["metadata", "--format-version", "1", "--locked"]);
    if let Some(triple) = target {
        command.args(["--filter-platform", triple]);
    }
    let output = command.output().context("failed to run `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` exited with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("`cargo metadata` output is not JSON")
}

/// Every feature enabled on every resolved copy of [`PACKAGE`], as one set.
///
/// Fails closed on anything it cannot read. A gate whose answer is "clean" because it found no
/// package, no resolve graph, or a node shape it did not recognise is worse than no gate: a
/// renamed package or a metadata format change would read exactly like a build that is fine.
/// Two resolved versions are both measured, so the feature cannot hide on the copy nobody looked
/// at.
fn features(metadata: &Value) -> Result<Vec<String>> {
    let ids: BTreeSet<&str> = metadata["packages"]
        .as_array()
        .context("metadata has no `packages`")?
        .iter()
        .filter(|package| package["name"].as_str() == Some(PACKAGE))
        .map(|package| {
            package["id"]
                .as_str()
                .with_context(|| format!("a `{PACKAGE}` package entry has no id"))
        })
        .collect::<Result<_>>()?;
    if ids.is_empty() {
        bail!(
            "`{PACKAGE}` is not in this workspace's `packages`: this gate measured nothing. If the \
             hash moved to another crate, this command moves with it."
        );
    }

    let mut enabled = BTreeSet::new();
    let mut measured = 0_usize;
    for node in metadata["resolve"]["nodes"]
        .as_array()
        .context("metadata has no `resolve.nodes`")?
    {
        let id = node["id"].as_str().context("resolve node without id")?;
        if !ids.contains(id) {
            continue;
        }
        measured += 1;
        for feature in node["features"]
            .as_array()
            .with_context(|| format!("resolve node {id} has no `features` array"))?
        {
            enabled.insert(
                feature
                    .as_str()
                    .with_context(|| format!("resolve node {id} lists a non-string feature"))?
                    .to_owned(),
            );
        }
    }
    if measured == 0 {
        bail!("`{PACKAGE}` is in `packages` but has no node in `resolve.nodes`");
    }
    Ok(enabled.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A resolved feature set, as [`features`] hands one to [`verdict`].
    fn resolved(features: &[&str]) -> Vec<String> {
        features.iter().map(|f| (*f).to_owned()).collect()
    }

    /// The control every red below is measured against. A `verdict` that refused everything would
    /// satisfy all three failure tests and nothing else; this is the test that stops it.
    #[test]
    fn a_clean_set_with_no_opt_in_passes() {
        assert!(verdict(&resolved(&["default", "std"]), None).is_ok());
    }

    /// The environment arm. `pure` is nowhere in the graph, so only the exported variable can fail
    /// this — which is the vector a manifest-only gate is blind to.
    ///
    /// Cargo sets `CARGO_FEATURE_<NAME>` to `1`, but blake3 asks only whether the variable exists
    /// (`build.rs:3-9`), so the empty value is the feature *on*. Reading the value rather than its
    /// presence would be a gate that a bare `CARGO_FEATURE_PURE=` walks straight past.
    #[test]
    fn the_opt_in_fails_the_gate_over_a_clean_set() {
        let clean = resolved(&["default", "std"]);
        let err = verdict(&clean, Some(OsStr::new("1")))
            .unwrap_err()
            .to_string();
        assert!(err.contains(OPT_IN), "{err}");
        assert!(verdict(&clean, Some(OsStr::new(""))).is_err());
    }

    /// The feature arm, with no opt-in set: the resolved set alone has to fail the gate.
    #[test]
    fn pure_in_the_resolved_set_fails_the_gate() {
        let leaking = resolved(&["default", "pure", "std"]);
        let err = verdict(&leaking, None).unwrap_err().to_string();
        assert!(err.contains(FEATURE), "{err}");
        assert!(err.contains(PACKAGE), "{err}");
    }

    /// The name of the variable is not free text: Cargo exposes a feature to a build script as
    /// `CARGO_FEATURE_<NAME>`, so watching any other spelling watches something blake3 never reads
    /// and reports clean over a shell that has the portable implementation switched on. Derived
    /// from [`FEATURE`] rather than restated as a literal, so what is pinned is the convention —
    /// and renaming the feature this gate guards moves the variable with it.
    #[test]
    fn the_watched_variable_is_the_one_cargo_sets_for_the_feature() {
        assert_eq!(OPT_IN, format!("CARGO_FEATURE_{}", FEATURE.to_uppercase()));
    }

    /// A `cargo metadata` document carrying the named packages, each `(id, name, features)`, with
    /// a resolve node for every one of them.
    fn metadata(packages: &[(&str, &str, &[&str])]) -> Value {
        json!({
            "packages": packages
                .iter()
                .map(|(id, name, _)| json!({ "id": id, "name": name }))
                .collect::<Vec<_>>(),
            "resolve": {
                "nodes": packages
                    .iter()
                    .map(|(id, _, features)| json!({ "id": id, "features": features }))
                    .collect::<Vec<_>>(),
            },
        })
    }

    #[test]
    fn a_default_graph_reports_the_features_it_has() {
        let value = metadata(&[
            ("blake3 1.8.7", "blake3", &["default", "std"]),
            ("serde 1.0.229", "serde", &["derive"]),
        ]);
        assert_eq!(features(&value).unwrap(), vec!["default", "std"]);
    }

    #[test]
    fn pure_in_the_graph_is_reported() {
        let value = metadata(&[("blake3 1.8.7", "blake3", &["default", "pure", "std"])]);
        assert!(features(&value).unwrap().iter().any(|f| f == FEATURE));
    }

    /// A second resolved version is not a hiding place. The positive control is the same fixture
    /// with `pure` on neither copy: if the reader stopped at the first matching node, this test
    /// would pass for the wrong reason and the control would still be green.
    #[test]
    fn a_second_resolved_version_is_measured_too() {
        let hidden = metadata(&[
            ("blake3 1.8.7", "blake3", &["default", "std"]),
            ("blake3 1.9.0", "blake3", &["pure"]),
        ]);
        assert_eq!(features(&hidden).unwrap(), vec!["default", "pure", "std"]);

        let control = metadata(&[
            ("blake3 1.8.7", "blake3", &["default", "std"]),
            ("blake3 1.9.0", "blake3", &["std"]),
        ]);
        assert_eq!(features(&control).unwrap(), vec!["default", "std"]);
    }

    /// Fail closed: a graph without the package is an error, not a clean bill of health. Control:
    /// the same document with the package present reads fine, so the failure is the absence and
    /// not the fixture.
    #[test]
    fn an_absent_package_is_an_error() {
        let absent = metadata(&[("serde 1.0.229", "serde", &["derive"])]);
        assert!(features(&absent).is_err());

        let present = metadata(&[
            ("serde 1.0.229", "serde", &["derive"]),
            ("blake3 1.8.7", "blake3", &["std"]),
        ]);
        assert_eq!(features(&present).unwrap(), vec!["std"]);
    }

    /// Fail closed on a resolve graph that does not describe the package, and on one that is not
    /// there at all.
    #[test]
    fn an_unresolved_package_is_an_error() {
        let unresolved = json!({
            "packages": [{ "id": "blake3 1.8.7", "name": "blake3" }],
            "resolve": { "nodes": [] },
        });
        assert!(features(&unresolved).is_err());

        let no_resolve = json!({ "packages": [{ "id": "blake3 1.8.7", "name": "blake3" }] });
        assert!(features(&no_resolve).is_err());
    }

    #[test]
    fn the_target_flag_is_read_in_both_spellings_and_nothing_else_is() {
        let spaced = ["--target".to_owned(), "x86_64-pc-windows-msvc".to_owned()];
        let joined = ["--target=x86_64-pc-windows-msvc".to_owned()];
        assert_eq!(
            target(&spaced).unwrap().as_deref(),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(target(&joined).unwrap(), target(&spaced).unwrap());
        assert_eq!(target(&[]).unwrap(), None);
        assert!(target(&["--target".to_owned()]).is_err());
        assert!(target(&["--targt=x86_64-pc-windows-msvc".to_owned()]).is_err());
    }
}
