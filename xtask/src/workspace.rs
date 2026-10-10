//! Which checkout is this, and did this binary come from it?
//!
//! `xtask` reads files out of the workspace, so it needs the workspace root. Taking that root from
//! `env!("CARGO_MANIFEST_DIR")` alone is fail-open: the constant is baked at **compile** time, so a
//! binary built from one checkout and re-used by another reports on the checkout that *built* it
//! while the operator reads the output as being about the checkout they are standing in.
//!
//! That re-use is not exotic here — a `CARGO_TARGET_DIR` shared between checkouts produces it, and
//! checkouts often live in separate git worktrees. Cargo does not stop it: freshness is
//! decided from source timestamps, so cargo prints `Fresh xtask (<this checkout>)` and runs the
//! other checkout's binary unchanged. Both faces occur: a `schemas --check` that
//! printed every file `ok` while the committed schema in the current checkout was `{}` (a gate
//! waving real drift through), and one that printed `DIFFERS` against a checkout whose schemas were
//! byte-perfect.
//!
//! So the root is resolved at **run** time and cross-checked against the compile-time constant.
//! When the two disagree, nothing this binary prints describes the current tree — neither the files
//! it reads nor the `dudes-proto` linked into it — and [`locate`] refuses to run, naming both
//! directories. The compile-time constant is kept for exactly that comparison and is never used to
//! find a file.
//!
//! One case is out of reach, deliberately. The cross-check compares *directories*, so a checkout
//! deleted and re-created at the same path reads as the same tree even though the binary was built
//! from the older one. Re-creating a checkout through git stamps current mtimes, which makes cargo
//! rebuild, so the stale binary does not survive to be run; defeating that needs an extraction that
//! restores older timestamps onto the same path. Closing it would mean hashing the tree on every
//! invocation, which is not worth it — but the limit is written here so the next reader finds it in
//! the file instead of rediscovering it.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// xtask's manifest directory as of the build that produced this binary; always `<root>/xtask`.
const BUILT_IN_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The name of xtask's own manifest directory, which is always the workspace root's child `xtask`.
/// A package name rather than a path, so it does not vary with the checkout.
const MANIFEST_DIR_NAME: &str = env!("CARGO_PKG_NAME");

/// The workspace root this process will read, and how it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workspace {
    root: PathBuf,
    found_by: FoundBy,
}

impl Workspace {
    /// The workspace root. Every path a command reads is joined onto this.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

impl fmt::Display for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.root.display(), self.found_by)
    }
}

/// How [`locate`] resolved the root. Printed with the root so a transcript states which tree it
/// describes rather than leaving a reader to assume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoundBy {
    /// `CARGO_MANIFEST_DIR` read from the environment at run time. Cargo sets it for the process it
    /// launches, so under `cargo xtask` it names the checkout *this* cargo invocation resolved —
    /// which is the point: it tracks the current tree even when the binary does not.
    CargoEnv,
    /// Walked up from the current directory to the manifest declaring `[workspace]`. This is the
    /// path taken when the binary is executed directly instead of through `cargo xtask`.
    CurrentDir,
}

impl fmt::Display for FoundBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CargoEnv => "root from CARGO_MANIFEST_DIR at run time",
            Self::CurrentDir => "root found by walking up from the current directory",
        })
    }
}

/// Resolve the workspace root at run time and refuse to continue if this binary was built from a
/// different checkout.
pub(crate) fn locate() -> Result<Workspace> {
    let (root, found_by) = resolve()?;
    locate_from(&root_of(Path::new(BUILT_IN_MANIFEST_DIR))?, root, found_by)
}

/// The refusal itself, with both roots supplied. [`locate`] is its only caller and passes the
/// compile-time one.
///
/// Split out because it is the whole point of this module and a gate that can be deleted without a
/// test noticing is not a gate: the helpers below are each testable in isolation, but the decision
/// they compose into is only reachable here.
fn locate_from(built_from: &Path, resolved: PathBuf, found_by: FoundBy) -> Result<Workspace> {
    if !same_tree(built_from, &resolved) {
        bail!(mismatch(built_from, &resolved, found_by));
    }
    Ok(Workspace {
        root: resolved,
        found_by,
    })
}

/// The root this process should read, before it is cross-checked against the compile-time one.
///
/// Cargo's run-time `CARGO_MANIFEST_DIR` is preferred over the current directory because it is the
/// checkout *this* cargo invocation resolved, so it stays correct when the command is run from a
/// subdirectory — but only when the variable is xtask's own; see [`root_from_manifest_dir`].
fn resolve() -> Result<(PathBuf, FoundBy)> {
    let from_env = std::env::var_os("CARGO_MANIFEST_DIR")
        .and_then(|dir| root_from_manifest_dir(Path::new(&dir)));
    if let Some(root) = from_env {
        return Ok((root, FoundBy::CargoEnv));
    }
    let cwd = std::env::current_dir().context("no current directory")?;
    Ok((walk_up(&cwd)?, FoundBy::CurrentDir))
}

/// The workspace root implied by a run-time `CARGO_MANIFEST_DIR`, or `None` when that variable does
/// not name xtask's own manifest directory.
///
/// Cargo sets the variable for the process it launches, so under `cargo xtask` it is xtask's and
/// its parent is the root. It is also *inherited*: executing this binary directly from inside a
/// cargo process running another member leaves that member's manifest directory in the environment,
/// and taking its parent names `<root>/crates`. The run would then be refused — fail-closed, so it
/// never answers wrongly — but the refusal would blame a shared `CARGO_TARGET_DIR` for what is a
/// stray environment variable in the very checkout the caller is standing in. `None` sends
/// [`resolve`] to the walk instead, which reads the tree it is standing in.
fn root_from_manifest_dir(dir: &Path) -> Option<PathBuf> {
    if dir.file_name() != Some(OsStr::new(MANIFEST_DIR_NAME)) {
        return None;
    }
    dir.parent().map(Path::to_path_buf)
}

/// The workspace root holding `manifest_dir`, which is always `<root>/xtask`.
fn root_of(manifest_dir: &Path) -> Result<PathBuf> {
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .with_context(|| format!("{} has no parent directory", manifest_dir.display()))
}

/// The nearest ancestor of `from` (inclusive) whose `Cargo.toml` declares `[workspace]`.
///
/// Deliberately a line scan rather than a TOML parse: this is only the no-cargo fallback, and
/// whatever it returns is still cross-checked against the compile-time root by [`locate`], so a
/// wrong answer fails closed instead of reading the wrong tree.
fn walk_up(from: &Path) -> Result<PathBuf> {
    for dir in from.ancestors() {
        let Ok(text) = fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        if text
            .lines()
            .any(|line| line.trim_start().starts_with("[workspace]"))
        {
            return Ok(dir.to_path_buf());
        }
    }
    bail!(
        "no Cargo.toml declaring `[workspace]` in {} or any parent directory; run `cargo xtask` from inside the workspace",
        from.display()
    )
}

/// Whether both paths name the same directory.
///
/// Compares resolved paths, so a symlinked prefix (`/tmp` -> `/private/tmp` on macOS) is not read
/// as a different tree. A path that cannot be resolved never matches: the usual reason is that the
/// building checkout has been deleted, and a binary whose origin no longer exists cannot be shown
/// to belong to this tree.
fn same_tree(built_from: &Path, resolved: &Path) -> bool {
    match (built_from.canonicalize(), resolved.canonicalize()) {
        (Ok(built), Ok(here)) => built == here,
        _ => false,
    }
}

/// The refusal: name both directories, say which is which, and name the cause.
fn mismatch(built_from: &Path, root: &Path, found_by: FoundBy) -> String {
    let gone = if built_from.exists() {
        ""
    } else {
        " (no longer exists)"
    };
    format!(
        "this xtask binary was built from a different checkout than the one it was asked to read\n  \
         built from: {built}{gone}\n  \
         reading:    {here} ({found_by})\n\
         Nothing it prints would describe the tree you are standing in: the paths baked into it and \
         the dudes-proto linked into it both belong to the building checkout. A CARGO_TARGET_DIR \
         shared between checkouts is what produces this — cargo decides freshness from source \
         timestamps, so it reports `Fresh xtask` and runs the other checkout's binary. Give this \
         checkout its own target directory, or run `cargo clean -p xtask`, then re-run.",
        built = built_from.display(),
        here = root.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory holding `Cargo.toml` with the given contents.
    fn manifest_dir(dir: &Path, contents: &str) -> PathBuf {
        fs::create_dir_all(dir).expect("create dir");
        fs::write(dir.join("Cargo.toml"), contents).expect("write manifest");
        dir.to_path_buf()
    }

    /// A checkout root that exists on disk, so `same_tree` can canonicalize it.
    fn checkout(dir: &Path) -> PathBuf {
        fs::create_dir_all(dir).expect("create dir");
        dir.to_path_buf()
    }

    #[test]
    fn locate_from_accepts_a_binary_built_from_this_checkout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = checkout(&tmp.path().join("tree"));
        let workspace = locate_from(&root, root.clone(), FoundBy::CurrentDir)
            .expect("one checkout must not read as two");
        assert_eq!(workspace.root(), root, "the resolved root is what is read");
        assert_eq!(workspace.found_by, FoundBy::CurrentDir, "{workspace}");
    }

    #[test]
    fn locate_from_refuses_a_binary_built_from_another_checkout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let built_from = checkout(&tmp.path().join("treeA"));
        let here = checkout(&tmp.path().join("treeB"));
        // Positive control: the accepting arm is reachable with these same inputs.
        locate_from(&here, here.clone(), FoundBy::CargoEnv)
            .expect("control: one tree read as itself must be accepted");
        let err = locate_from(&built_from, here.clone(), FoundBy::CargoEnv)
            .expect_err("a binary built elsewhere must not report on this tree");
        let message = err.to_string();
        assert!(
            message.contains(&built_from.display().to_string()),
            "{message}"
        );
        assert!(message.contains(&here.display().to_string()), "{message}");
    }

    #[test]
    fn only_xtasks_own_manifest_dir_is_taken_from_the_environment() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        assert_eq!(
            root_from_manifest_dir(&root.join("xtask")),
            Some(root.to_path_buf()),
            "xtask's own manifest directory names the root"
        );
        assert_eq!(
            root_from_manifest_dir(&root.join("crates").join("dudes-proto")),
            None,
            "another member's manifest directory, inherited, is not xtask's"
        );
    }

    #[test]
    fn same_tree_holds_for_one_directory_named_two_ways() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let direct = tmp.path().join("xtask");
        fs::create_dir_all(&direct).expect("create dir");
        let roundabout = tmp.path().join("xtask").join("..").join("xtask");
        assert!(same_tree(&direct, &roundabout));
    }

    #[test]
    fn same_tree_fails_for_two_checkouts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("treeA");
        let b = tmp.path().join("treeB");
        fs::create_dir_all(&a).expect("create dir");
        fs::create_dir_all(&b).expect("create dir");
        assert!(!same_tree(&a, &b), "two checkouts must not read as one");
    }

    #[test]
    fn same_tree_fails_when_the_building_checkout_is_gone() {
        let here = tempfile::tempdir().expect("tempdir");
        let gone = {
            let doomed = tempfile::tempdir().expect("tempdir");
            doomed.path().to_path_buf()
        };
        assert!(!gone.exists(), "positive control: the path must be gone");
        assert!(!same_tree(&gone, here.path()));
    }

    #[test]
    fn walk_up_finds_the_workspace_manifest_from_a_member() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = manifest_dir(tmp.path(), "[workspace]\nmembers = [\"member\"]\n");
        let member = manifest_dir(&root.join("member"), "[package]\nname = \"member\"\n");
        let found = walk_up(&member).expect("walk up");
        assert!(
            same_tree(&found, &root),
            "{} != {}",
            found.display(),
            root.display()
        );
    }

    #[test]
    fn walk_up_is_not_fooled_by_workspace_dependencies() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // `[workspace.dependencies]` alone does not make a directory a workspace root.
        let member = manifest_dir(
            &tmp.path().join("member"),
            "[package]\nname = \"member\"\n\n[workspace.dependencies]\nserde = \"1\"\n",
        );
        let root = manifest_dir(tmp.path(), "[workspace]\n");
        let found = walk_up(&member).expect("walk up");
        assert!(
            same_tree(&found, &root),
            "{} != {}",
            found.display(),
            root.display()
        );
    }

    #[test]
    fn walk_up_fails_outside_a_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = walk_up(tmp.path()).expect_err("must not invent a root");
        assert!(
            err.to_string().contains("[workspace]"),
            "message must say what was looked for: {err}"
        );
    }

    #[test]
    fn the_refusal_names_both_directories_and_the_cause() {
        let message = mismatch(
            Path::new("/checkouts/one"),
            Path::new("/checkouts/two"),
            FoundBy::CargoEnv,
        );
        assert!(message.contains("/checkouts/one"), "{message}");
        assert!(message.contains("/checkouts/two"), "{message}");
        assert!(message.contains("CARGO_TARGET_DIR"), "{message}");
        assert!(message.contains("no longer exists"), "{message}");
    }
}
