//! The browser form of `dudes floor open`.
//!
//! The one-time link goes into an owner-only launch file in the runtime directory, and one fixed
//! program per platform opens that file. The opener's argv carries only the file's path, which
//! holds no secret. Every failure falls back to printing, and the file is deleted first.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use crate::env::{Environment, NoDirectory};
use crate::floor::{BASE_PATH, FRAGMENT_PREFIX};

const PREFIX: &str = "floor-open-";
const SUFFIX: &str = ".html";
#[cfg(unix)]
const SETTLE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(20);
const MACOS_OPENER: &str = "/usr/bin/open";

/// Windows prints the link rather than opening a browser; the Windows opener stays built for when
/// that is enabled.
const WINDOWS_REHEARSED: bool = false;

/// What decides whether a browser can open here, read once from the environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Desktop {
    /// `SSH_CONNECTION` or `SSH_TTY` is set.
    pub ssh: bool,
    /// `DISPLAY` or `WAYLAND_DISPLAY` is set and not empty.
    pub display: bool,
    /// `SystemRoot`, as the environment gives it.
    pub system_root: Option<PathBuf>,
}

/// The platforms the opener distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl Platform {
    const HOST: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else if cfg!(target_os = "linux") {
        Self::Linux
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Other
    };
}

/// Why the browser form printed the link instead.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Fallback {
    #[error("this is an SSH session, so a browser would open on another screen")]
    Ssh,
    #[error("no graphical display is set")]
    NoDisplay,
    #[error("opening a browser is not yet supported on Linux")]
    Linux,
    #[error("opening a browser is not yet supported on Windows")]
    Windows,
    #[error("opening a browser is not supported on this platform")]
    Platform,
    #[error("SystemRoot does not name an absolute directory")]
    SystemRoot,
    #[error(transparent)]
    NoDirectory(#[from] NoDirectory),
    #[error("the runtime directory {} is not an absolute path", .0.display())]
    Relative(PathBuf),
    #[error("the runtime directory {} is not an owner-only directory of this user", .0.display())]
    NotOwnerOnly(PathBuf),
    #[error("the launch file path {} is one the opener cannot take", .0.display())]
    Path(PathBuf),
    #[error("the Floor link is not a loopback sign-in link")]
    Url,
    #[error("creating the launch file: {0}")]
    Create(io::Error),
    #[error("starting {}: {source}", .program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    #[cfg(unix)]
    #[error("{} exited with {status}", .program.display())]
    Exited {
        program: PathBuf,
        status: std::process::ExitStatus,
    },
}

/// The one program that opens a launch file on this platform. Only [`opener`] makes one.
#[derive(Debug)]
pub(crate) struct Opener(PathBuf);

/// The fixed opener for `platform`, or why there is none. Nothing here searches `PATH`.
fn opener(platform: Platform, desktop: &Desktop) -> Result<Opener, Fallback> {
    if desktop.ssh {
        return Err(Fallback::Ssh);
    }
    match platform {
        Platform::MacOs => Ok(Opener(PathBuf::from(MACOS_OPENER))),
        Platform::Linux if !desktop.display => Err(Fallback::NoDisplay),
        Platform::Linux => Err(Fallback::Linux),
        Platform::Windows if !WINDOWS_REHEARSED => Err(Fallback::Windows),
        Platform::Windows => windows_opener(desktop),
        Platform::Other => Err(Fallback::Platform),
    }
}

/// `<SystemRoot>\explorer.exe`, when `SystemRoot` is absolute.
fn windows_opener(desktop: &Desktop) -> Result<Opener, Fallback> {
    desktop
        .system_root
        .as_deref()
        .filter(|root| is_absolute_windows(root))
        .map(|root| Opener(root.join("explorer.exe")))
        .ok_or(Fallback::SystemRoot)
}

/// A drive-letter or UNC path, judged by its text so the rule is the same on every host.
fn is_absolute_windows(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || text.starts_with(r"\\")
}

/// Whether `explorer.exe` can take `path` as one file argument: no `\\?\` form, which it does not
/// read, and no comma, which it reads as a switch separator.
fn explorer_takes(path: &Path) -> bool {
    let text = path.to_string_lossy();
    !text.starts_with(r"\\?\") && !text.contains(',')
}

/// Delete every regular file named `floor-open-*.html` in `dir`, whatever its age. Failures are
/// ignored and printed nowhere, so `--no-browser` output stays as it was.
pub(crate) fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let launch = name
            .to_str()
            .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(SUFFIX));
        if launch && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Open the Floor at `link`, the fragment-bearing URL, through a launch file.
///
/// # Errors
///
/// A [`Fallback`] naming why nothing opened. The launch file is gone by then.
pub(crate) fn open(env: &Environment, link: &str) -> Result<(), Fallback> {
    let opener = opener(Platform::HOST, env.desktop())?;
    launch(&opener, env.runtime_dir()?, link)
}

/// Write the launch file for `link` in `dir` and hand it to `opener`.
fn launch(opener: &Opener, dir: &Path, link: &str) -> Result<(), Fallback> {
    if !dir.is_absolute() {
        return Err(Fallback::Relative(dir.to_path_buf()));
    }
    let page = launch_page(link).ok_or(Fallback::Url)?;
    let file = dir.join(format!("{PREFIX}{}{SUFFIX}", random_hex()?));
    if Platform::HOST == Platform::Windows && !explorer_takes(&file) {
        return Err(Fallback::Path(file));
    }
    write_launch_file(dir, &file, page.as_bytes())?;
    start(opener, &file).inspect_err(|_| {
        let _ = fs::remove_file(&file);
    })
}

/// 32 lowercase hex characters from 16 random bytes, so nothing can create the path in advance.
fn random_hex() -> Result<String, Fallback> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| Fallback::Create(io::Error::other(error)))?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

/// The launch page for `link`, or `None` when `link` is not exactly a loopback Floor sign-in link.
///
/// The one variable is the link, written as a JSON string literal into a script that replaces the
/// page with it. The page loads nothing else, stores nothing and reads no other input.
fn launch_page(link: &str) -> Option<String> {
    if !is_loopback_sign_in_link(link) {
        return None;
    }
    let literal = serde_json::to_string(link).ok()?;
    Some(format!(
        "<!doctype html>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>Dudes Floor</title>\n\
         <script>location.replace({literal});</script>\n\
         <noscript>Run dudes floor open --no-browser and open the link it prints.</noscript>\n"
    ))
}

/// `^http://(127(\.[0-9]{1,3}){3}|\[::1\]):[0-9]{1,5}/floor/#bootstrap=[A-Za-z0-9_-]{43}$`.
fn is_loopback_sign_in_link(link: &str) -> bool {
    let Some(rest) = link.strip_prefix("http://") else {
        return false;
    };
    let Some(slash) = rest.find('/') else {
        return false;
    };
    let (authority, route) = rest.split_at(slash);
    let Some(bearer) = route
        .strip_prefix(BASE_PATH)
        .and_then(|fragment| fragment.strip_prefix(FRAGMENT_PREFIX))
    else {
        return false;
    };
    let digits = |text: &str, max: usize| {
        (1..=max).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let host_ok = |host: &str| {
        host == "[::1]"
            || host.strip_prefix("127.").is_some_and(|octets| {
                let parts: Vec<&str> = octets.split('.').collect();
                parts.len() == 3 && parts.iter().all(|part| digits(part, 3))
            })
    };
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    host_ok(host)
        && digits(port, 5)
        && bearer.len() == 43
        && bearer
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Create `file` in `dir` as the current user's alone, refusing any path that already exists, a
/// symlink included, then write `page` into it.
#[cfg(unix)]
fn write_launch_file(dir: &Path, file: &Path, page: &[u8]) -> Result<(), Fallback> {
    let refuse = || Fallback::NotOwnerOnly(dir.to_path_buf());
    let meta = fs::metadata(dir).map_err(|_| refuse())?;
    if !meta.is_dir() || meta.permissions().mode() & 0o777 != 0o700 {
        return Err(refuse());
    }
    let mut launch = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
        .map_err(Fallback::Create)?;
    // A new file is owned by this process's user, so it answers "who am I" with no uid lookup,
    // and it is still empty when the answer is no.
    let mine = launch.metadata().map(|created| created.uid() == meta.uid());
    if !mine.unwrap_or(false) {
        let _ = fs::remove_file(file);
        return Err(refuse());
    }
    launch.write_all(page).map_err(|error| {
        let _ = fs::remove_file(file);
        Fallback::Create(error)
    })
}

#[cfg(windows)]
fn write_launch_file(dir: &Path, file: &Path, page: &[u8]) -> Result<(), Fallback> {
    if !dir.is_dir() || !dudes_win::is_creator_only(dir).unwrap_or(false) {
        return Err(Fallback::NotOwnerOnly(dir.to_path_buf()));
    }
    let mut launch = dudes_win::create_lasting_file_creator_only(file).map_err(Fallback::Create)?;
    launch.write_all(page).map_err(|error| {
        let _ = fs::remove_file(file);
        Fallback::Create(error)
    })
}

/// Start `opener` on `file` and let it go.
///
/// On Unix a child still running after two seconds, or one that exited 0, has opened the file. On
/// Windows `explorer.exe` can exit 1 having opened it, so a successful start is enough.
#[expect(
    clippy::disallowed_types,
    reason = "the one fixed opener per platform, handed only the launch file's path"
)]
fn start(opener: &Opener, file: &Path) -> Result<(), Fallback> {
    let program = &opener.0;
    let failed = |source| Fallback::Spawn {
        program: program.clone(),
        source,
    };
    #[cfg(unix)]
    {
        let mut unix_command = std::process::Command::new(program);
        unix_command.arg(file);
        unix_command.stdin(std::process::Stdio::null());
        unix_command.stdout(std::process::Stdio::null());
        unix_command.stderr(std::process::Stdio::null());
        unix_command.env_remove("DUDES_TOKEN");
        unix_command.env_remove("DUDES_SOCK");
        unix_command.process_group(0);
        let mut child = unix_command.spawn().map_err(failed)?;
        let deadline = Instant::now() + SETTLE;
        loop {
            match child.try_wait().map_err(failed)? {
                Some(status) if status.success() => return Ok(()),
                Some(status) => {
                    return Err(Fallback::Exited {
                        program: program.clone(),
                        status,
                    });
                }
                None if Instant::now() >= deadline => return Ok(()),
                None => std::thread::sleep(POLL),
            }
        }
    }
    #[cfg(windows)]
    {
        let mut command = windows_spawn::Command::new(program);
        command.arg(file);
        command.stdin(windows_spawn::Stdio::null());
        command.stdout(windows_spawn::Stdio::null());
        command.stderr(windows_spawn::Stdio::null());
        command.env_remove("DUDES_TOKEN");
        command.env_remove("DUDES_SOCK");
        let options = windows_spawn::SpawnOptions::new()
            .creation_flags(
                windows_spawn::CreationFlags::DETACHED_PROCESS
                    | windows_spawn::CreationFlags::NEW_PROCESS_GROUP,
            )
            .drop_policy(windows_spawn::DropPolicy::Detach);
        command.spawn_with(options).map(drop).map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEARER: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFg";

    fn link() -> String {
        format!("http://127.0.0.1:53211/floor/#bootstrap={BEARER}")
    }

    fn desktop(ssh: bool, display: bool, system_root: Option<&str>) -> Desktop {
        Desktop {
            ssh,
            display,
            system_root: system_root.map(PathBuf::from),
        }
    }

    fn program(result: Result<Opener, Fallback>) -> Result<PathBuf, String> {
        result
            .map(|opener| opener.0)
            .map_err(|error| error.to_string())
    }

    /// One fixed program per platform, and every unsupported condition falls back, as
    /// Windows does.
    #[test]
    fn each_platform_has_one_fixed_opener_or_a_reason() {
        let plain = desktop(false, true, None);
        assert_eq!(
            program(opener(Platform::MacOs, &plain)),
            Ok(PathBuf::from("/usr/bin/open"))
        );
        assert_eq!(
            program(windows_opener(&desktop(false, false, Some(r"C:\Windows")))),
            Ok(Path::new(r"C:\Windows").join("explorer.exe"))
        );
        for root in [Some("Windows"), None] {
            assert_eq!(
                program(windows_opener(&desktop(false, false, root))),
                Err("SystemRoot does not name an absolute directory".to_owned())
            );
        }
        for (platform, facts, reason) in [
            (
                Platform::MacOs,
                desktop(true, true, None),
                "this is an SSH session, so a browser would open on another screen",
            ),
            (
                Platform::Windows,
                desktop(true, false, Some(r"C:\Windows")),
                "this is an SSH session, so a browser would open on another screen",
            ),
            (
                Platform::Linux,
                desktop(false, false, None),
                "no graphical display is set",
            ),
            (
                Platform::Linux,
                desktop(false, true, None),
                "opening a browser is not yet supported on Linux",
            ),
            (
                Platform::Windows,
                desktop(false, false, Some(r"C:\Windows")),
                "opening a browser is not yet supported on Windows",
            ),
            (
                Platform::Other,
                plain.clone(),
                "opening a browser is not supported on this platform",
            ),
        ] {
            assert_eq!(
                program(opener(platform, &facts)),
                Err(reason.to_owned()),
                "{platform:?} {facts:?}"
            );
        }
    }

    /// `explorer.exe` gets no `\\?\` path and no comma, and takes an ordinary one.
    #[test]
    fn explorer_takes_no_verbatim_path_and_no_comma() {
        assert!(explorer_takes(Path::new(
            r"C:\Users\al\AppData\Local\dudes\floor-open-0123.html"
        )));
        assert!(!explorer_takes(Path::new(
            r"\\?\C:\Users\al\AppData\Local\dudes\floor-open-0123.html"
        )));
        assert!(!explorer_takes(Path::new(
            r"C:\Users\a,b\dudes\floor-open-0123.html"
        )));
        assert!(is_absolute_windows(Path::new(r"C:\Windows")));
        assert!(is_absolute_windows(Path::new(r"\\server\share")));
        assert!(!is_absolute_windows(Path::new(r"Windows")));
        assert!(!is_absolute_windows(Path::new(r"C:Windows")));
    }

    /// The writer takes exactly the pattern and refuses everything else.
    #[test]
    fn the_launch_page_takes_only_a_loopback_sign_in_link() {
        for good in [
            link(),
            format!("http://127.255.0.9:1/floor/#bootstrap={BEARER}"),
            format!("http://[::1]:65535/floor/#bootstrap={BEARER}"),
        ] {
            assert!(is_loopback_sign_in_link(&good), "{good}");
        }
        for bad in [
            format!("https://127.0.0.1:53211/floor/#bootstrap={BEARER}"),
            format!("http://10.0.0.1:53211/floor/#bootstrap={BEARER}"),
            format!("http://localhost:53211/floor/#bootstrap={BEARER}"),
            format!("http://127.0.0.1:123456/floor/#bootstrap={BEARER}"),
            format!("http://127.0.0.1/floor/#bootstrap={BEARER}"),
            format!("http://127.0.0.1:53211/floor#bootstrap={BEARER}"),
            format!("http://127.0.0.1:53211/other/#bootstrap={BEARER}"),
            format!("http://127.0.0.1:53211/floor/#bootstrap={BEARER}x"),
            format!("http://127.0.0.1:53211/floor/#bootstrap={}", &BEARER[1..]),
            format!(
                "http://127.0.0.1:53211/floor/#bootstrap=\"</script>{}",
                &BEARER[11..]
            ),
            format!("http://127.0.0.1:53211/floor/?x=1#bootstrap={BEARER}"),
            format!("http://user@127.0.0.1:53211/floor/#bootstrap={BEARER}"),
            "javascript:alert(1)".to_owned(),
        ] {
            assert!(!is_loopback_sign_in_link(&bad), "{bad}");
            assert_eq!(launch_page(&bad), None, "{bad}");
        }
    }

    /// The link is the page's one variable, once, as a JSON string, beside the fixed lines.
    #[test]
    fn the_launch_page_carries_the_link_once_and_nothing_else_variable() {
        let page = launch_page(&link()).expect("a loopback link");
        assert_eq!(
            page,
            format!(
                "<!doctype html>\n\
                 <meta charset=\"utf-8\">\n\
                 <meta name=\"referrer\" content=\"no-referrer\">\n\
                 <title>Dudes Floor</title>\n\
                 <script>location.replace(\"{}\");</script>\n\
                 <noscript>Run dudes floor open --no-browser and open the link it prints.</noscript>\n",
                link()
            )
        );
        assert_eq!(page.matches(BEARER).count(), 1);
    }

    #[cfg(unix)]
    fn runtime_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a runtime directory");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("owner-only");
        dir
    }

    #[cfg(unix)]
    fn launch_files(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .expect("the directory")
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(PREFIX))
            })
            .collect();
        found.sort();
        found
    }

    /// Poll for a stand-in's output file: `launch` can return once the opener is merely still
    /// running past `SETTLE`, before a slow-to-schedule stand-in has written anything, so a test
    /// cannot assume the file is there the instant `launch` returns.
    #[cfg(unix)]
    fn await_stand_in(path: &Path) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            match fs::read_to_string(path) {
                Ok(content) => return content,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the stand-in never wrote {}",
                        path.display()
                    );
                    std::thread::sleep(POLL);
                }
                Err(error) => panic!("the stand-in ran: {error}"),
            }
        }
    }

    /// The file is mode 0600 from creation, holds the page, and its name is 32 lowercase hex.
    #[cfg(unix)]
    #[test]
    fn the_launch_file_is_owner_only_with_a_random_name() {
        let dir = runtime_dir();
        let file = dir
            .path()
            .join(format!("{PREFIX}{}{SUFFIX}", random_hex().expect("hex")));
        write_launch_file(dir.path(), &file, b"page").expect("created");
        let meta = fs::symlink_metadata(&file).expect("the file");
        assert!(meta.is_file());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read(&file).expect("read"), b"page");
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        let hex = &name[PREFIX.len()..name.len() - SUFFIX.len()];
        assert_eq!(hex.len(), 32);
        assert!(
            hex.bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
        assert_ne!(random_hex().expect("hex"), random_hex().expect("hex"));
    }

    /// Creation refuses a planted file and a planted symlink at the chosen name, and writes
    /// nothing through either.
    #[cfg(unix)]
    #[test]
    fn creation_refuses_a_planted_file_and_a_planted_symlink() {
        let dir = runtime_dir();
        let planted = dir.path().join("floor-open-planted.html");
        fs::write(&planted, "theirs").expect("plant a file");
        assert!(matches!(
            write_launch_file(dir.path(), &planted, b"page"),
            Err(Fallback::Create(error)) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&planted).expect("read"), b"theirs");

        let target = dir.path().join("elsewhere");
        let link = dir.path().join("floor-open-link.html");
        std::os::unix::fs::symlink(&target, &link).expect("plant a symlink");
        assert!(matches!(
            write_launch_file(dir.path(), &link, b"page"),
            Err(Fallback::Create(error)) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert!(!target.exists(), "nothing was written through the link");
    }

    /// A runtime directory that is not mode 0700 gets no launch file, and no opener starts.
    #[cfg(unix)]
    #[test]
    fn a_runtime_directory_others_can_read_gets_no_launch_file() {
        let dir = runtime_dir();
        let ran = tempfile::tempdir().expect("a directory outside the runtime directory");
        let marker = ran.path().join("opener-ran");
        let recorder = ran.path().join("opener");
        fs::write(
            &recorder,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .expect("a recording opener");
        fs::set_permissions(&recorder, fs::Permissions::from_mode(0o755)).expect("runnable");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).expect("widen");
        let file = dir.path().join("floor-open-0.html");
        assert!(matches!(
            write_launch_file(dir.path(), &file, b"page"),
            Err(Fallback::NotOwnerOnly(_))
        ));
        assert!(!file.exists());

        let refused = launch(&Opener(recorder), dir.path(), &link());
        assert!(
            matches!(refused, Err(Fallback::NotOwnerOnly(_))),
            "{refused:?}"
        );
        assert_eq!(launch_files(dir.path()), Vec::<PathBuf>::new());
        assert!(!marker.exists(), "no opener started");
    }

    /// The sweep removes every regular launch file and nothing else.
    #[cfg(unix)]
    #[test]
    fn the_sweep_removes_every_launch_file_and_nothing_else() {
        let dir = runtime_dir();
        for name in ["floor-open-a.html", "floor-open-0123456789abcdef.html"] {
            fs::write(dir.path().join(name), "old").expect("an earlier launch file");
        }
        for name in ["operator.cred", "floor-open-a.htm", "other.html"] {
            fs::write(dir.path().join(name), "keep").expect("another file");
        }
        fs::create_dir(dir.path().join("floor-open-dir.html")).expect("a directory");
        std::os::unix::fs::symlink(
            dir.path().join("operator.cred"),
            dir.path().join("floor-open-link.html"),
        )
        .expect("a symlink");
        sweep(dir.path());
        assert_eq!(
            launch_files(dir.path()),
            vec![
                dir.path().join("floor-open-a.htm"),
                dir.path().join("floor-open-dir.html"),
                dir.path().join("floor-open-link.html"),
            ]
        );
        assert!(dir.path().join("operator.cred").exists());
        assert!(dir.path().join("other.html").exists());
    }

    /// A stand-in opener records its argv and standard streams, so the argv is shown to be the
    /// launch file's path alone, with no bearer, and the streams the null device.
    #[cfg(unix)]
    #[test]
    fn the_opener_gets_only_the_launch_file_path_and_null_streams() {
        let dir = runtime_dir();
        let record = dir.path().join("record");
        let stand_in = dir.path().join("opener");
        // The `>` redirect on the braced group opens (and truncates) its target before the
        // commands inside run, so a reader that lands in that window sees an empty file rather
        // than a missing one. Write to a temporary name and `mv` it into place so `record` only
        // ever appears fully written.
        fs::write(
            &stand_in,
            format!(
                "#!/bin/sh\nfor fd in 0 1 2; do \
                 if [ -c /dev/fd/$fd ]; then eval s$fd=null; else eval s$fd=other; fi; done\n\
                 {{ printf '%s\\n' \"$#\" \"$1\" \"$s0\" \"$s1\" \"$s2\"; cat \"$1\"; }} > '{record}.tmp'\n\
                 mv '{record}.tmp' '{record}'\n",
                record = record.display()
            ),
        )
        .expect("a stand-in opener");
        fs::set_permissions(&stand_in, fs::Permissions::from_mode(0o755)).expect("runnable");

        launch(&Opener(stand_in), dir.path(), &link()).expect("opened");
        let recorded = await_stand_in(&record);
        let files = launch_files(dir.path());
        assert_eq!(files.len(), 1, "the file stays for the next sweep");
        let mut lines = recorded.lines();
        assert_eq!(lines.next(), Some("1"), "one argument");
        let argument = lines.next().expect("the argument");
        assert_eq!(Path::new(argument), files[0]);
        assert!(!argument.contains(BEARER), "no bearer in the argv");
        assert_eq!(lines.by_ref().take(3).collect::<Vec<_>>(), ["null"; 3]);
        assert!(
            recorded.contains(&format!("location.replace(\"{}\")", link())),
            "the opener could read the page: {recorded}"
        );
    }

    /// Every failure the CLI can see deletes the launch file before it answers, and a child still
    /// running after the settle time counts as opened.
    #[cfg(unix)]
    #[test]
    fn a_failed_open_deletes_the_launch_file_and_a_running_one_counts_as_opened() {
        let dir = runtime_dir();
        let failed = launch(
            &Opener(PathBuf::from("/usr/bin/false")),
            dir.path(),
            &link(),
        );
        assert!(matches!(failed, Err(Fallback::Exited { .. })), "{failed:?}");
        assert_eq!(launch_files(dir.path()), Vec::<PathBuf>::new());

        let missing = launch(
            &Opener(dir.path().join("no-such-opener")),
            dir.path(),
            &link(),
        );
        assert!(
            matches!(missing, Err(Fallback::Spawn { .. })),
            "{missing:?}"
        );
        assert_eq!(launch_files(dir.path()), Vec::<PathBuf>::new());

        // The slow opener waits for a release file rather than a fixed time, so the test can end
        // it: a child left running after the test holds whatever descriptors it inherited.
        let slow = dir.path().join("slow");
        let (pid_file, release) = (dir.path().join("slow.pid"), dir.path().join("release"));
        fs::write(
            &slow,
            format!(
                "#!/bin/sh\necho $$ > '{0}.tmp'\nmv '{0}.tmp' '{0}'\n\
                 while [ ! -e '{1}' ]; do sleep 0.05; done\nexit 1\n",
                pid_file.display(),
                release.display()
            ),
        )
        .expect("a slow opener");
        fs::set_permissions(&slow, fs::Permissions::from_mode(0o755)).expect("runnable");
        let started = std::time::Instant::now();
        launch(&Opener(slow), dir.path(), &link()).expect("still running counts as opened");
        // Generous past SETTLE (2s): this proves the wait does not block forever on the
        // still-running child, so it only needs to outlast CPU contention under load.
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the wait is bounded"
        );
        assert_eq!(launch_files(dir.path()).len(), 1);
        let pid: i32 = await_stand_in(&pid_file).trim().parse().expect("a pid");
        fs::write(&release, "").expect("release the slow opener");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(pid),
            Some(nix::sys::wait::WaitPidFlag::WNOHANG),
        )
        .expect("the slow opener is this test's child")
            == nix::sys::wait::WaitStatus::StillAlive
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the slow opener outlived its release"
            );
            std::thread::sleep(POLL);
        }

        launch(&Opener(PathBuf::from("/usr/bin/true")), dir.path(), &link()).expect("exit 0");
    }

    /// A relative runtime directory, including one that starts with `-`, reaches no opener.
    #[test]
    fn a_relative_runtime_directory_falls_back() {
        for dir in ["run", "-run"] {
            let fallback = launch(
                &Opener(PathBuf::from("/usr/bin/true")),
                Path::new(dir),
                &link(),
            );
            assert_eq!(
                fallback.map_err(|error| error.to_string()),
                Err(format!(
                    "the runtime directory {dir} is not an absolute path"
                ))
            );
            assert!(!Path::new(dir).exists());
        }
    }
}
