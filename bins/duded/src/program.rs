//! Where the daemon finds a program it launches. Invariant 7 puts the `PATH` lookup in the daemon,
//! and [`on_path`] is that lookup: the managed launch, the attached launch and the command
//! predicate all go through it, so every answer to "which file would run" comes from one search.
//!
//! # A child must never search its own `PATH`
//!
//! A launch sets the child's working directory to the agent's workspace. Given a bare name, the
//! standard library then searches `PATH` in the child, after it has changed into that directory,
//! so a relative or empty `PATH` entry names a file inside the workspace, which an agent or an
//! untrusted repository can write. [`resolve_launch`] therefore hands the host only an absolute
//! program, and [`on_path`] skips every `PATH` entry that is not absolute.
//!
//! # Windows
//!
//! Given a bare name, Rust's standard library on Windows searches the child's `PATH`, then the
//! directory of the daemon's own executable, the system directory, the Windows directory and the
//! daemon's `PATH`, and appends `.exe` to a name with no extension
//! (`library/std/src/sys/process/windows.rs`, `resolve_exe` and `search_paths`, Rust 1.98). Given
//! an absolute path it searches nothing. [`on_path`] keeps the `.exe` rule and none of the other
//! locations, so a program that is only in one of them is refused rather than found.
//!
//! # Runnable
//!
//! A candidate matches when it is a regular file, after following symlinks, that the daemon may
//! execute: the kernel's own `access(2)` test with `X_OK` on Unix, which refuses a file a mode, an
//! ACL or a `noexec` mount denies, where `execvp` would get `EACCES` and try the next entry. The
//! Floor's provider check reports [`resolve`]'s answer, so what it shows is the file
//! this search would launch.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use dudes_providers::launch::{EnvVar, LaunchSpec};

/// The first absolute entry of `path` holding a runnable file named `name`, joined to it.
///
/// A relative or empty entry is skipped rather than joined to a working directory: whose directory
/// that would be is the question the daemon must not leave open. On Windows a `name` without a
/// `.` is looked up with `.exe` appended, as the standard library does.
#[must_use]
pub fn on_path(name: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .filter(|entry| entry.is_absolute())
        .map(|entry| entry.join(file_name(name)))
        .find(|candidate| runnable(candidate))
}

/// The absolute file a launch of `program` runs with `path` as its child's `PATH`. An absolute
/// `program` is checked as written. A bare name is found with [`on_path`].
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a relative `program` with a separator, which the child would
/// resolve inside its working directory, and [`io::ErrorKind::NotFound`] when the program is not a
/// runnable file or no absolute entry of `path` holds one.
pub fn resolve(program: &OsStr, path: Option<&OsStr>) -> io::Result<PathBuf> {
    locate(program, path).map_err(io::Error::from)
}

/// [`resolve`]'s answer before a refusal becomes the launch's error, so the Floor can word the
/// same refusal with its own path display.
pub(crate) fn locate(program: &OsStr, path: Option<&OsStr>) -> Result<PathBuf, Refusal> {
    let written = Path::new(program);
    if written.is_absolute() {
        return if runnable(written) {
            Ok(written.to_path_buf())
        } else {
            Err(Refusal::missing(written, path))
        };
    }
    if written.file_name() != Some(program) {
        return Err(Refusal::Relative(written.to_path_buf()));
    }
    on_path(program, path).ok_or_else(|| Refusal::missing(written, path))
}

/// Why [`locate`] found no program to launch.
#[derive(Clone, Debug)]
pub(crate) enum Refusal {
    /// A relative program with a separator, which the child would resolve inside its workspace.
    Relative(PathBuf),
    /// No runnable file as written or on `PATH`, and the Windows file found in its place, if any.
    Missing {
        written: PathBuf,
        hint: Option<RefusalHint>,
    },
}

impl Refusal {
    fn missing(written: &Path, path: Option<&OsStr>) -> Self {
        Self::Missing {
            written: written.to_path_buf(),
            hint: windows_hint(written, path),
        }
    }

    /// The Floor's form of a Windows refusal, each path as `show` displays it. `None` for any
    /// other refusal, and when `show` cannot display a path the message needs.
    pub(crate) fn detail(&self, show: impl Fn(&Path) -> Option<String>) -> Option<String> {
        match self {
            Self::Missing {
                written,
                hint: Some(hint),
            } => hint.message(written, show),
            _ => None,
        }
    }
}

/// The launch's form: every path in full, since this error never reaches the browser.
impl From<Refusal> for io::Error {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Relative(written) => Self::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the launch names `{}` by a relative path, which would resolve inside the agent's \
                     workspace",
                    written.display()
                ),
            ),
            Refusal::Missing { written, hint } => {
                let message = hint
                    .and_then(|hint| {
                        hint.message(&written, |path| Some(path.display().to_string()))
                    })
                    .unwrap_or_else(|| {
                        if written.is_absolute() {
                            format!("`{}` is not a file the daemon may run", written.display())
                        } else {
                            format!(
                                "`{}` is on no absolute entry of the launch's PATH",
                                written.display()
                            )
                        }
                    });
                Self::new(io::ErrorKind::NotFound, message)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum RefusalHint {
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    WindowsShim(PathBuf),
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    WindowsExecutable(PathBuf),
}

impl RefusalHint {
    /// The one message both forms share. It needs every path it names, so a path `show` cannot
    /// display leaves no message.
    fn message(&self, written: &Path, show: impl Fn(&Path) -> Option<String>) -> Option<String> {
        let written = show(written)?;
        Some(match self {
            Self::WindowsShim(shim) => format!(
                "`{written}` was not found as a runnable program; found Windows command shim `{}`, \
                 which the daemon does not launch. Set the provider's `command` to the absolute path \
                 of the real `.exe`. If the adapter supports script arguments, set `command` to the \
                 absolute path of `node.exe` and pass the script separately.",
                show(shim)?
            ),
            Self::WindowsExecutable(exe) => {
                let exe = show(exe)?;
                let abbreviation = if exe.starts_with('~') {
                    " `~` is only a display abbreviation and is not expanded in configuration."
                } else {
                    ""
                };
                format!(
                    "`{written}` is not a file the daemon may run; `{exe}` exists. Set the provider's \
                     `command` to the full absolute path of `{exe}`.{abbreviation}"
                )
            }
        })
    }
}

#[cfg(windows)]
fn windows_hint(written: &Path, path: Option<&OsStr>) -> Option<RefusalHint> {
    if written.is_absolute() {
        if is_windows_shim(written) && written.is_file() {
            return Some(RefusalHint::WindowsShim(written.to_path_buf()));
        }
        if !is_windows_exe(written) {
            let mut exe = written.as_os_str().to_os_string();
            exe.push(".exe");
            let exe = PathBuf::from(exe);
            if runnable(&exe) {
                return Some(RefusalHint::WindowsExecutable(exe));
            }
        }
        return None;
    }
    let shim = is_windows_shim(written);
    if !shim && is_windows_exe(written) {
        return None;
    }
    for entry in std::env::split_paths(path?).filter(|entry| entry.is_absolute()) {
        if shim {
            let candidate = entry.join(written);
            if candidate.is_file() {
                return Some(RefusalHint::WindowsShim(candidate));
            }
        } else {
            for extension in [".cmd", ".bat"] {
                let mut name = written.as_os_str().to_os_string();
                name.push(extension);
                let candidate = entry.join(name);
                if candidate.is_file() {
                    return Some(RefusalHint::WindowsShim(candidate));
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn windows_hint(_written: &Path, _path: Option<&OsStr>) -> Option<RefusalHint> {
    None
}

/// Whether the standard library would run `path` through `cmd.exe`. It tests for a `.bat` or
/// `.cmd` suffix, in any case, on the name Win32 resolves, which has lost its trailing dots and
/// spaces. So `claude.cmd.` and `claude.cmd ` are shims too.
#[cfg(any(windows, test))]
fn is_windows_shim(path: &Path) -> bool {
    let mut name = path.as_os_str().as_encoded_bytes();
    while let [rest @ .., b'.' | b' '] = name {
        name = rest;
    }
    name.len()
        .checked_sub(4)
        .and_then(|start| name.get(start..))
        .is_some_and(|suffix| {
            suffix.eq_ignore_ascii_case(b".bat") || suffix.eq_ignore_ascii_case(b".cmd")
        })
}

#[cfg(windows)]
fn is_windows_exe(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

/// `spec` with `argv[0]` as the absolute program [`resolve`] finds on the `PATH` the launch gives
/// its child.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for an empty `argv`, and whatever [`resolve`] refuses.
pub fn resolve_launch(mut spec: LaunchSpec) -> io::Result<LaunchSpec> {
    let program = spec.argv.first().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the launch names no program")
    })?;
    spec.argv[0] = resolve(program, child_path(&spec.env))?.into_os_string();
    Ok(spec)
}

#[cfg(unix)]
fn runnable(candidate: &Path) -> bool {
    candidate.is_file() && rustix::fs::access(candidate, rustix::fs::Access::EXEC_OK).is_ok()
}

#[cfg(windows)]
fn runnable(candidate: &Path) -> bool {
    candidate.is_file() && !is_windows_shim(candidate)
}

#[cfg(not(any(unix, windows)))]
fn runnable(candidate: &Path) -> bool {
    candidate.is_file()
}

/// The `PATH` a launch with environment `env` gives its child.
pub(crate) fn child_path(env: &[EnvVar]) -> Option<&OsStr> {
    env_value(env, "PATH")
}

/// The value of `name` in `env`. Windows matches environment names without case.
pub(crate) fn env_value<'a>(env: &'a [EnvVar], name: &str) -> Option<&'a OsStr> {
    env.iter()
        .find(|var| {
            if cfg!(windows) {
                var.name().eq_ignore_ascii_case(name)
            } else {
                var.name() == name
            }
        })
        .map(EnvVar::expose_value)
}

#[cfg(windows)]
fn file_name(name: &OsStr) -> std::ffi::OsString {
    let mut file = name.to_os_string();
    if !name.as_encoded_bytes().contains(&b'.') {
        file.push(".exe");
    }
    file
}

#[cfg(not(windows))]
fn file_name(name: &OsStr) -> &OsStr {
    name
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn launch_error(written: &str, hint: RefusalHint) -> String {
        io::Error::from(Refusal::Missing {
            written: PathBuf::from(written),
            hint: Some(hint),
        })
        .to_string()
    }

    #[test]
    fn refusal_message_names_a_windows_shim_and_a_safe_command() {
        assert_eq!(
            launch_error(
                "claude",
                RefusalHint::WindowsShim(PathBuf::from("/tools/claude.cmd"))
            ),
            "`claude` was not found as a runnable program; found Windows command shim \
             `/tools/claude.cmd`, which the daemon does not launch. Set the provider's `command` \
             to the absolute path of the real `.exe`. If the adapter supports script arguments, \
             set `command` to the absolute path of `node.exe` and pass the script separately."
        );
    }

    #[test]
    fn redacted_windows_advice_requires_the_full_absolute_path() {
        let hint = RefusalHint::WindowsExecutable(PathBuf::from(
            "/home/operator/tools with spaces/codex.exe",
        ));
        let message = hint
            .message(
                Path::new("/home/operator/tools with spaces/codex"),
                |path| Some(path.display().to_string().replace("/home/operator", "~")),
            )
            .unwrap();
        assert_eq!(
            message,
            "`~/tools with spaces/codex` is not a file the daemon may run; \
             `~/tools with spaces/codex.exe` exists. Set the provider's `command` to the full \
             absolute path of `~/tools with spaces/codex.exe`. `~` is only a display abbreviation \
             and is not expanded in configuration."
        );
    }

    #[test]
    fn refusal_message_names_an_existing_windows_executable() {
        assert_eq!(
            launch_error(
                "/tools/codex",
                RefusalHint::WindowsExecutable(PathBuf::from("/tools/codex.exe"))
            ),
            "`/tools/codex` is not a file the daemon may run; `/tools/codex.exe` exists. Set the \
             provider's `command` to the full absolute path of `/tools/codex.exe`."
        );
    }

    #[test]
    fn a_windows_shim_is_named_as_win32_resolves_it() {
        for shim in [
            "/tools/claude.cmd",
            "/tools/CLAUDE.Bat",
            "/tools/claude.cmd.",
            "/tools/claude.cmd ",
            "/tools/claude.bat. .",
            "/tools/.cmd",
        ] {
            assert!(is_windows_shim(Path::new(shim)), "{shim:?} runs in cmd.exe");
        }
        for program in [
            "/tools/claude",
            "/tools/claude.exe",
            "/tools/claude.cmd.exe",
            "/tools/claude.cmdx",
            "/tools/cmd",
        ] {
            assert!(
                !is_windows_shim(Path::new(program)),
                "{program:?} is no shim"
            );
        }
    }

    fn install(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("the directory");
        let program = dir.join(file_name(OsStr::new(name)));
        std::fs::write(&program, "").expect("the program");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                .expect("make it executable");
        }
        program
    }

    fn launch(program: &str, path: &OsStr) -> LaunchSpec {
        LaunchSpec {
            argv: vec![OsString::from(program), OsString::from("--flag")],
            env: vec![EnvVar::new("PATH", path)],
            cwd: std::env::temp_dir(),
            stdin: dudes_providers::launch::LaunchInput::Null,
        }
    }

    fn joined(entries: &[&Path]) -> OsString {
        std::env::join_paths(entries).expect("joinable entries")
    }

    #[test]
    fn a_bare_name_becomes_the_first_absolute_entry_holding_it() {
        let dir = tempfile::tempdir().expect("a directory");
        let first = install(&dir.path().join("first"), "claude");
        install(&dir.path().join("second"), "claude");
        let path = joined(&[
            Path::new("relative"),
            &dir.path().join("empty"),
            &dir.path().join("first"),
            &dir.path().join("second"),
        ]);
        let spec = resolve_launch(launch("claude", &path)).expect("found");
        assert_eq!(
            spec.argv,
            [first.into_os_string(), OsString::from("--flag")]
        );
    }

    /// The same directory written relative to the test's working directory, by climbing to the
    /// root with `..` and descending again.
    #[cfg(unix)]
    fn relative_from_here(dir: &Path) -> PathBuf {
        let here = std::env::current_dir().expect("the test's directory");
        let mut relative: PathBuf = here.components().skip(1).map(|_| "..").collect();
        relative.push(dir.strip_prefix("/").expect("an absolute directory"));
        relative
    }

    #[cfg(unix)]
    #[test]
    fn relative_and_empty_entries_are_never_searched() {
        let dir = tempfile::tempdir().expect("a directory");
        install(dir.path(), "claude");
        let relative = relative_from_here(dir.path());
        assert!(relative.is_relative() && relative.join("claude").is_file());
        let path = OsString::from(format!("{}:", relative.display()));
        assert_eq!(on_path(OsStr::new("claude"), Some(&path)), None);
        let error = resolve_launch(launch("claude", &path)).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);

        let absolute = joined(&[dir.path()]);
        assert!(
            on_path(OsStr::new("claude"), Some(&absolute)).is_some(),
            "control: the same directory written absolutely is searched"
        );
    }

    #[test]
    fn an_absolute_program_is_kept_and_a_relative_one_is_refused() {
        let dir = tempfile::tempdir().expect("a directory");
        let absolute = install(dir.path(), "anything");
        let kept = resolve_launch(launch(absolute.to_str().expect("utf-8"), OsStr::new("")))
            .expect("kept");
        assert_eq!(kept.argv[0], absolute.as_os_str());
        std::fs::remove_file(&absolute).expect("remove it");
        let error = resolve_launch(launch(absolute.to_str().expect("utf-8"), OsStr::new("")))
            .expect_err("an absolute program that is gone is refused");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        for relative in ["./claude", "bin/claude", "..", ""] {
            let error = resolve_launch(launch(relative, OsStr::new("/usr/bin")))
                .expect_err("a relative program is refused");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{relative:?}");
        }
    }

    #[test]
    fn a_launch_without_a_path_finds_nothing() {
        let mut spec = launch("claude", OsStr::new(""));
        spec.env.clear();
        let error = resolve_launch(spec).expect_err("no PATH, no program");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(windows)]
    #[test]
    fn a_name_without_an_extension_is_found_as_an_exe() {
        let dir = tempfile::tempdir().expect("a directory");
        let exe = install(dir.path(), "claude");
        assert_eq!(exe.extension(), Some(OsStr::new("exe")));
        let path = joined(&[dir.path()]);
        assert_eq!(on_path(OsStr::new("claude"), Some(&path)), Some(exe));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shims_are_refused_with_their_path_but_a_later_exe_is_found() {
        let dir = tempfile::tempdir().expect("a directory");
        let shims = dir.path().join("shims");
        std::fs::create_dir(&shims).expect("the shim directory");
        for extension in ["cmd", "bat"] {
            let shim = shims.join(format!("claude.{extension}"));
            std::fs::write(&shim, "").expect("the shim");
            let path = joined(&[&shims]);
            let error = resolve(OsStr::new("claude"), Some(&path))
                .expect_err("a shell shim is not an executable");
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            assert!(
                error
                    .to_string()
                    .contains(shim.to_str().expect("UTF-8 path"))
            );
            std::fs::remove_file(shim).expect("remove the shim");
        }

        let shim = shims.join("claude.cmd");
        std::fs::write(&shim, "").expect("the shim");
        let executable = install(&dir.path().join("executables"), "claude");
        let path = joined(&[&shims, &dir.path().join("executables")]);
        assert_eq!(
            resolve(OsStr::new("claude"), Some(&path)).expect("exe"),
            executable
        );
        let error = resolve(shim.as_os_str(), Some(&path)).expect_err("explicit shim is refused");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error
                .to_string()
                .contains(shim.to_str().expect("UTF-8 path"))
        );
        for trailing in [".", " "] {
            let mut spelled = shim.as_os_str().to_os_string();
            spelled.push(trailing);
            resolve(&spelled, Some(&path)).expect_err("Win32 strips the trailing character");
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_absolute_name_without_exe_keeps_the_rule_and_suggests_the_existing_file() {
        let dir = tempfile::tempdir().expect("a directory");
        let exe = install(dir.path(), "codex");
        let name = dir.path().join("codex");
        let error = resolve(name.as_os_str(), None).expect_err("the exact absolute name is absent");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error
                .to_string()
                .contains(exe.to_str().expect("UTF-8 path"))
        );
        assert_eq!(resolve(exe.as_os_str(), None).expect("explicit exe"), exe);

        let dotted = dir.path().join("codex.v2");
        let mut dotted_exe = dotted.as_os_str().to_os_string();
        dotted_exe.push(".exe");
        let dotted_exe = PathBuf::from(dotted_exe);
        std::fs::write(&dotted_exe, "").expect("the executable with a dotted name");
        let error = resolve(dotted.as_os_str(), None).expect_err("the exact dotted name is absent");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error
                .to_string()
                .contains(dotted_exe.to_str().expect("UTF-8 path"))
        );
    }
}
