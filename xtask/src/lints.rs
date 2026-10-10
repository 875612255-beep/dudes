//! `cargo xtask lints` — pin every workspace unsafe override and its source grant.
//!
//! An override crate may copy the workspace lint tables with exactly three
//! changes: `unsafe_code` becomes `deny`, `unsafe_op_in_unsafe_fn = "deny"` is added, and
//! `clippy::undocumented_unsafe_blocks` becomes `deny`. This gate checks those maps, registers
//! every overriding member, and rejects an `unsafe` keyword outside its registered source files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use anyhow::{Context, Result, bail};
use proc_macro2::{TokenStream, TokenTree};

use crate::metadata;
use crate::workspace::Workspace;

#[derive(Clone, Copy)]
struct Rule {
    member: &'static str,
    unsafe_files: &'static [&'static str],
}

const RULES: &[Rule] = &[
    Rule {
        member: "dudes-host",
        unsafe_files: &["src/windows.rs"],
    },
    Rule {
        member: "dudes-pty",
        unsafe_files: &["src/tree/unix.rs", "src/tree/windows.rs"],
    },
    Rule {
        member: "dudes-win",
        unsafe_files: &["src/windows.rs"],
    },
];

type Table = BTreeMap<String, String>;

pub(crate) fn run(workspace: &Workspace) -> Result<()> {
    let root_manifest = fs::read_to_string(workspace.root().join("Cargo.toml"))
        .context("read workspace Cargo.toml")?;
    let workspace_rust = table(&root_manifest, "workspace.lints.rust")?;
    let workspace_clippy = table(&root_manifest, "workspace.lints.clippy")?;
    if workspace_rust.is_empty() || workspace_clippy.is_empty() {
        bail!("workspace lint tables must both be explicit and non-empty");
    }
    let expected_rust = expected_rust(&workspace_rust);
    let expected_clippy = expected_clippy(&workspace_clippy);
    let members = metadata::members(&metadata::cargo_metadata()?)?;
    let mut seen = BTreeSet::new();

    for package in members {
        let manifest_path = workspace.root().join(&package.manifest);
        let manifest = fs::read_to_string(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        match classify(
            package.name.as_str(),
            &manifest,
            &expected_rust,
            &expected_clippy,
        )? {
            Classification::Inherited => {
                println!("lints: {:<18} inherits", package.name);
            }
            Classification::Override(rule) => {
                seen.insert(rule.member);
                let member_root = manifest_path
                    .parent()
                    .with_context(|| format!("{} has no parent", manifest_path.display()))?;
                scan_sources(member_root, rule)?;
                println!("lints: {:<18} registered override", rule.member);
            }
        }
    }

    let registered: BTreeSet<&str> = RULES.iter().map(|rule| rule.member).collect();
    if seen != registered {
        bail!(
            "unsafe override register does not match workspace members: missing {:?}, stale {:?}",
            registered.difference(&seen).collect::<Vec<_>>(),
            seen.difference(&registered).collect::<Vec<_>>()
        );
    }
    Ok(())
}

enum Classification {
    Inherited,
    Override(&'static Rule),
}

fn classify(
    member: &str,
    manifest: &str,
    expected_rust: &Table,
    expected_clippy: &Table,
) -> Result<Classification> {
    let base = table(manifest, "lints")?;
    let rust = table(manifest, "lints.rust")?;
    let clippy = table(manifest, "lints.clippy")?;
    match base.get("workspace").map(String::as_str) {
        Some("true") if base.len() == 1 && rust.is_empty() && clippy.is_empty() => {
            if RULES.iter().any(|rule| rule.member == member) {
                bail!("registered override `{member}` unexpectedly inherits workspace lints");
            }
            Ok(Classification::Inherited)
        }
        Some("true") => bail!(
            "`{member}` mixes workspace lint inheritance with local lint entries; choose one exact mode"
        ),
        Some(other) => bail!("`{member}` sets [lints] workspace to unsupported value `{other}`"),
        None if base.is_empty() && rust.is_empty() && clippy.is_empty() => {
            bail!("`{member}` omits explicit workspace lint inheritance")
        }
        None if !base.is_empty() => {
            bail!("`{member}` has a [lints] table without `workspace = true`")
        }
        None => {
            let rule = rule(member)?;
            compare(rule.member, "rust", &rust, expected_rust)?;
            compare(rule.member, "clippy", &clippy, expected_clippy)?;
            Ok(Classification::Override(rule))
        }
    }
}

fn rule(member: &str) -> Result<&'static Rule> {
    RULES
        .iter()
        .find(|rule| rule.member == member)
        .with_context(|| {
            format!("workspace member `{member}` overrides unsafe_code but has no lints row")
        })
}

fn expected_rust(workspace: &Table) -> Table {
    let mut expected = workspace.clone();
    expected.insert("unsafe_code".into(), "\"deny\"".into());
    expected.insert("unsafe_op_in_unsafe_fn".into(), "\"deny\"".into());
    expected
}

fn expected_clippy(workspace: &Table) -> Table {
    let mut expected = workspace.clone();
    expected.insert("undocumented_unsafe_blocks".into(), "\"deny\"".into());
    expected
}

fn compare(member: &str, table_name: &str, actual: &Table, expected: &Table) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        bail!(
            "`{member}` [lints.{table_name}] differs from the workspace table plus the permitted deltas\n  expected: {expected:?}\n  actual:   {actual:?}"
        )
    }
}

fn table(source: &str, wanted: &str) -> Result<Table> {
    let mut current = None;
    let mut values = Table::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = Some(&line[1..line.len() - 1]);
            continue;
        }
        if current != Some(wanted) || line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("line {} in [{wanted}] is not a key/value", index + 1))?;
        let key = key.trim().to_owned();
        let value = value.trim().to_owned();
        if values.insert(key.clone(), value).is_some() {
            bail!("[{wanted}] declares `{key}` twice");
        }
    }
    Ok(values)
}

fn scan_sources(member_root: &Path, rule: &Rule) -> Result<()> {
    let allowed: BTreeSet<&str> = rule.unsafe_files.iter().copied().collect();
    let mut unsafe_files = BTreeSet::new();
    for path in package_rust_files(member_root)? {
        let relative = path
            .strip_prefix(member_root)
            .expect("source path is below member root")
            .to_string_lossy()
            .replace('\\', "/");
        let source =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let lines =
            unsafe_lines(&source).with_context(|| format!("tokenize {}", path.display()))?;
        if lines.is_empty() {
            continue;
        }
        if !allowed.contains(relative.as_str()) {
            bail!(
                "`{}` contains unsafe at lines {:?}, outside `{}`'s registered files {:?}",
                relative,
                lines,
                rule.member,
                rule.unsafe_files
            );
        }
        unsafe_files.insert(relative);
    }
    let missing: Vec<&str> = allowed
        .iter()
        .copied()
        .filter(|path| !unsafe_files.contains(*path))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        bail!(
            "`{}` registers files with no unsafe keyword: {missing:?}",
            rule.member
        )
    }
}

fn package_rust_files(member_root: &Path) -> Result<Vec<PathBuf>> {
    rust_files(member_root)
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                if !matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("target" | ".git")
                ) {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn unsafe_lines(source: &str) -> Result<Vec<usize>> {
    let stream = TokenStream::from_str(source)
        .map_err(|error| anyhow::anyhow!("source is not a Rust token stream: {error}"))?;
    let mut lines = Vec::new();
    collect_unsafe(stream, &mut lines);
    lines.sort_unstable();
    lines.dedup();
    Ok(lines)
}

fn collect_unsafe(stream: TokenStream, lines: &mut Vec<usize>) {
    for token in stream {
        match token {
            TokenTree::Ident(ident) if ident == "unsafe" => lines.push(ident.span().start().line),
            TokenTree::Group(group) => collect_unsafe(group.stream(), lines),
            TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_discovery_rejects_omissions_and_every_unregistered_override() {
        let workspace_rust = table(
            "[workspace.lints.rust]\nunsafe_code = \"forbid\"\nmissing_docs = \"warn\"\n",
            "workspace.lints.rust",
        )
        .unwrap();
        let workspace_clippy = table(
            "[workspace.lints.clippy]\nundocumented_unsafe_blocks = \"warn\"\n",
            "workspace.lints.clippy",
        )
        .unwrap();
        let rust = expected_rust(&workspace_rust);
        let clippy = expected_clippy(&workspace_clippy);
        assert!(matches!(
            classify("ordinary", "[lints]\nworkspace = true\n", &rust, &clippy).unwrap(),
            Classification::Inherited
        ));
        assert!(matches!(
            classify(
                "dudes-pty",
                "[lints.rust]\nunsafe_code = \"deny\"\nunsafe_op_in_unsafe_fn = \"deny\"\nmissing_docs = \"warn\"\n\
                 [lints.clippy]\nundocumented_unsafe_blocks = \"deny\"\n",
                &rust,
                &clippy,
            )
            .unwrap(),
            Classification::Override(_)
        ));
        for manifest in [
            "",
            "[lints]\nworkspace = false\n",
            "[lints.rust]\nunsafe_code = \"allow\"\n",
            "[lints.rust]\nunsafe_code = \"warn\"\n",
            "[lints.rust]\nunsafe_code = { level = \"deny\" }\n",
            "[lints.rust]\nunsafe_code = \"deny\"\n",
        ] {
            assert!(
                classify("surprise-unsafe-crate", manifest, &rust, &clippy).is_err(),
                "unregistered or omitted lint mode passed: {manifest:?}"
            );
        }
    }

    #[test]
    fn a_lint_copy_difference_is_rejected() {
        let workspace = table(
            "[workspace.lints.rust]\nunsafe_code = \"forbid\"\nmissing_docs = \"warn\"\n",
            "workspace.lints.rust",
        )
        .unwrap();
        let expected = expected_rust(&workspace);
        assert!(compare("clean", "rust", &expected, &expected).is_ok());
        let mut dirty = expected.clone();
        dirty.insert("missing_docs".into(), "\"allow\"".into());
        assert!(compare("dirty", "rust", &dirty, &expected).is_err());
    }

    #[test]
    fn unsafe_outside_the_registered_file_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src/tree")).unwrap();
        fs::write(
            temp.path().join("src/tree/windows.rs"),
            "unsafe { core::hint::unreachable_unchecked() }",
        )
        .unwrap();
        let rule = Rule {
            member: "fixture",
            unsafe_files: &["src/tree/windows.rs"],
        };
        scan_sources(temp.path(), &rule).expect("positive control: registered file");
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(
            temp.path().join("tests/escape.rs"),
            "unsafe { core::hint::unreachable_unchecked() }",
        )
        .unwrap();
        assert!(scan_sources(temp.path(), &rule).is_err());
    }
}
