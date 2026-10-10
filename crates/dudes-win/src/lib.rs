//! Windows DACL and filesystem operations behind a small safe surface.
//!
//! The public functions are safe; their Win32 calls live in one Windows-only module whose
//! `unsafe` grant and size are pinned by `tests/unsafe_grant.rs`. Reading the current user's SID
//! uses process-token APIs, while the crate's shipped responsibility remains DACL/filesystem work.
//! Both the operator CLI and daemon can use it without linking the child-ownership crate
//! `dudes-host`. The source guard checks the three job/termination spellings that must stay absent; it
//! does not claim to enumerate every possible process capability.

use std::io;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    Dacl, create_agent_dir_creator_only, create_file_creator_only,
    create_lasting_file_creator_only, create_runtime_dir_creator_only, is_creator_only,
    open_file_creator_only, read_dacl, read_owner,
};

/// `ERROR_BROKEN_PIPE`: the other end of the pipe is closed.
const ERROR_BROKEN_PIPE: i32 = 109;
/// `ERROR_NO_DATA`: the pipe is being closed.
const ERROR_NO_DATA: i32 = 232;
/// `ERROR_PIPE_NOT_CONNECTED`: no client is connected to the server end.
const ERROR_PIPE_NOT_CONNECTED: i32 = 233;

/// Something whose pipe can be peeked without reading or blocking.
///
/// A trait so the reading of its result is testable on every platform; the Windows implementation
/// is [`BorrowedHandle`](std::os::windows::io::BorrowedHandle).
pub trait PipePeek {
    /// Peek the pipe. `Ok` means the other end is still there, whether or not bytes are waiting.
    ///
    /// # Errors
    ///
    /// Whatever the peek reports.
    fn peek(&self) -> io::Result<()>;
}

/// Whether the client of a named pipe has gone: its peek reports a broken pipe, a pipe being
/// closed, or no connected client.
///
/// Any other failure is not read as a hang-up, because the caller drops work on a hang-up and a
/// peek that failed for another reason says nothing about the client.
pub fn pipe_peer_gone(pipe: &impl PipePeek) -> bool {
    matches!(
        pipe.peek().err().and_then(|error| error.raw_os_error()),
        Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED)
    )
}

#[cfg(windows)]
impl PipePeek for std::os::windows::io::BorrowedHandle<'_> {
    fn peek(&self) -> io::Result<()> {
        windows::peek_named_pipe(*self)
    }
}

/// Return the SID of the user this process runs as.
///
/// The function is exported on every platform so callers can inject one platform capability
/// without adding consumer-side `cfg` branches. Only Windows can answer it.
///
/// # Errors
///
/// On Windows, whatever reading the current process token reports. Elsewhere,
/// [`io::ErrorKind::Unsupported`].
pub fn current_user_sid() -> io::Result<String> {
    #[cfg(windows)]
    {
        windows::current_user_sid()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a Windows user SID is unavailable on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Option<i32>);

    impl PipePeek for Fake {
        fn peek(&self) -> io::Result<()> {
            self.0
                .map_or(Ok(()), |code| Err(io::Error::from_raw_os_error(code)))
        }
    }

    /// A pipe whose client is there reports no hang-up; the three codes a vanished client leaves
    /// do; any other failure does not. Mutant: read every error as a hang-up, or none.
    #[test]
    fn only_a_broken_closing_or_unconnected_pipe_is_a_hang_up() {
        assert!(!pipe_peer_gone(&Fake(None)));
        for code in [109, 232, 233] {
            assert!(pipe_peer_gone(&Fake(Some(code))), "{code}");
        }
        for code in [5, 6, 997] {
            assert!(!pipe_peer_gone(&Fake(Some(code))), "{code}");
        }
    }
}
