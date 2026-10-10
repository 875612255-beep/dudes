//! The public PTY interface and the daemon-owned session table.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use dudes_proto::rpc::pty::PtyId;
use dudes_providers::launch::LaunchSpec;

use crate::tree;

/// Maximum bytes returned by one [`PtyReader::read`].
pub const MAX_READ_BYTES: usize = 64 * 1024;
const READ_QUEUE_DEPTH: usize = 8;

/// A pseudoterminal's character-cell dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// One bounded read from an attached pseudoterminal.
#[derive(Debug, PartialEq, Eq)]
pub enum PtyRead {
    /// Exact bytes read from the PTY, never more than [`MAX_READ_BYTES`].
    Data(Vec<u8>),
    /// The PTY output stream closed.
    Eof,
}

/// One PTY close result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// A live host entry was closed.
    Closed,
    /// The host no longer owned that handle.
    AlreadyClosed,
}

/// The read half returned by [`PtyHost::open`].
///
/// It owns only a channel receiver. The daemon-owned host retains every operating-system resource,
/// so dropping this value cannot detach a child from host teardown.
pub struct PtyReader {
    input: Receiver<io::Result<PtyRead>>,
}

impl std::fmt::Debug for PtyReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PtyReader").finish_non_exhaustive()
    }
}

impl PtyReader {
    /// Block until the next bounded data chunk, EOF, or read failure.
    pub fn read(&mut self) -> io::Result<PtyRead> {
        self.input.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the PTY reader ended without reporting EOF",
            )
        })?
    }
}

/// A newly opened PTY and its independent bounded reader.
#[derive(Debug)]
pub struct OpenedPty {
    /// Run-local PTY handle.
    pub id: PtyId,
    /// Bounded output reader.
    pub reader: PtyReader,
}

/// A PTY open failure, distinguished by whether native cleanup proved absence.
#[derive(Debug)]
pub enum OpenError {
    /// No child was created, or a created tree was confirmed absent before return.
    Failed(io::Error),
    /// A child existed and cleanup could not prove its tree absent.
    ///
    /// The host retains `pty`; [`PtyHost::close`] or [`PtyHost::shutdown`] is the recovery route.
    CleanupUnconfirmed {
        /// Reserved handle of the retained cleanup-owned session.
        pty: PtyId,
        /// The operation that made open fail.
        cause: io::Error,
        /// Why cleanup could not prove absence.
        cleanup: io::Error,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) => fmt::Display::fmt(error, formatter),
            Self::CleanupUnconfirmed {
                pty,
                cause,
                cleanup,
            } => write!(
                formatter,
                "PTY {} open failed ({cause}); cleanup is unconfirmed ({cleanup})",
                pty.0
            ),
        }
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Failed(error) | Self::CleanupUnconfirmed { cause: error, .. } => Some(error),
        }
    }
}

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        Self::Failed(error)
    }
}

struct Session {
    native: tree::Session,
    reader: Option<JoinHandle<()>>,
    raw_reader: Option<fs::File>,
    closing: Arc<AtomicBool>,
    #[cfg(test)]
    force_cleanup_uncertainty_once: bool,
}

/// The daemon-owned PTY host.
///
/// There is one per daemon. It owns every live session until [`Self::close`] or
/// [`Self::shutdown`] has synchronously stopped and reaped it.
///
/// # Daemon-exit requirement
///
/// The daemon must call [`Self::shutdown`] on every ordinary exit and every unwinding exit. On
/// Windows, each session's kill-on-close Job also terminates its members if the daemon dies without
/// unwinding. On Unix, dropping this host or killing the daemon with `SIGKILL` does not terminate a
/// process group; the explicit shutdown call is the accepted ownership mechanism.
pub struct PtyHost {
    sessions: BTreeMap<PtyId, Session>,
    next_id: u64,
    #[cfg(test)]
    fail_reader_start_once: bool,
    #[cfg(test)]
    force_cleanup_uncertainty_once: bool,
}

impl std::fmt::Debug for PtyHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyHost")
            .field("sessions", &self.sessions.len())
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl Default for PtyHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyHost {
    /// Construct an empty host.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            next_id: 1,
            #[cfg(test)]
            fail_reader_start_once: false,
            #[cfg(test)]
            force_cleanup_uncertainty_once: false,
        }
    }

    /// Open one PTY from an exact typed launch.
    ///
    /// Attached launches cannot carry launch-time stdin. The child receives only the complete
    /// environment in `spec`, with no inherited or synthesized variable, and `cwd` must name a
    /// directory at this call and again at the operating-system spawn.
    pub fn open(&mut self, spec: LaunchSpec, size: PtySize) -> Result<OpenedPty, OpenError> {
        validate(&spec, size)?;
        let id = PtyId(self.next_id);
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("PTY handle space exhausted"))?;

        let tree::Spawned {
            session,
            reader,
            issue,
        } = tree::spawn(spec, size)?;
        let (send, receive) = mpsc::sync_channel(READ_QUEUE_DEPTH);
        let closing = Arc::new(AtomicBool::new(false));
        self.sessions.insert(
            id,
            Session {
                native: session,
                reader: None,
                raw_reader: Some(reader),
                closing: Arc::clone(&closing),
                #[cfg(test)]
                force_cleanup_uncertainty_once: false,
            },
        );
        self.next_id = next_id;

        if let Some(cause) = issue {
            return Err(self.cleanup_failed_open(id, cause));
        }

        let mut reader = self
            .sessions
            .get_mut(&id)
            .expect("newly adopted PTY")
            .raw_reader
            .take()
            .expect("newly adopted PTY reader");
        let reader_closing = Arc::clone(&closing);
        #[cfg(test)]
        let fail_reader_start = std::mem::take(&mut self.fail_reader_start_once);
        #[cfg(not(test))]
        let fail_reader_start = false;
        let reader_thread = match Self::start_reader(id, fail_reader_start, move || {
            loop {
                let mut bytes = vec![0_u8; MAX_READ_BYTES];
                match tree::read(&mut reader, &mut bytes) {
                    Ok(0) => {
                        let _ = send_bounded(&send, &reader_closing, Ok(PtyRead::Eof));
                        return;
                    }
                    Ok(count) => {
                        bytes.truncate(count);
                        if !send_bounded(&send, &reader_closing, Ok(PtyRead::Data(bytes))) {
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        let _ = send_bounded(&send, &reader_closing, Err(error));
                        return;
                    }
                }
            }
        }) {
            Ok(thread) => thread,
            Err(error) => {
                return Err(self.cleanup_failed_open(id, error));
            }
        };

        self.sessions.get_mut(&id).expect("adopted PTY").reader = Some(reader_thread);
        Ok(OpenedPty {
            id,
            reader: PtyReader { input: receive },
        })
    }

    /// The pid of the child one PTY runs. This host holds it unreaped until the PTY is closed,
    /// so the pid names that child for as long as it is returned.
    #[must_use]
    pub fn pid(&self, pty: PtyId) -> Option<u32> {
        self.sessions.get(&pty).map(|session| session.native.pid())
    }

    /// A duplicate of the kill-on-close Job that contains one PTY's tree.
    ///
    /// Kill-on-close fires when the Job's last handle closes, so while the duplicate is open only
    /// [`Self::close`], [`Self::shutdown`] or process exit ends the tree.
    #[cfg(windows)]
    pub fn job(&self, pty: PtyId) -> io::Result<std::os::windows::io::OwnedHandle> {
        self.sessions
            .get(&pty)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("the PTY handle {} is not open", pty.0),
                )
            })?
            .native
            .job()
    }

    /// Write the exact caller slice once and return the operating system's byte count.
    pub fn write(&mut self, pty: PtyId, bytes: &[u8]) -> io::Result<usize> {
        let session = self.active_session_mut(pty)?;
        session.native.write(bytes)
    }

    /// Change the PTY's character-cell dimensions.
    pub fn resize(&mut self, pty: PtyId, size: PtySize) -> io::Result<()> {
        validate_size(size)?;
        self.active_session_mut(pty)?.native.resize(size)
    }

    /// Stop and reap one PTY tree before returning.
    pub fn close(&mut self, pty: PtyId) -> io::Result<CloseOutcome> {
        let Some(session) = self.sessions.get_mut(&pty) else {
            return Ok(CloseOutcome::AlreadyClosed);
        };
        session.closing.store(true, Ordering::Release);
        drop(session.raw_reader.take());
        let report = session.native.close();
        #[cfg(test)]
        let report = if std::mem::take(&mut session.force_cleanup_uncertainty_once) {
            tree::CloseReport {
                confirmed: false,
                error: Some(io::Error::other("injected close cleanup uncertainty")),
            }
        } else {
            report
        };
        if !report.confirmed {
            return Err(report
                .error
                .unwrap_or_else(|| io::Error::other("PTY cleanup is unconfirmed")));
        }
        let reader = session.reader.take();
        self.sessions.remove(&pty);
        let joined = join_reader(reader);
        match (report.error, joined) {
            (Some(error), _) | (None, Err(error)) => Err(error),
            (None, Ok(())) => Ok(CloseOutcome::Closed),
        }
    }

    /// Stop and reap every PTY tree before returning.
    ///
    /// The daemon calls this on every ordinary and unwinding exit. Windows kill-on-close is an
    /// additional kernel backstop; Unix Drop or `SIGKILL` alone does not terminate process groups.
    /// Every session is attempted. The first error is returned only after the full sweep.
    pub fn shutdown(&mut self) -> io::Result<()> {
        let ids: Vec<PtyId> = self.sessions.keys().copied().collect();
        let mut first = None;
        for id in ids {
            if let Err(error) = self.close(id) {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn session_mut(&mut self, pty: PtyId) -> io::Result<&mut Session> {
        self.sessions.get_mut(&pty).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("the PTY handle {} is not open", pty.0),
            )
        })
    }

    fn active_session_mut(&mut self, pty: PtyId) -> io::Result<&mut Session> {
        let session = self.session_mut(pty)?;
        if session.closing.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the PTY handle is reserved for cleanup",
            ));
        }
        Ok(session)
    }

    fn cleanup_failed_open(&mut self, pty: PtyId, cause: io::Error) -> OpenError {
        #[cfg(test)]
        let force_uncertainty = std::mem::take(&mut self.force_cleanup_uncertainty_once);
        let session = self.sessions.get_mut(&pty).expect("adopted PTY");
        session.closing.store(true, Ordering::Release);
        drop(session.raw_reader.take());
        let report = session.native.close();
        #[cfg(test)]
        let report = if force_uncertainty {
            tree::CloseReport {
                confirmed: false,
                error: Some(io::Error::other("injected cleanup uncertainty")),
            }
        } else {
            report
        };
        if report.confirmed {
            let reader = session.reader.take();
            self.sessions.remove(&pty);
            let cleanup = report.error.or_else(|| join_reader(reader).err());
            OpenError::Failed(match cleanup {
                Some(error) => io::Error::other(format!(
                    "{cause}; native cleanup confirmed absence after error: {error}"
                )),
                None => cause,
            })
        } else {
            OpenError::CleanupUnconfirmed {
                pty,
                cause,
                cleanup: report
                    .error
                    .unwrap_or_else(|| io::Error::other("native cleanup did not prove absence")),
            }
        }
    }

    fn start_reader(
        pty: PtyId,
        fail: bool,
        read: impl FnOnce() + Send + 'static,
    ) -> io::Result<JoinHandle<()>> {
        if fail {
            return Err(io::Error::other("injected reader startup failure"));
        }
        thread::Builder::new()
            .name(format!("dudes-pty-reader-{}", pty.0))
            .spawn(read)
    }
}

fn join_reader(reader: Option<JoinHandle<()>>) -> io::Result<()> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("the PTY reader thread panicked")),
        None => Ok(()),
    }
}

fn send_bounded(
    send: &SyncSender<io::Result<PtyRead>>,
    closing: &AtomicBool,
    mut value: io::Result<PtyRead>,
) -> bool {
    loop {
        match send.try_send(value) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                if closing.load(Ordering::Acquire) {
                    return false;
                }
                value = returned;
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn validate(spec: &LaunchSpec, size: PtySize) -> io::Result<()> {
    validate_size(size)?;
    if spec.argv.is_empty() {
        return Err(invalid("attached launch argv must not be empty"));
    }
    if !matches!(spec.stdin, dudes_providers::launch::LaunchInput::Null) {
        return Err(invalid("attached launch stdin must be absent"));
    }
    for arg in &spec.argv {
        validate_os(arg, "attached launch argv contains a NUL")?;
    }
    for env in &spec.env {
        if env.name().is_empty() {
            return Err(invalid("attached launch environment name is empty"));
        }
        validate_os(
            env.name(),
            "attached launch environment name contains a NUL",
        )?;
        validate_os(
            env.expose_value(),
            "attached launch environment value contains a NUL",
        )?;
        if env.name().as_encoded_bytes().contains(&b'=') {
            return Err(invalid(
                "attached launch environment name contains an equals sign",
            ));
        }
    }
    match fs::metadata(&spec.cwd) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "attached launch cwd is not a directory",
            ));
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn validate_size(size: PtySize) -> io::Result<()> {
    if size.cols == 0
        || size.rows == 0
        || size.cols > i16::MAX as u16
        || size.rows > i16::MAX as u16
    {
        Err(invalid("PTY dimensions must be in 1..=32767"))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn validate_os(value: &OsStr, message: &'static str) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    if value.as_bytes().contains(&0) {
        Err(invalid(message))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn validate_os(value: &OsStr, message: &'static str) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    if value.encode_wide().any(|unit| unit == 0) {
        Err(invalid(message))
    } else {
        Ok(())
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::OpenOptions;
    use std::io::{Seek as _, SeekFrom, Write as _};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use dudes_providers::launch::EnvVar;

    use super::*;

    const EARLY_CHILD: &str = "DUDES_PTY_EARLY_CHILD";
    const EARLY_GRANDCHILD: &str = "DUDES_PTY_EARLY_GRANDCHILD";
    const BEAT: &str = "DUDES_PTY_EARLY_BEAT";

    #[test]
    fn reader_start_failure_distinguishes_confirmed_cleanup_from_retained_uncertainty() {
        let temp = tempfile::tempdir().unwrap();
        let size = PtySize { cols: 80, rows: 24 };

        let mut confirmed = PtyHost::new();
        confirmed.fail_reader_start_once = true;
        assert!(matches!(
            confirmed.open(sleep_spec(temp.path()), size),
            Err(OpenError::Failed(_))
        ));
        assert!(confirmed.sessions.is_empty());

        let mut uncertain = PtyHost::new();
        uncertain.fail_reader_start_once = true;
        uncertain.force_cleanup_uncertainty_once = true;
        let pty = match uncertain.open(sleep_spec(temp.path()), size) {
            Err(OpenError::CleanupUnconfirmed { pty, .. }) => pty,
            other => panic!("expected retained cleanup uncertainty, got {other:?}"),
        };
        assert!(uncertain.sessions.contains_key(&pty));
        assert_cleanup_only(uncertain.write(pty, b"must not reach the PTY"));
        assert_cleanup_only(uncertain.resize(
            pty,
            PtySize {
                cols: 120,
                rows: 40,
            },
        ));
        assert_eq!(uncertain.close(pty).unwrap(), CloseOutcome::Closed);
        assert!(uncertain.sessions.is_empty());
    }

    fn assert_cleanup_only<T: fmt::Debug>(result: io::Result<T>) {
        let error = result.expect_err("cleanup-owned PTY accepted a data-plane operation");
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
        assert_eq!(error.to_string(), "the PTY handle is reserved for cleanup");
    }

    #[test]
    fn post_spawn_group_uncertainty_cleans_an_early_leaders_descendant_before_failure_returns() {
        let temp = tempfile::tempdir().unwrap();
        let beat = temp.path().join("grandchild.beat");
        crate::tree::force_group_issue_once_for_test(&beat);
        let result = PtyHost::new().open(
            early_leader_spec(temp.path(), &beat),
            PtySize { cols: 80, rows: 24 },
        );
        assert!(matches!(result, Err(OpenError::Failed(_))), "{result:?}");
        let before = read_beat(&beat).expect("grandchild established positive-control heartbeat");
        thread::sleep(Duration::from_millis(120));
        assert_eq!(
            read_beat(&beat),
            Some(before),
            "grandchild survived cleanup"
        );
    }

    #[test]
    fn post_spawn_readiness_timeout_still_cleans_the_spawned_child() {
        let temp = tempfile::tempdir().unwrap();
        let absent_beat = temp.path().join("absent.beat");
        let mut host = PtyHost::new();
        crate::tree::force_group_issue_once_for_test(&absent_beat);

        let error = host
            .open(sleep_spec(temp.path()), PtySize { cols: 80, rows: 24 })
            .expect_err("readiness timeout must fail the post-spawn open");

        let OpenError::Failed(error) = error else {
            panic!("readiness timeout left cleanup unconfirmed: {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "PTY_TEST_FIRST_HEARTBEAT_TIMEOUT");
        assert!(
            host.sessions.is_empty(),
            "the spawned child remained owned after confirmed cleanup"
        );
    }

    #[test]
    fn child_status_error_is_returned_only_after_tree_teardown_is_confirmed() {
        let temp = tempfile::tempdir().unwrap();
        let mut host = PtyHost::new();
        let opened = host
            .open(sleep_spec(temp.path()), PtySize { cols: 80, rows: 24 })
            .unwrap();
        crate::tree::force_status_error_once_for_test();
        let error = host.close(opened.id).unwrap_err();
        assert!(error.to_string().contains("status observation"), "{error}");
        assert!(host.sessions.is_empty(), "confirmed cleanup stays owned");
        assert_eq!(host.close(opened.id).unwrap(), CloseOutcome::AlreadyClosed);
    }

    #[test]
    fn retained_close_refuses_data_plane_operations_and_shutdown_recovers() {
        let temp = tempfile::tempdir().unwrap();
        let mut host = PtyHost::new();
        let opened = host
            .open(sleep_spec(temp.path()), PtySize { cols: 80, rows: 24 })
            .unwrap();
        host.sessions
            .get_mut(&opened.id)
            .unwrap()
            .force_cleanup_uncertainty_once = true;

        let error = host.close(opened.id).unwrap_err();
        assert!(error.to_string().contains("cleanup uncertainty"), "{error}");
        assert_cleanup_only(host.write(opened.id, b"must not reach the PTY"));
        assert_cleanup_only(host.resize(
            opened.id,
            PtySize {
                cols: 120,
                rows: 40,
            },
        ));

        host.shutdown().unwrap();
        assert!(host.sessions.is_empty());
    }

    #[test]
    #[ignore = "spawned by the post-spawn cleanup control"]
    fn early_leader_child() {
        if std::env::var_os(EARLY_CHILD).is_none() {
            return;
        }
        let executable = std::env::current_exe().unwrap();
        let beat = PathBuf::from(std::env::var_os(BEAT).unwrap());
        let mut grandchild = Command::new(executable)
            .args([
                "--ignored",
                "--exact",
                "host::tests::early_leader_grandchild",
                "--nocapture",
            ])
            .env_clear()
            .env(EARLY_GRANDCHILD, "1")
            .env(BEAT, &beat)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        thread::spawn(move || {
            let _ = grandchild.wait();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while read_beat(&beat).is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(read_beat(&beat).is_some(), "grandchild did not start");
    }

    #[test]
    #[ignore = "spawned by the early-leader helper"]
    fn early_leader_grandchild() {
        if std::env::var_os(EARLY_GRANDCHILD).is_none() {
            return;
        }
        heartbeat(&PathBuf::from(std::env::var_os(BEAT).unwrap()));
    }

    fn sleep_spec(cwd: &Path) -> LaunchSpec {
        LaunchSpec {
            argv: vec!["/bin/sleep".into(), "60".into()],
            env: Vec::new(),
            cwd: cwd.to_path_buf(),
            stdin: dudes_providers::launch::LaunchInput::Null,
        }
    }

    fn early_leader_spec(cwd: &Path, beat: &Path) -> LaunchSpec {
        LaunchSpec {
            argv: vec![
                std::env::current_exe().unwrap().into_os_string(),
                "--ignored".into(),
                "--exact".into(),
                "host::tests::early_leader_child".into(),
                "--nocapture".into(),
            ],
            env: vec![EnvVar::new(EARLY_CHILD, "1"), EnvVar::new(BEAT, beat)],
            cwd: cwd.to_path_buf(),
            stdin: dudes_providers::launch::LaunchInput::Null,
        }
    }

    fn heartbeat(path: &Path) -> ! {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut count = 0_u64;
        loop {
            file.seek(SeekFrom::Start(0)).unwrap();
            write!(file, "{count:020}").unwrap();
            file.flush().unwrap();
            count = count.wrapping_add(1);
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn read_beat(path: &Path) -> Option<u64> {
        crate::tree::first_heartbeat_for_test(path)
    }
}
