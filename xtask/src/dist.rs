//! `cargo xtask dist` — the release layout: the three binaries and the built Floor in one
//! directory, and that directory as an archive for `install.sh`.
//!
//! `dudes quickstart` starts the `duded` beside it, and `duded` finds `dudes-hook` and, when
//! `DUDES_FLOOR_ASSETS` is unset, the Floor bundle as a `floor` directory beside itself. So a
//! release is one directory holding all of them, and this command assembles it from a release
//! build and `floor/dist`. It builds the binaries but not the Floor, whose build needs the exact
//! Node toolchain `floor/README.md` names; a missing or stale `floor/dist` is refused by name.
//!
//! It then writes that directory as `dudes-<version>-<target>.tar.gz`, named by the host target
//! triple or by the `--target <triple>` it was given, and records the archive's SHA-256 in `SHA256SUMS` beside it. `install.sh` reads both,
//! so their names and the archive's single top-level `dudes-<version>-<target>/` directory are its
//! contract. A `--target` build must be one the machine can run, because the daemon it built checks
//! the Floor before and after the layout is assembled. No archive is written on Windows, whose installer does not exist yet.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail, ensure};
use sha2::{Digest as _, Sha256};

use crate::workspace::Workspace;

/// The executables a release carries, without the platform suffix.
const BINARIES: [&str; 3] = ["duded", "dudes", "dudes-hook"];

const INSTALL_SH: &str = include_str!("../../install.sh");

fn installer_targets() -> Result<Vec<&'static str>> {
    let list = INSTALL_SH
        .lines()
        .find_map(|line| line.strip_prefix("supported_targets='"))
        .and_then(|line| line.strip_suffix('\''))
        .context("install.sh has no supported target list")?;
    Ok(list.split_whitespace().collect())
}

/// The licence and notice files a release carries beside the binaries.
const NOTICES: [&str; 3] = ["LICENSE-APACHE", "LICENSE-MIT", "THIRD-PARTY.md"];

/// The Floor build's inventory of the files `duded` serves.
#[cfg(test)]
const MANIFEST: &str = "release-manifest.json";

/// How to build the Floor, for the refusals below.
const FLOOR_BUILD: &str = "cd floor && npm ci --ignore-scripts && npm run build";

/// Read `--target <triple>` (or `--target=<triple>`) out of `dist`'s arguments. The triple must be
/// one `install.sh` can pick, because no other archive would ever be installed.
pub(crate) fn target(args: &[String]) -> Result<Option<String>> {
    let mut found = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let value = if arg == "--target" {
            rest.next().cloned().unwrap_or_default()
        } else if let Some(value) = arg.strip_prefix("--target=") {
            value.to_owned()
        } else {
            bail!("unexpected argument `{arg}`; usage: dist [--target <triple>]");
        };
        ensure!(!value.is_empty(), "--target needs a triple");
        let supported = installer_targets()?;
        ensure!(
            supported.contains(&value.as_str()),
            "{value} is not a target install.sh installs; choose one of: {}",
            supported.join(", ")
        );
        found = Some(value);
    }
    Ok(found)
}

pub(crate) fn run(workspace: &Workspace, target_triple: Option<&str>) -> Result<()> {
    let root = workspace.root();
    let version = env!("CARGO_PKG_VERSION");
    let floor = root.join("floor/dist");

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    println!("dist: cargo build --release --locked -p duded -p dudes -p dudes-hook");
    let mut compile = Command::new(&cargo);
    compile
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "duded",
            "-p",
            "dudes",
            "-p",
            "dudes-hook",
        ])
        .current_dir(root);
    if let Some(triple) = target_triple {
        compile.args(["--target", triple]);
    }
    let built = compile.status().context("building the release binaries")?;
    ensure!(built.success(), "the release build failed ({built})");

    let target = crate::metadata::cargo_metadata()?["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .context("`cargo metadata` names no target directory")?;
    let dist = target.join("dist");
    let out = dist.join(format!("dudes-{version}"));
    let release = release_dir(&target, target_triple);
    let daemon = release.join(format!("duded{}", std::env::consts::EXE_SUFFIX));
    let admitted_files = floor_files(&daemon, &floor)?;
    assemble(&release, &floor, &admitted_files, root, &out)?;
    floor_files(
        &out.join(format!("duded{}", std::env::consts::EXE_SUFFIX)),
        &out.join("floor"),
    )?;
    println!("dist: {}", out.display());

    if cfg!(windows) {
        println!("dist: no archive on Windows, whose installer does not exist yet");
        return Ok(());
    }
    let triple = match target_triple {
        Some(triple) => triple.to_owned(),
        None => host_triple(&cargo)?,
    };
    let name = format!("dudes-{version}-{triple}");
    let archive = dist.join(format!("{name}.tar.gz"));
    write_archive(&out, &name, &archive)?;
    record_checksum(&dist.join("SHA256SUMS"), version, &archive)?;
    println!("dist: {}", archive.display());
    Ok(())
}

/// Where `cargo build --release` writes binaries: below the target directory itself for the host,
/// and one `<triple>/` deeper when `--target` names a triple.
fn release_dir(target_dir: &Path, triple: Option<&str>) -> PathBuf {
    match triple {
        Some(triple) => target_dir.join(triple).join("release"),
        None => target_dir.join("release"),
    }
}

/// The host target triple of `cargo`, which its release build compiled for.
fn host_triple(cargo: &OsStr) -> Result<String> {
    let output = Command::new(cargo)
        .arg("-vV")
        .output()
        .context("running `cargo -vV`")?;
    ensure!(
        output.status.success(),
        "`cargo -vV` failed ({})",
        output.status
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .context("`cargo -vV` names no host")
}

fn floor_files(daemon: &Path, floor: &Path) -> Result<Vec<String>> {
    let admitted = Command::new(daemon)
        .arg("--check-floor")
        .arg(floor)
        .output()
        .with_context(|| format!("running {} Floor admission", daemon.display()))?;
    ensure!(
        admitted.status.success(),
        "{} was refused by the daemon's Floor admission: {}; rebuild it ({FLOOR_BUILD})",
        floor.display(),
        String::from_utf8_lossy(&admitted.stderr).trim()
    );
    serde_json::from_slice(&admitted.stdout)
        .context("reading the daemon's admitted Floor inventory")
}

/// Write `out` afresh: the binaries from `release`, `floor_files` from `floor` under `floor/`, and
/// the notices from `root`.
fn assemble(
    release: &Path,
    floor: &Path,
    floor_files: &[String],
    root: &Path,
    out: &Path,
) -> Result<()> {
    if out.exists() {
        fs::remove_dir_all(out).with_context(|| format!("removing {}", out.display()))?;
    }
    for binary in BINARIES {
        let name = format!("{binary}{}", std::env::consts::EXE_SUFFIX);
        copy(&release.join(&name), &out.join(&name))?;
    }
    for notice in NOTICES {
        copy(&root.join(notice), &out.join(notice))?;
    }
    for file in floor_files {
        copy(&floor.join(file), &out.join("floor").join(file))?;
    }
    Ok(())
}

/// Copy one file, creating its directory.
fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::copy(from, to)
        .map(drop)
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))
}

/// Write `dir` to `to` as a gzip-compressed tar with every entry under `top/`.
///
/// The same files give the same bytes: entries in byte order of their paths, modification time
/// zero, owner and group zero with no names, and each mode set by kind, not taken from the build
/// host. `gzip -n` leaves the name and time out of the gzip header.
fn write_archive(dir: &Path, top: &str, to: &Path) -> Result<()> {
    let file = fs::File::create(to).with_context(|| format!("creating {}", to.display()))?;
    let mut gzip = Command::new("gzip")
        .args(["-9", "-n"])
        .stdin(Stdio::piped())
        .stdout(file)
        .spawn()
        .context("starting gzip, which writes the release archive")?;
    let mut tar = gzip.stdin.take().context("gzip's standard input")?;
    let written = write_tar(dir, top, &mut tar);
    drop(tar);
    let compressed = gzip.wait().context("waiting for gzip")?;
    written?;
    ensure!(compressed.success(), "gzip failed ({compressed})");
    Ok(())
}

/// The ustar stream `write_archive` compresses.
fn write_tar(dir: &Path, top: &str, out: &mut impl Write) -> Result<()> {
    tar_header(out, &format!("{top}/"), 0o755, 0, b'5')?;
    for path in tree(dir)? {
        let name = format!("{top}/{path}");
        if path.ends_with('/') {
            tar_header(out, &name, 0o755, 0, b'5')?;
            continue;
        }
        let source = dir.join(&path);
        let bytes = fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
        let executable = BINARIES
            .iter()
            .any(|binary| path == format!("{binary}{}", std::env::consts::EXE_SUFFIX));
        let mode = if executable { 0o755 } else { 0o644 };
        tar_header(out, &name, mode, bytes.len() as u64, b'0')?;
        out.write_all(&bytes)?;
        out.write_all(&[0; 512][..(512 - bytes.len() % 512) % 512])?;
    }
    out.write_all(&[0; 1024])?;
    Ok(())
}

/// One ustar header block. `kind` is `b'0'` for a file and `b'5'` for a directory.
fn tar_header(out: &mut impl Write, path: &str, mode: u64, size: u64, kind: u8) -> Result<()> {
    let (prefix, name) = ustar_name(path)?;
    let mut block = [0u8; 512];
    block[..name.len()].copy_from_slice(name.as_bytes());
    block[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    for (field, value) in [
        (100..108, mode),
        (108..116, 0),
        (116..124, 0),
        (124..136, size),
        (136..148, 0),
    ] {
        octal(&mut block[field], value);
    }
    block[148..156].fill(b' ');
    block[156] = kind;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    let checksum = block.iter().map(|&byte| u64::from(byte)).sum();
    octal(&mut block[148..155], checksum);
    out.write_all(&block).map_err(Into::into)
}

/// `path` as a ustar header's prefix and name: the whole path as the name when it fits in 100
/// bytes, else split at a `/` into a prefix of at most 155 bytes and a name of at most 100.
fn ustar_name(path: &str) -> Result<(&str, &str)> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    path.match_indices('/')
        .map(|(at, _)| (&path[..at], &path[at + 1..]))
        .find(|(prefix, name)| prefix.len() <= 155 && !name.is_empty() && name.len() <= 100)
        .with_context(|| format!("{path} is too long for a tar header"))
}

/// `value` as zero-padded octal digits, leaving the field's last byte NUL.
fn octal(field: &mut [u8], value: u64) {
    let digits = format!("{value:0width$o}", width = field.len() - 1);
    field[..digits.len()].copy_from_slice(digits.as_bytes());
}

/// Every directory and file below `dir`, as `/`-separated paths relative to it in byte order, a
/// directory's with a trailing `/` as tar names it.
fn tree(dir: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        let entries =
            fs::read_dir(&directory).with_context(|| format!("reading {}", directory.display()))?;
        for entry in entries {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| anyhow!("{} is not UTF-8", name.display()))?;
            let mut path = format!("{relative}{name}");
            if entry.file_type()?.is_dir() {
                path.push('/');
                pending.push((entry.path(), path.clone()));
            }
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// Record `archive`'s SHA-256 in `sums` as a `<hex>  <file name>` line.
///
/// The file lists the archives of one release, one line each in name order: this archive's earlier
/// line is replaced, other targets of `version` are kept, and other versions are dropped, so
/// `install.sh` finds one latest release per target.
fn record_checksum(sums: &Path, version: &str, archive: &Path) -> Result<()> {
    let targets = installer_targets()?;
    let name = archive
        .file_name()
        .and_then(OsStr::to_str)
        .context("the archive has a UTF-8 file name")?;
    let bytes = fs::read(archive).with_context(|| format!("reading {}", archive.display()))?;
    let digest = Sha256::digest(&bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    let existing = match fs::read_to_string(sums) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", sums.display()));
        }
    };
    let release = format!("dudes-{version}-");
    let mut lines: BTreeMap<&str, &str> = existing
        .lines()
        .filter_map(|line| line.split_once("  "))
        .map(|(hash, file)| (file, hash))
        .filter(|(file, _)| {
            file.strip_prefix(&release)
                .and_then(|target| target.strip_suffix(".tar.gz"))
                .is_some_and(|target| targets.contains(&target))
        })
        .collect();
    lines.insert(name, &digest);
    let text = lines.iter().fold(String::new(), |mut text, (file, hash)| {
        let _ = writeln!(text, "{hash}  {file}");
        text
    });
    fs::write(sums, text).with_context(|| format!("writing {}", sums.display()))
}

#[cfg(all(test, unix))]
mod install_sh;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        fs::write(path, text).expect("a file");
    }

    #[test]
    fn target_accepts_a_supported_triple_in_either_spelling_and_refuses_the_rest() {
        let args = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(target(&args(&[])).expect("no target"), None);
        assert_eq!(
            target(&args(&["--target", "x86_64-apple-darwin"])).expect("a target"),
            Some("x86_64-apple-darwin".to_owned())
        );
        assert_eq!(
            target(&args(&["--target=aarch64-unknown-linux-musl"])).expect("a target"),
            Some("aarch64-unknown-linux-musl".to_owned())
        );
        for refused in [
            args(&["--target"]),
            args(&["--target="]),
            args(&["--target", "x86_64-pc-windows-msvc"]),
            args(&["x86_64-apple-darwin"]),
            args(&["--release"]),
        ] {
            assert!(target(&refused).is_err(), "{refused:?} is refused");
        }
    }

    #[test]
    fn a_target_build_is_looked_for_one_directory_deeper() {
        let dir = Path::new("/t");
        assert_eq!(release_dir(dir, None), Path::new("/t/release"));
        assert_eq!(
            release_dir(dir, Some("aarch64-apple-darwin")),
            Path::new("/t/aarch64-apple-darwin/release")
        );
    }

    #[test]
    fn the_layout_puts_the_admitted_floor_beside_the_binaries() {
        let root = tempfile::tempdir().expect("a workspace");
        let release = root.path().join("target/release");
        for binary in BINARIES {
            write(
                &release.join(format!("{binary}{}", std::env::consts::EXE_SUFFIX)),
                binary,
            );
        }
        for notice in NOTICES {
            write(&root.path().join(notice), notice);
        }
        let floor = root.path().join("floor/dist");
        write(
            &floor.join("release-manifest.json"),
            r#"{"daemon_version":"1.2.3","files":[{"path":"assets/app-01234567.js"},{"path":"index.html"}]}"#,
        );
        write(&floor.join("index.html"), "<!doctype html>");
        write(&floor.join("assets/app-01234567.js"), "app");
        write(&floor.join(".vite/manifest.json"), "a build byproduct");
        let out = root.path().join("target/dist/dudes-1.2.3");
        write(&out.join("stale"), "from an earlier run");

        let admitted = vec![
            MANIFEST.to_owned(),
            "index.html".to_owned(),
            "assets/app-01234567.js".to_owned(),
        ];
        assemble(&release, &floor, &admitted, root.path(), &out).expect("assembled");

        let exe = std::env::consts::EXE_SUFFIX;
        let mut expected = vec![
            "LICENSE-APACHE".to_owned(),
            "LICENSE-MIT".to_owned(),
            "THIRD-PARTY.md".to_owned(),
            format!("duded{exe}"),
            format!("dudes-hook{exe}"),
            format!("dudes{exe}"),
            "floor/".to_owned(),
            "floor/assets/".to_owned(),
            "floor/assets/app-01234567.js".to_owned(),
            "floor/index.html".to_owned(),
            "floor/release-manifest.json".to_owned(),
        ];
        expected.sort();
        assert_eq!(tree(&out).expect("the layout"), expected);
        assert_eq!(
            fs::read_to_string(out.join("floor/assets/app-01234567.js")).expect("the asset"),
            "app"
        );
    }

    /// A long Floor asset name, as the build writes for its licence inventory, whose archive path
    /// needs a ustar prefix.
    #[cfg(unix)]
    const LONG_ASSET: &str = "third-party-licenses-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json";

    /// A small release directory: the binaries, one notice and a Floor with nested assets.
    #[cfg(unix)]
    fn release_tree(dir: &Path) {
        for binary in BINARIES {
            write(&dir.join(binary), &format!("#!/bin/sh\necho {binary}\n"));
        }
        write(&dir.join("LICENSE-MIT"), "MIT");
        write(&dir.join("floor/index.html"), "<!doctype html>");
        write(&dir.join("floor/assets/app-01234567.js"), "app");
        write(&dir.join("floor/assets").join(LONG_ASSET), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn the_archive_holds_the_release_under_one_directory_in_byte_order() {
        let root = tempfile::tempdir().expect("a workspace");
        release_tree(&root.path().join("release"));
        let archive = root
            .path()
            .join("dudes-1.2.3-x86_64-unknown-linux-gnu.tar.gz");

        write_archive(
            &root.path().join("release"),
            "dudes-1.2.3-x86_64-unknown-linux-gnu",
            &archive,
        )
        .expect("an archive");

        let listed = Command::new("tar")
            .arg("-tzf")
            .arg(&archive)
            .output()
            .expect("tar lists the archive");
        assert!(listed.status.success(), "tar reads the archive: {listed:?}");
        assert_eq!(
            String::from_utf8(listed.stdout).expect("UTF-8 names"),
            "dudes-1.2.3-x86_64-unknown-linux-gnu/
dudes-1.2.3-x86_64-unknown-linux-gnu/LICENSE-MIT
dudes-1.2.3-x86_64-unknown-linux-gnu/duded
dudes-1.2.3-x86_64-unknown-linux-gnu/dudes
dudes-1.2.3-x86_64-unknown-linux-gnu/dudes-hook
dudes-1.2.3-x86_64-unknown-linux-gnu/floor/
dudes-1.2.3-x86_64-unknown-linux-gnu/floor/assets/
dudes-1.2.3-x86_64-unknown-linux-gnu/floor/assets/app-01234567.js
dudes-1.2.3-x86_64-unknown-linux-gnu/floor/assets/third-party-licenses-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json
dudes-1.2.3-x86_64-unknown-linux-gnu/floor/index.html
"
        );

        let unpacked = root.path().join("unpacked");
        fs::create_dir(&unpacked).expect("a directory to unpack into");
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&unpacked)
            .status()
            .expect("tar unpacks the archive");
        assert!(status.success());
        let top = unpacked.join("dudes-1.2.3-x86_64-unknown-linux-gnu");
        let daemon = Command::new(top.join("duded"))
            .output()
            .expect("the unpacked daemon runs");
        assert_eq!(daemon.stdout, b"duded\n");
        assert_eq!(
            fs::read_to_string(top.join("floor/assets/app-01234567.js")).expect("the asset"),
            "app"
        );
        assert_eq!(
            fs::read_to_string(top.join("floor/assets").join(LONG_ASSET)).expect("the long asset"),
            "{}"
        );
        let modified = fs::metadata(top.join("LICENSE-MIT"))
            .and_then(|metadata| metadata.modified())
            .expect("a modification time");
        assert_eq!(modified, std::time::SystemTime::UNIX_EPOCH);
    }

    /// Two trees holding the same files, written at different times with different modes, give
    /// byte-identical archives with no owner in them.
    #[cfg(unix)]
    #[test]
    fn the_archive_depends_only_on_the_files() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("a workspace");
        let (first, second) = (root.path().join("first"), root.path().join("second"));
        release_tree(&first);
        release_tree(&second);
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(second.join("LICENSE-MIT"))
            .and_then(|file| file.set_modified(hour_ago))
            .expect("an older modification time");
        fs::set_permissions(second.join("duded"), fs::Permissions::from_mode(0o700))
            .expect("other permissions");

        let archives = [&first, &second].map(|tree| {
            let archive = tree.with_extension("tar.gz");
            write_archive(tree, "dudes-1.2.3-aarch64-apple-darwin", &archive).expect("an archive");
            fs::read(archive).expect("the archive's bytes")
        });
        assert_eq!(archives[0], archives[1]);

        let tar = Command::new("gzip")
            .args(["-d", "-c"])
            .arg(first.with_extension("tar.gz"))
            .output()
            .expect("gzip decompresses the archive")
            .stdout;
        assert_eq!(&tar[108..116], b"0000000\0", "uid is zero");
        assert_eq!(&tar[116..124], b"0000000\0", "gid is zero");
        assert_eq!(&tar[265..329], &[0; 64], "no owner or group names");
    }

    #[test]
    fn sha256sums_lists_each_archive_of_this_release_once() {
        let root = tempfile::tempdir().expect("a dist directory");
        let sums = root.path().join("SHA256SUMS");
        write(
            &sums,
            &format!(
                "{a}  dudes-1.2.2-aarch64-apple-darwin.tar.gz\n\
                 {b}  dudes-1.2.3-x86_64-unknown-linux-gnu.tar.gz\n\
                 {b}  dudes-1.2.3-rc.1-aarch64-apple-darwin.tar.gz\n\
                 {c}  dudes-1.2.3-aarch64-apple-darwin.tar.gz\n",
                a = "a".repeat(64),
                b = "b".repeat(64),
                c = "c".repeat(64),
            ),
        );
        let archive = root.path().join("dudes-1.2.3-aarch64-apple-darwin.tar.gz");
        write(&archive, "archive");

        record_checksum(&sums, "1.2.3", &archive).expect("recorded");

        assert_eq!(
            fs::read_to_string(&sums).expect("the sums"),
            format!(
                "0eb3e36bfb24dcd9bb1d1bece1531216b59539a8fde17ee80224af0653c92aa3  \
                 dudes-1.2.3-aarch64-apple-darwin.tar.gz\n\
                 {b}  dudes-1.2.3-x86_64-unknown-linux-gnu.tar.gz\n",
                b = "b".repeat(64),
            )
        );
    }
}
