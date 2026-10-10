//! The dev-only cargo features are off by default, and this is what fails when one stops being.
//!
//! Two features in this package exist so that an acceptance run can do something a released daemon
//! must never do: `fault-injection` crashes the daemon at a chosen point, and `stub-provider`
//! replaces the provider CLI with a program that spends no vendor credit. Both are safe only
//! because the code is *absent* from an ordinary build — not refused at runtime, absent — and that
//! absence rests entirely on no manifest naming them in a `default` list.
//!
//! **Until this file existed, that rested on a sentence.** `bins/dudes/src/env.rs` states of the
//! fault variables that they are read "only under `duded`'s `fault-injection` feature, which no
//! manifest in the workspace enables", and nothing checked it. A default added in a hurry would
//! have shipped a crash seam, or a daemon that reads an environment variable to decide which
//! binary is the provider, and every test in the workspace would still have been green.
//!
//! # What this covers, and what it does not
//!
//! It reads every `Cargo.toml` in the workspace and asserts that no `[features] default = [...]`
//! names either feature, and that no dependency edge enables one through `features = [...]`. That
//! is the whole mechanism by which a feature becomes on-by-default, so the coverage matches the
//! claim.
//!
//! It does **not** check what a CI workflow or a human types on a command line. `--features
//! stub-provider` on the command line is exactly how the acceptance run is meant to build, so a
//! check that forbade it would forbid the feature's only purpose.

use std::path::{Path, PathBuf};

/// The features that must never be on by default, and why each one would be a defect.
const DEV_ONLY_FEATURES: &[(&str, &str)] = &[
    (
        "fault-injection",
        "the acceptance run's crash arms; a released daemon must not contain code that stalls it",
    ),
    (
        "stub-provider",
        "the credit-free provider used by the cost-reporting check; a released daemon must not take its provider command from \
         the environment",
    ),
];

/// Every `Cargo.toml` in the workspace, found by walking rather than by a list that rots.
fn manifests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target/` holds vendored manifests that are not this workspace's to police, and
                // `.git` holds none at all.
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name != "target" && name != ".git" && name != "node_modules" {
                    stack.push(path);
                }
            } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The workspace root, from this package's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("Cargo.lock").is_file())
        .expect("a workspace root above this package")
        .to_path_buf()
}

#[test]
fn no_manifest_turns_a_dev_only_feature_on_by_default() {
    let root = workspace_root();
    let manifests = manifests(&root);
    // Anti-vacuity: a walk that found nothing would pass this test in silence.
    assert!(
        manifests.len() >= 5,
        "the manifest walk found {} files under {}, which is too few to be this workspace",
        manifests.len(),
        root.display()
    );
    assert!(
        manifests
            .iter()
            .any(|path| path.ends_with("bins/duded/Cargo.toml")),
        "the manifest walk did not reach the package that declares both features"
    );

    let mut offences = Vec::new();
    for manifest in &manifests {
        let Ok(text) = std::fs::read_to_string(manifest) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            // `required-features` is the opposite of enabling — it *restricts* a target to builds
            // where the feature is already on, which is how `examples/stub-agent.rs` is kept out
            // of an ordinary build. Skipping it is not a hole: a line that only narrows what is
            // built cannot turn anything on.
            if line.starts_with("required-features") {
                continue;
            }
            // The two shapes that turn a feature on without anyone asking: this package's own
            // `default = [...]`, and a dependency edge that names it in `features = [...]`.
            let enabling = line.starts_with("default") && line.contains('[')
                || line.contains("features") && line.contains('[');
            if !enabling {
                continue;
            }
            for (feature, why) in DEV_ONLY_FEATURES {
                if line.contains(&format!("\"{feature}\"")) {
                    offences.push(format!(
                        "{}: `{}` enables `{feature}` — {why}",
                        manifest.strip_prefix(&root).unwrap_or(manifest).display(),
                        line,
                    ));
                }
            }
        }
    }

    assert!(
        offences.is_empty(),
        "a dev-only feature is on by default:\n  {}",
        offences.join("\n  ")
    );
}

/// The features exist to be found: if one is renamed away, the list above must follow it.
///
/// Without this, deleting `stub-provider` from the manifest would leave the test above passing
/// forever on a name nothing declares — green for the wrong reason, which is the failure mode the
/// whole file exists to remove.
#[test]
fn every_dev_only_feature_is_declared_somewhere() {
    let root = workspace_root();
    let declared: String = manifests(&root)
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();
    for (feature, _) in DEV_ONLY_FEATURES {
        assert!(
            declared.contains(&format!("{feature} = [")),
            "`{feature}` is in the dev-only list but no manifest declares it; either the feature \
             was renamed and this list did not follow, or the list has rotted"
        );
    }
}
