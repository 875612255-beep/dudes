//! The daemon's one-entry `agent-bin` directory: `<runtime_dir>/agent-bin/dudes`, a symbolic link
//! to the `dudes` file beside the daemon's own executable.
//!
//! A managed agent runs `dudes mail ...` through its own shell. This directory goes first on the
//! agent's `PATH` so an older `dudes` further along cannot answer instead. It holds only that one
//! link, never the daemon's whole executable directory, so no other tool on the operator's `PATH`
//! changes precedence.

use std::path::{Path, PathBuf};

/// The directory's name under the runtime directory.
const DIR: &str = "agent-bin";

/// Rebuild the directory for this run and answer its path, or answer `None` and leave no entry
/// when the sibling is missing, the executable path is unknown or the link cannot be made. Logs one
/// warning in each of those cases. Does nothing off Unix.
pub(crate) fn prepare(runtime_dir: &Path, exe: Option<&Path>) -> Option<PathBuf> {
    let dir = runtime_dir.join(DIR);
    remove(&dir);
    match build(&dir, exe) {
        Ok(()) => Some(dir),
        Err(reason) => {
            remove(&dir);
            if cfg!(unix) {
                tracing::warn!(
                    reason,
                    "agents will not get the daemon's own `dudes` first on PATH"
                );
            }
            None
        }
    }
}

/// Best effort: a leftover directory is rebuilt by the next start regardless.
pub(crate) fn remove(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
fn build(dir: &Path, exe: Option<&Path>) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;

    const LINK: &str = "dudes";

    let exe = exe.ok_or("the daemon's own executable path is unavailable")?;
    let sibling = exe
        .parent()
        .map(|parent| parent.join(LINK))
        .filter(|sibling| sibling.is_absolute() && sibling.is_file())
        .ok_or_else(|| format!("no `dudes` file beside {}", exe.display()))?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .map_err(|error| format!("creating {}: {error}", dir.display()))?;
    let temporary = dir.join(".dudes.new");
    std::os::unix::fs::symlink(&sibling, &temporary)
        .and_then(|()| std::fs::rename(&temporary, dir.join(LINK)))
        .map_err(|error| format!("linking {}: {error}", sibling.display()))
}

#[cfg(not(unix))]
fn build(_dir: &Path, _exe: Option<&Path>) -> Result<(), String> {
    Err("agent mail does not exist on this platform".to_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).expect("bin");
        std::fs::write(bin.join("duded"), "").expect("duded");
        std::fs::write(bin.join("dudes"), "").expect("dudes");
        std::fs::write(bin.join("claude"), "").expect("another tool");
        let run = root.path().join("run");
        std::fs::create_dir(&run).expect("run");
        (root, bin.join("duded"), run)
    }

    fn entries(dir: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect()
    }

    #[test]
    fn one_entry_pointing_at_the_sibling_and_nothing_else() {
        let (root, exe, run) = layout();
        let dir = prepare(&run, Some(&exe)).expect("prepared");
        assert_eq!(dir, run.join("agent-bin"));
        assert_eq!(entries(&dir), [std::ffi::OsString::from("dudes")]);
        assert_eq!(
            std::fs::read_link(dir.join("dudes")).expect("a link"),
            root.path().join("bin/dudes")
        );
        let mode = std::fs::metadata(&dir).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_stale_entry_does_not_survive_a_start() {
        let (_root, exe, run) = layout();
        let dir = run.join("agent-bin");
        std::fs::create_dir(&dir).expect("dir");
        std::os::unix::fs::symlink("/old/dudes", dir.join("dudes")).expect("stale link");
        std::fs::write(dir.join("claude"), "").expect("stale extra");
        prepare(&run, Some(&exe)).expect("prepared");
        assert_eq!(entries(&dir), [std::ffi::OsString::from("dudes")]);
        assert_ne!(
            std::fs::read_link(dir.join("dudes")).expect("a link"),
            Path::new("/old/dudes")
        );
    }

    #[test]
    fn a_missing_sibling_leaves_no_directory() {
        let (_root, exe, run) = layout();
        std::fs::remove_file(exe.with_file_name("dudes")).expect("remove sibling");
        assert_eq!(prepare(&run, Some(&exe)), None);
        assert!(!run.join("agent-bin").exists());
    }

    #[test]
    fn an_unknown_executable_leaves_no_directory() {
        let (_root, _exe, run) = layout();
        assert_eq!(prepare(&run, None), None);
        assert!(!run.join("agent-bin").exists());
    }
}
