//! `cargo xtask journal` / `export` / `gate` — the offline evidence the end-to-end gate transcript reads,
//! and the runner that makes the transcript reachable rather than run once.
//!
//! # Why these live here and not in the `dudes` CLI
//!
//! Both readers must work **with no daemon running**. redb refuses a second open of a held store —
//! and after the crash arm's `SIGKILL` there is no daemon left to ask. The `dudes` CLI must not read
//! `state.redb` directly, and it has no `replay` command that could do so on its behalf. Repository
//! automation is the smaller change: it adds nothing to the operator's method surface and no row
//! to any authority table.
//!
//! # What each is for
//!
//! - [`journal`] — the log as text, one line per record: `<lsn> <kind> <record as JSON>`. The
//!   transcript's PASS/FAIL checks read this instead of a JSON processor, so the gate needs no
//!   `jq` on a machine that has never built this repo.
//! - [`export`] — the **projection export**, from the store or from a from-scratch replay
//!   of the journal. Arm 2's identity is the two being equal.
//! - [`run`] — the three arms in order, every attempt, `exit(failures ? 1 : 0)`.
//!
//! Every one of them reads the shipped `dudes-journal` types. Nothing here re-implements a fold, a
//! decision or a record shape: a checker that carried its own copy would keep passing after the
//! daemon drifted away from it, which is the one failure an acceptance harness must not have.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use dudes_journal::{Projections, Reader, replay};

use crate::workspace::Workspace;

/// The journal's directory inside a state directory.
///
/// **Owned by `bins/duded/src/boot.rs`** (`JOURNAL_DIR`), which is where the daemon creates it.
///
/// This is a second copy, deliberately, and [`tests::the_state_layout_matches_the_daemons`] is what
/// keeps it from becoming drift: the test reads the daemon's own declaration and compares. Using
/// the constant instead would mean repository automation linking the daemon's library — the single
/// writer's lock, its RPC surface, its host seams — to learn two directory names, which is a
/// dependency direction nothing here needs and nobody has argued for. (It would link no async
/// runtime: `cargo tree -p duded -e normal` shows none, so `cargo xtask layering`'s row for this
/// member is not the reason.)
const JOURNAL_DIR: &str = "journal";

/// The projection store inside a state directory. Owned by the same file (`PROJECTION_FILE`).
const PROJECTION_FILE: &str = "state.redb";

/// The example directory the gate arms live in, relative to the workspace root.
const EXAMPLE_DIR: &str = "examples/two-roles";

/// Print the journal under `state`, one line per record.
///
/// Each line is `<lsn> <kind> <record as JSON>`: the position, the record kind as the contract
/// spells it, and the record itself. Three fields with the two cheap ones first, so a caller can
/// select on the kind with `awk '$2 == "turn_issued"'` and still have the whole record on the line
/// when it needs a field out of it.
pub(crate) fn journal(state: &Path) -> Result<()> {
    let dir = state.join(JOURNAL_DIR);
    let reader = Reader::new(&dir);
    for item in reader
        .iter_from(dudes_types::Lsn::ZERO)
        .with_context(|| format!("reading the journal at {}", dir.display()))?
    {
        let (lsn, entry) = item?;
        let kind = serde_json::to_string(&entry.record.kind())?;
        let record = serde_json::to_string(&entry.record)?;
        // `kind` serialises as a quoted string; the quotes would be noise in a column a script
        // matches on, so they come off here rather than in every caller.
        println!("{} {} {record}", lsn.0, kind.trim_matches('"'));
    }
    Ok(())
}

/// Where an [`export`] takes its rows from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The `state.redb` the daemon left behind.
    Store,
    /// A from-scratch replay of the journal into a projection that has folded nothing.
    Journal,
}

/// Print the projection export of the state directory `state`.
///
/// The two sources are the two sides of arm 2's identity. `Store` opens what the daemon left —
/// which, on the crash arm, is a store that was restarted and replayed. `Journal` builds a
/// projection from nothing and folds the same log into it. Equal exports mean the daemon rebuilt
/// what it had; unequal ones name the row, because the document is one row per line.
pub(crate) fn export(state: &Path, source: Source) -> Result<()> {
    let projections = match source {
        Source::Store => {
            let path = state.join(PROJECTION_FILE);
            if !path.exists() {
                bail!("no projection store at {}", path.display());
            }
            Projections::open(&path)
                .with_context(|| format!("opening the projection store {}", path.display()))?
        }
        Source::Journal => {
            let dir = state.join(JOURNAL_DIR);
            let mut fresh = Projections::in_memory().context("a fresh in-memory projection")?;
            replay(&Reader::new(&dir), &mut fresh)
                .with_context(|| format!("replaying the journal at {}", dir.display()))?;
            fresh
        }
    };
    print!("{}", projections.export()?);
    Ok(())
}

/// Run the end-to-end gate: arm 1, arm 2 and the control, in that order, every time.
///
/// The control runs on **every** attempt: a crash arm with no
/// no-crash arm cannot tell "replay re-issued exactly one turn" from "this harness only ever
/// produces one child".
///
/// Each arm is its own script and its own process, so a failure names the arm and the arms cannot
/// leak state into one another through a shell.
pub(crate) fn run(workspace: &Workspace) -> Result<()> {
    // The transcript's own first line. Building here rather than assuming a build makes `cargo
    // xtask gate` the single command a scheduled job can run, which is what "reachable rather than
    // run once" asks for. `$CARGO`, with a typed argument list and no shell, as for every
    // command this binary starts.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
    println!("gate: cargo build --release --workspace --locked");
    let built = Command::new(cargo)
        .args(["build", "--release", "--workspace", "--locked"])
        .current_dir(workspace.root())
        .status()
        .context("building the workspace")?;
    if !built.success() {
        bail!("the release build failed ({built}); the arms have nothing to run");
    }

    let dir = workspace.root().join(EXAMPLE_DIR);
    let arms: [(&str, PathBuf, Vec<&str>); 3] = [
        (
            "arm 1 — the loop closes with no human in it",
            dir.join("run.sh"),
            vec![],
        ),
        (
            "arm 2 — persist before spawn survives a kill",
            dir.join("crash.sh"),
            vec![],
        ),
        (
            "the control — the same run without the crash",
            dir.join("crash.sh"),
            vec!["--no-crash"],
        ),
    ];

    let mut failures = 0;
    for (what, script, args) in arms {
        println!("\n=== {what} ===\n{} {}", script.display(), args.join(" "));
        let status = Command::new(&script)
            .args(&args)
            .current_dir(workspace.root())
            .status()
            .with_context(|| format!("running {}", script.display()))?;
        if status.success() {
            println!("PASS  {what}");
        } else {
            failures += 1;
            println!("FAIL  {what} ({status})");
        }
    }
    if failures > 0 {
        bail!("{failures} of 3 gate arms failed");
    }
    println!("\nPASS  the end-to-end gate: 3 of 3 arms");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two path constants above are copies of the daemon's, so they are read back out of the
    /// daemon's source and compared. A copy nobody checks is how a gate ends up reading an empty
    /// directory and reporting a clean journal.
    ///
    /// The control is the assertion that each declaration was *found*: without it a renamed
    /// constant would make both searches return nothing and the test would pass on two absences.
    #[test]
    fn the_state_layout_matches_the_daemons() {
        let workspace = crate::workspace::locate().expect("this checkout");
        let boot = workspace.root().join("bins/duded/src/boot.rs");
        let source = std::fs::read_to_string(&boot)
            .unwrap_or_else(|error| panic!("reading {}: {error}", boot.display()));

        for (name, value) in [
            ("JOURNAL_DIR", JOURNAL_DIR),
            ("PROJECTION_FILE", PROJECTION_FILE),
        ] {
            let declaration = format!("pub const {name}: &str = ");
            let line = source
                .lines()
                .find(|line| line.trim_start().starts_with(&declaration))
                .unwrap_or_else(|| panic!("`{declaration}` is gone from {}", boot.display()));
            assert!(
                line.contains(&format!("\"{value}\"")),
                "the daemon declares `{name}` as `{line}`, and xtask copies `{value}`"
            );
        }
    }
}
