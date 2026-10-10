//! The bounded writes a provider reader requests from its owned protocol child.

use std::fmt;

/// Maximum size of one newline-delimited protocol frame, including its newline.
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// One provider-composed protocol frame. Debug output never includes its payload.
#[derive(Clone, PartialEq, Eq)]
pub struct StartupFrame {
    bytes: Vec<u8>,
    task: bool,
}

impl StartupFrame {
    pub(crate) fn new(mut bytes: Vec<u8>, task: bool) -> Option<Self> {
        bytes.push(b'\n');
        (bytes.len() <= MAX_FRAME_BYTES).then_some(Self { bytes, task })
    }

    /// The complete frame, including its terminating newline.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Whether delivering this frame submits the immutable issued task.
    #[must_use]
    pub const fn is_task(&self) -> bool {
        self.task
    }
}

impl fmt::Debug for StartupFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartupFrame")
            .field("bytes", &self.bytes.len())
            .field("task", &self.task)
            .finish()
    }
}

/// A nonblocking request to the host that owns this child's stdin.
#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolAction {
    /// Queue a bounded provider-composed frame.
    Write(StartupFrame),
    /// The task's matching start response arrived; startup is complete.
    Started,
    /// Close stdin after all already queued frames and require a bounded exit.
    Close,
    /// Fail the protocol and request termination of this exact owned child.
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_include_newline_in_limit_and_hide_payload_in_debug() {
        let frame = StartupFrame::new(b"private task".to_vec(), true).unwrap();
        assert_eq!(frame.bytes(), b"private task\n");
        assert!(frame.is_task());
        assert!(!format!("{frame:?}").contains("private task"));
        assert!(StartupFrame::new(vec![b'x'; MAX_FRAME_BYTES - 1], false).is_some());
        assert!(StartupFrame::new(vec![b'x'; MAX_FRAME_BYTES], false).is_none());
    }
}
