//! The Floor's port, kept in the state directory from one daemon run to the next.
//!
//! A browser keeps a Floor view's notes and agent order in storage scoped to the page's origin,
//! and the origin includes the port. A port the operating system picks afresh at every start
//! would strand that storage at every restart, so a run with no `DUDES_FLOOR_PORT` asks for the
//! port the last run bound before it asks for a new one.
//!
//! The file is local run configuration, not journal state (invariant 1): no decision reads it,
//! replay neither reads nor rebuilds it, and deleting it costs only the next run's origin.

use std::fs::File;
use std::io::{self, Write as _};
use std::path::Path;

const FILE_NAME: &str = "floor-port";

/// The port the last run's Floor bound, when the file holds one.
pub(crate) fn remembered(state_dir: &Path) -> Option<u16> {
    std::fs::read_to_string(state_dir.join(FILE_NAME))
        .ok()?
        .trim_end()
        .parse()
        .ok()
}

/// Keep `port` for the next run, in a file only this user can read or write.
pub(crate) fn remember(state_dir: &Path, port: u16) -> io::Result<()> {
    writeln!(create_owner_only(&state_dir.join(FILE_NAME))?, "{port}")
}

/// Create or truncate `path`, and leave it owner-only whatever mode it had before.
#[cfg(unix)]
fn create_owner_only(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Create or truncate `path` with a protected creator-only owner and DACL.
#[cfg(windows)]
fn create_owner_only(path: &Path) -> io::Result<File> {
    let file = dudes_win::open_file_creator_only(path)?;
    file.set_len(0)?;
    Ok(file)
}
