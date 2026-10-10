//! `cargo xtask schemas [--check]` — export `dudes-proto` contract types to `schemas/*.json`.
//!
//! The documents come from [`dudes_proto::schema::schemas`]; this command only writes them
//! (pretty JSON, trailing newline) or, with `--check`, compares them byte-for-byte with what is
//! committed and fails on any difference, so CI proves the committed schemas match the code.
//! Files in `schemas/` that the generator no longer produces are reported as stale.
//!
//! A mismatch says which of three things happened, because they call for different actions and
//! have been confused for each other: the committed file *differs* from what the code generates,
//! the committed file could not be *read*, or this binary is reading a different checkout than the
//! caller thinks — the last is refused up front by [`crate::workspace::locate`], which is also
//! where the directory being read gets its one and only source of truth.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::workspace::Workspace;

const DIR: &str = "schemas";

pub(crate) fn run(workspace: &Workspace, check: bool) -> Result<()> {
    let dir = workspace.root().join(DIR);
    let generated: Vec<(String, String)> = dudes_proto::schema::schemas()
        .into_iter()
        .map(|doc| {
            let mut text =
                serde_json::to_string_pretty(&doc.schema).context("schema is not JSON")?;
            text.push('\n');
            Ok((doc.file_name.to_owned(), text))
        })
        .collect::<Result<_>>()?;

    let stale = stale_files(&dir, generated.iter().map(|(name, _)| name.as_str()))?;
    if check {
        check_all(&dir, &generated, &stale)
    } else {
        write_all(&dir, &generated, &stale)
    }
}

fn write_all(dir: &Path, generated: &[(String, String)], stale: &[String]) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, text) in generated {
        let path = dir.join(name);
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        println!("schemas: wrote {DIR}/{name}");
    }
    for name in stale {
        println!("schemas: {DIR}/{name} is not generated any more; remove it");
    }
    Ok(())
}

/// What `--check` found for one committed file.
///
/// A separate type rather than three `println!` arms because the labels are the command's answer,
/// not decoration: "the file differs" and "I could not read the file" call for different actions
/// and have been confused for each other. Rendering them through [`fmt::Display`] puts the wording
/// somewhere a test can assert it — a label no test reads is a label free to change silently.
#[derive(Debug)]
enum Outcome {
    /// The committed file is byte-for-byte what the code generates.
    Matches,
    /// The committed file was read and is not what the code generates. Both sizes are carried so
    /// that a file replaced wholesale is distinguishable at a glance from a field that moved:
    /// `{}` reads as 3 bytes against six figures of generated schema.
    Differs { committed: usize, generated: usize },
    /// The committed file could not be read at all — absent, or unreadable for an OS reason. Not
    /// drift: the code's output was never compared with anything.
    Unreadable(String),
}

impl Outcome {
    /// Whether this outcome should fail the check.
    fn is_difference(&self) -> bool {
        !matches!(self, Self::Matches)
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matches => f.write_str("ok"),
            Self::Differs {
                committed,
                generated,
            } => write!(
                f,
                "DIFFERS from the code (committed {committed} bytes, generated {generated} bytes)"
            ),
            Self::Unreadable(err) => write!(f, "UNREADABLE ({err})"),
        }
    }
}

/// Compare one committed file with the text the code generates for it.
fn compare(path: &Path, generated: &str) -> Outcome {
    match fs::read_to_string(path) {
        Ok(committed) if committed == generated => Outcome::Matches,
        Ok(committed) => Outcome::Differs {
            committed: committed.len(),
            generated: generated.len(),
        },
        Err(err) => Outcome::Unreadable(err.to_string()),
    }
}

fn check_all(dir: &Path, generated: &[(String, String)], stale: &[String]) -> Result<()> {
    let mut differences = Vec::new();
    for (name, text) in generated {
        let outcome = compare(&dir.join(name), text);
        println!("schemas: {DIR}/{name} {outcome}");
        if outcome.is_difference() {
            differences.push(name.clone());
        }
    }
    for name in stale {
        println!("schemas: {DIR}/{name} STALE (not generated any more)");
        differences.push(name.clone());
    }
    if differences.is_empty() {
        return Ok(());
    }
    bail!(
        "{} schema file(s) out of date in {}: {}; run `cargo xtask schemas` and commit",
        differences.len(),
        dir.display(),
        differences.join(", ")
    )
}

/// `*.schema.json` files in `dir` that the generator does not produce.
fn stale_files<'a>(dir: &Path, generated: impl Iterator<Item = &'a str>) -> Result<Vec<String>> {
    let expected: BTreeSet<&str> = generated.collect();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".schema.json") && !expected.contains(name.as_str()) {
            stale.push(name);
        }
    }
    stale.sort();
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One generated document, as [`run`] would hand it to [`check_all`].
    fn generated() -> Vec<(String, String)> {
        vec![(
            "thing.schema.json".to_owned(),
            "{\n  \"a\": 1\n}\n".to_owned(),
        )]
    }

    #[test]
    fn check_passes_when_the_committed_file_matches() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let generated = generated();
        fs::write(tmp.path().join(&generated[0].0), &generated[0].1).expect("write");
        check_all(tmp.path(), &generated, &[]).expect("a matching file is not drift");
    }

    #[test]
    fn check_fails_on_drift_and_names_the_directory_it_read() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let generated = generated();
        fs::write(tmp.path().join(&generated[0].0), "{}\n").expect("write");
        let err = check_all(tmp.path(), &generated, &[]).expect_err("drift must fail the check");
        let message = err.to_string();
        assert!(message.contains("thing.schema.json"), "{message}");
        assert!(
            message.contains(&tmp.path().display().to_string()),
            "the failure must name the directory actually read: {message}"
        );
    }

    #[test]
    fn an_unreadable_committed_file_fails_the_check_and_is_not_labelled_drift() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let generated = generated();
        let path = tmp.path().join(&generated[0].0);

        // Positive control: with the file present and wrong, this same comparison says DIFFERS. So
        // the two labels are being told apart below, not one of them simply being unreachable.
        fs::write(&path, "{}\n").expect("write");
        let drift = compare(&path, &generated[0].1).to_string();
        assert!(drift.starts_with("DIFFERS"), "{drift}");

        fs::remove_file(&path).expect("remove");
        let unreadable = compare(&path, &generated[0].1).to_string();
        assert!(unreadable.starts_with("UNREADABLE"), "{unreadable}");
        assert!(
            !unreadable.contains("DIFFERS"),
            "a file that could not be read has not been compared with anything, and the two call \
             for different actions: {unreadable}"
        );

        let err = check_all(tmp.path(), &generated, &[]).expect_err("a missing file is not ok");
        assert!(err.to_string().contains("thing.schema.json"), "{err}");
    }

    #[test]
    fn drift_reports_both_byte_counts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let generated = generated();
        let path = tmp.path().join(&generated[0].0);
        let committed = "{}\n";
        fs::write(&path, committed).expect("write");
        assert_ne!(
            committed.len(),
            generated[0].1.len(),
            "control: the two sizes must differ, or reporting one twice would pass"
        );
        let line = compare(&path, &generated[0].1).to_string();
        assert!(
            line.contains(&format!("committed {} bytes", committed.len())),
            "{line}"
        );
        assert!(
            line.contains(&format!("generated {} bytes", generated[0].1.len())),
            "{line}"
        );
    }

    #[test]
    fn check_fails_on_a_file_the_generator_no_longer_produces() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let generated = generated();
        fs::write(tmp.path().join(&generated[0].0), &generated[0].1).expect("write");
        let err = check_all(tmp.path(), &generated, &["gone.schema.json".to_owned()])
            .expect_err("a stale file must fail the check");
        assert!(err.to_string().contains("gone.schema.json"), "{err}");
    }

    #[test]
    fn stale_files_lists_only_unexpected_schema_documents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(tmp.path().join("kept.schema.json"), "{}").expect("write");
        fs::write(tmp.path().join("gone.schema.json"), "{}").expect("write");
        fs::write(tmp.path().join("README.md"), "not a schema").expect("write");
        let stale = stale_files(tmp.path(), ["kept.schema.json"].into_iter()).expect("read dir");
        assert_eq!(stale, vec!["gone.schema.json".to_owned()]);
    }
}
