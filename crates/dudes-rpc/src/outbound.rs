//! The write half of a connection, shared: the path a notification takes to a caller.
//!
//! [`crate::Handler`] is a function from a
//! call to a value, and the stream loop was a strict read-frame/write-response pair, so a handler
//! could not reach the writer of the connection it was serving — and a subscription is exactly a
//! handler that has to. This type is that path, and it is the whole of it.
//!
//! Three properties, and each of them is why this is a type rather than a bare `Arc<Mutex<W>>`:
//!
//! - **One frame at a time.** Every write goes through one mutex, so a notification written by
//!   another thread cannot land in the middle of a response line. Interleaving would not corrupt
//!   "some" frames — it would corrupt exactly the two involved, and only under load.
//! - **Closed is a state, not an error you infer.** [`Outbound::close`] is called by the connection
//!   loop when its reader ends, so a subscriber registry has a deterministic signal that a peer is
//!   gone. Waiting for a write to fail works too, but only on the *next* append, which may be
//!   never.
//! - **The writer is erased.** A registry of subscribers is a homogeneous collection, and the
//!   transport is generic over its connection type, so the writer cannot be a type parameter that
//!   reaches all the way out to [`crate::tail::Subscribers`].
//!
//! A poisoned mutex is recovered rather than propagated: a panic in one handler must not make the
//! connection permanently unwritable, and the data behind the lock is a writer with no invariant a
//! panic could have broken.

use std::fmt;
use std::io::Write;
use std::sync::{Mutex, PoisonError};

use serde::Serialize;

use crate::codec::{FrameError, write_frame};

/// A write that did not reach the peer.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// The connection has ended; nothing was written.
    ///
    /// Distinct from [`SendError::Frame`] because it is the *expected* end of a subscription — a
    /// peer that hangs up is not a fault — and a registry drops a subscriber on it without logging
    /// a failure.
    #[error("the connection is closed")]
    Closed,
    /// The frame could not be written.
    #[error(transparent)]
    Frame(#[from] FrameError),
}

/// The write half of one connection, shareable across threads.
pub struct Outbound {
    /// `None` once closed. The state and the writer live under one lock so that "closed" cannot be
    /// observed as true by one thread while another is mid-write.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
}

impl fmt::Debug for Outbound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbound")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl Outbound {
    /// The write half over `writer`.
    ///
    /// `Send + 'static` because the value is erased into a shared registry that outlives the call
    /// that registered it; a borrowed writer could not be.
    pub fn new(writer: impl Write + Send + 'static) -> Self {
        Self {
            writer: Mutex::new(Some(Box::new(writer))),
        }
    }

    /// A write half that swallows everything, for a caller that has no peer.
    #[must_use]
    pub fn sink() -> Self {
        Self::new(std::io::sink())
    }

    /// Serialize `value` as one frame and flush it.
    ///
    /// # Errors
    ///
    /// [`SendError::Closed`] when the connection has ended, [`SendError::Frame`] when the write
    /// failed.
    pub fn send(&self, value: &impl Serialize) -> Result<(), SendError> {
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let writer = guard.as_mut().ok_or(SendError::Closed)?;
        write_frame(writer, value)?;
        Ok(())
    }

    /// End the connection's write half. Idempotent.
    ///
    /// Drops the writer, so a duplicated socket handle is released here rather than whenever the
    /// last subscriber happens to be collected.
    pub fn close(&self) {
        *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Whether [`Outbound::close`] has been called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    /// A writer several threads can share and a test can read back.
    #[derive(Clone, Debug, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Sink {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A frame goes out whole and terminated, and a second frame follows it rather than joining it.
    #[test]
    fn frames_are_written_one_per_line() {
        let sink = Sink::default();
        let out = Outbound::new(sink.clone());
        out.send(&json!({ "n": 1 })).unwrap();
        out.send(&json!({ "n": 2 })).unwrap();
        assert_eq!(sink.text(), "{\"n\":1}\n{\"n\":2}\n");
    }

    /// Closing is the deterministic end: the next send reports [`SendError::Closed`] rather than
    /// succeeding into a dropped socket, and closing twice is not an error.
    ///
    /// The first `send` is the positive control — without it, a type that refused *every* send
    /// would satisfy the assertion below.
    #[test]
    fn a_closed_outbound_refuses_further_sends() {
        let sink = Sink::default();
        let out = Outbound::new(sink.clone());
        out.send(&json!({ "n": 1 })).expect("open, so it writes");
        assert!(!out.is_closed());

        out.close();
        out.close();
        assert!(out.is_closed());
        assert!(matches!(
            out.send(&json!({ "n": 2 })),
            Err(SendError::Closed)
        ));
        assert_eq!(
            sink.text(),
            "{\"n\":1}\n",
            "nothing was written after close"
        );
    }

    /// Concurrent senders do not interleave: every line read back is a whole frame.
    ///
    /// The payload is deliberately long, so an unsynchronised writer would split it across a
    /// `write` boundary rather than getting away with one small atomic call.
    #[test]
    fn concurrent_sends_do_not_interleave() {
        let sink = Sink::default();
        let out = Arc::new(Outbound::new(sink.clone()));
        let payload = "x".repeat(4096);

        std::thread::scope(|scope| {
            for n in 0..8 {
                let out = Arc::clone(&out);
                let payload = payload.clone();
                scope.spawn(move || {
                    for _ in 0..16 {
                        out.send(&json!({ "n": n, "pad": payload })).unwrap();
                    }
                });
            }
        });

        let text = sink.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 8 * 16, "one line per send");
        for line in lines {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("each line is a whole frame");
            assert_eq!(value["pad"].as_str().map(str::len), Some(4096));
        }
    }
}
