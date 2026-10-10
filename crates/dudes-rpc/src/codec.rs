//! The frame: one JSON object per line.
//!
//! Newline-delimited rather than length-prefixed because the same text is what a human types into
//! `nc` when the daemon will not answer, and because the journal already owns the binary framing
//! this transport does not need. `serde_json` never emits a bare newline inside a string — it
//! escapes it as `\n` — so "one object per line" survives any payload, which
//! [`tests::a_payload_containing_a_newline_stays_one_frame`] pins.

use std::io::{BufRead, Write};

pub use dudes_proto::rpc::MAX_FRAME_BYTES;
use serde::Serialize;

/// A framing failure. Not a JSON-RPC error — those are values that travel *inside* a frame.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The frame was longer than [`MAX_FRAME_BYTES`].
    ///
    /// One variant for both directions, because the fault is the same size in each — but **the
    /// recovery is opposite, and the call site is what knows which it is**. From [`read_frame`] the
    /// stream is desynchronised: the rest of the oversized line is still in the buffer and cannot
    /// be skipped without reading exactly the bytes the cap exists to refuse, so the connection
    /// ends. From [`write_frame`] *nothing was written*, so the stream is still in sync and only
    /// the frame is lost — the peer is owed a smaller answer, not a disconnection.
    #[error("a frame exceeded the 1 MiB cap")]
    TooLarge,
    /// The line was read whole but is not UTF-8, so it is not text and cannot be JSON.
    ///
    /// Separate from [`FrameError::Io`] because the two need opposite handling and conflating them
    /// costs the connection: this frame was **consumed to its newline**, so the stream is still in
    /// sync and the peer can be answered and kept, whereas an `Io` failure means the stream itself
    /// is gone. Reported as its own variant rather than as an `io::ErrorKind::InvalidData`, which a
    /// genuine transport failure can also carry.
    #[error("a frame was not valid UTF-8")]
    NotUtf8,
    /// The underlying stream failed.
    #[error("frame i/o: {0}")]
    Io(#[from] std::io::Error),
}

/// Read one frame: everything up to and including the next newline, with the newline removed.
///
/// `Ok(None)` is end of stream — the peer closed, which is not an error. The returned text is not
/// parsed: a frame that is not JSON is a `-32700` *response*, so the caller answers it and keeps
/// the connection, which [`read_frame`] cannot do because it does not know about responses.
///
/// # Errors
///
/// [`FrameError::TooLarge`] when the line exceeds [`MAX_FRAME_BYTES`], [`FrameError::NotUtf8`] when
/// the line is not text, [`FrameError::Io`] when the stream fails.
pub fn read_frame<R: BufRead>(reader: &mut R) -> Result<Option<String>, FrameError> {
    let mut bytes = Vec::new();
    // `take` bounds the read at the cap rather than after it: `read_until` on its own would happily
    // buffer a gigabyte before anyone could object.
    let mut limited = std::io::Read::take(&mut *reader, MAX_FRAME_BYTES as u64 + 1);
    let read = limited.read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    let text = String::from_utf8(bytes).map_err(|_| FrameError::NotUtf8)?;
    Ok(Some(text))
}

/// Serialize `value` as one frame and flush it.
///
/// Flushing is part of the contract, not an optimisation to skip: a request-response transport in
/// which the answer sits in a buffer is a hang, and it is a hang that only shows up under a client
/// that waits.
///
/// **The cap is enforced here too, and enforced before a byte moves.** A frame over the cap is
/// fatal at whoever reads it — it cannot be resynchronised — so writing one does not fail at the
/// writer, it fails at *every* peer, and for a record the journal replays it fails again at each
/// reconnect. The refusal is what stops that, and writing nothing is its load-bearing half: the
/// stream stays in sync, so the peer loses one frame and keeps its connection.
///
/// The predicate is exactly [`read_frame`]'s — the serialized bytes **and** their terminator
/// against [`MAX_FRAME_BYTES`] — so what this function emits is what that one accepts, which
/// [`tests::what_write_frame_emits_read_frame_accepts`] pins from both sides rather than asserting
/// the number twice.
///
/// What a refusal *means* is the caller's, because only the caller knows: a response becomes a
/// JSON-RPC error the peer can read ([`crate::serve_stream`]), and a notification ends its
/// subscription ([`crate::tail::Subscribers::appended`]).
///
/// # Errors
///
/// [`FrameError::TooLarge`] when the frame would exceed [`MAX_FRAME_BYTES`], in which case nothing
/// was written; [`FrameError::Io`] when the stream fails; a serialization failure is reported as
/// an [`std::io::ErrorKind::InvalidData`].
pub fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> Result<(), FrameError> {
    let mut line = serde_json::to_vec(value)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    line.push(b'\n');
    if line.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    writer.write_all(&line)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use dudes_proto::record::{Entry, Record};
    use dudes_proto::rpc::journal::journal_appended;
    use dudes_types::{AgentId, Lsn, Timestamp};
    use serde_json::json;

    use super::*;

    /// The frame is a line: two objects on two lines are two frames, and the newline is not part of
    /// either.
    #[test]
    fn two_lines_are_two_frames_and_then_end_of_stream() {
        let mut reader = Cursor::new(b"{\"a\":1}\n{\"b\":2}\n".to_vec());
        assert_eq!(
            read_frame(&mut reader).unwrap().as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(
            read_frame(&mut reader).unwrap().as_deref(),
            Some("{\"b\":2}")
        );
        assert_eq!(read_frame(&mut reader).unwrap(), None, "end of stream");
    }

    /// A final line with no trailing newline is still a frame — a peer that writes and closes has
    /// not sent half a request.
    #[test]
    fn an_unterminated_final_line_is_a_frame() {
        let mut reader = Cursor::new(b"{\"a\":1}".to_vec());
        assert_eq!(
            read_frame(&mut reader).unwrap().as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    /// The 1 MiB cap is enforced, and it is enforced at the cap rather than near it. The positive
    /// control is the line one byte under: without it a `TooLarge` for everything would pass.
    #[test]
    fn the_frame_cap_is_enforced_at_the_cap() {
        let mut just_under = vec![b'x'; MAX_FRAME_BYTES - 1];
        just_under.push(b'\n');
        let mut reader = Cursor::new(just_under);
        assert_eq!(
            read_frame(&mut reader).unwrap().map(|f| f.len()),
            Some(MAX_FRAME_BYTES - 1),
            "a line one byte under the cap is a frame"
        );

        let mut over = vec![b'x'; MAX_FRAME_BYTES];
        over.push(b'\n');
        let mut reader = Cursor::new(over);
        assert!(
            matches!(read_frame(&mut reader), Err(FrameError::TooLarge)),
            "a line at the cap plus its newline is refused"
        );
    }

    /// "One JSON object per line" holds for any payload, because `serde_json` escapes a newline
    /// inside a string rather than emitting it. Without this the framing would be a data-dependent
    /// injection: a result carrying `\n` would arrive as two frames, the second of them garbage.
    #[test]
    fn a_payload_containing_a_newline_stays_one_frame() {
        let mut out = Vec::new();
        write_frame(&mut out, &json!({ "text": "first\nsecond", "id": 1 })).unwrap();
        assert_eq!(out.last(), Some(&b'\n'), "the frame is terminated");
        assert!(
            !out[..out.len() - 1].contains(&b'\n'),
            "the terminator is the only newline: {}",
            String::from_utf8_lossy(&out)
        );
        let mut reader = Cursor::new(out);
        let frame = read_frame(&mut reader).unwrap().unwrap();
        let back: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(back["text"], "first\nsecond");
    }

    /// Invalid UTF-8 is reported rather than lossily replaced: a frame the daemon cannot read as
    /// text is not a frame whose meaning it should guess at.
    ///
    /// It is reported as [`FrameError::NotUtf8`] and **not** as an `Io` error, because the server
    /// answers one and disconnects on the other. This assertion is what stops the two being merged
    /// back together by someone tidying the enum.
    #[test]
    fn invalid_utf8_is_its_own_error_not_a_replacement_character() {
        let mut reader = Cursor::new(vec![0xff, 0xfe, b'\n']);
        let err = read_frame(&mut reader).unwrap_err();
        assert!(
            matches!(&err, FrameError::NotUtf8),
            "expected NotUtf8, got {err:?}"
        );
    }

    /// A value whose frame — its serialized JSON plus the terminating newline — is exactly
    /// `bytes` long.
    ///
    /// The envelope arithmetic is asserted rather than trusted: `{"t":""}` is eight bytes and the
    /// newline is the ninth, so the payload is `bytes - 9`. Without that assertion a change to the
    /// shape would leave the boundary tests below silently measuring some other length, and
    /// passing.
    fn frame_of_exactly(bytes: usize) -> serde_json::Value {
        let value = json!({ "t": "x".repeat(bytes - 9) });
        assert_eq!(
            serde_json::to_vec(&value).unwrap().len() + 1,
            bytes,
            "the helper's own envelope arithmetic"
        );
        value
    }

    /// One typed `journal.appended` whose complete frame is exactly `bytes` long. Unescaped ASCII
    /// text makes each payload byte move the boundary by one, and the assertion verifies it.
    fn appended_frame_of_exactly(bytes: usize) -> Entry {
        let mut entry = Entry {
            at: Timestamp::from_unix_millis(1_787_394_600_000),
            record: Record::MemoryAppend {
                agent: AgentId::NIL,
                text: String::new(),
                body: None,
                provenance: None,
            },
        };
        let lsn = Lsn(7);
        let overhead = serde_json::to_vec(&journal_appended(lsn, &entry))
            .unwrap()
            .len()
            + 1;
        let Record::MemoryAppend { text, .. } = &mut entry.record else {
            unreachable!("the fixture constructs MemoryAppend")
        };
        text.push_str(&"x".repeat(bytes - overhead));
        assert_eq!(
            serde_json::to_vec(&journal_appended(lsn, &entry))
                .unwrap()
                .len()
                + 1,
            bytes,
            "the fixture's complete notification arithmetic"
        );
        entry
    }

    /// The contract-owned `journal.appended` serializer meets the transport's canonical cap at
    /// the same byte: exact cap is emitted whole, while cap+1 is refused before any byte moves.
    #[test]
    fn appended_notification_uses_the_exact_shared_frame_boundary() {
        let lsn = Lsn(7);
        let at_cap = appended_frame_of_exactly(MAX_FRAME_BYTES);
        let mut accepted = Vec::new();
        write_frame(&mut accepted, &journal_appended(lsn, &at_cap)).unwrap();
        assert_eq!(accepted.len(), MAX_FRAME_BYTES);

        let over_cap = appended_frame_of_exactly(MAX_FRAME_BYTES + 1);
        let mut refused = Vec::new();
        assert!(matches!(
            write_frame(&mut refused, &journal_appended(lsn, &over_cap)),
            Err(FrameError::TooLarge)
        ));
        assert!(
            refused.is_empty(),
            "cap+1 is refused before any byte reaches the writer"
        );
    }

    /// The cap is enforced on the way *out*, and enforced before a byte moves.
    ///
    /// **The empty-writer assertion is the load-bearing half, not the error.** An over-cap frame is
    /// fatal to its connection precisely because its remainder cannot be resynchronised — so a
    /// refusal that had already written a prefix would cost the peer its connection exactly as
    /// writing the whole frame does. Refusing *and writing nothing* is what leaves the stream in
    /// sync and the connection usable.
    ///
    /// The control is the frame one byte under: without it a `write_frame` that refused everything
    /// would satisfy the assertions above it.
    #[test]
    fn an_over_cap_frame_is_refused_before_a_byte_reaches_the_writer() {
        let mut out = Vec::new();
        let err = write_frame(&mut out, &frame_of_exactly(MAX_FRAME_BYTES + 1))
            .expect_err("a frame one byte over the cap is refused");
        assert!(
            matches!(err, FrameError::TooLarge),
            "expected TooLarge, got {err:?}"
        );
        assert!(
            out.is_empty(),
            "nothing may reach the writer, but {} bytes did",
            out.len()
        );

        let mut out = Vec::new();
        write_frame(&mut out, &frame_of_exactly(MAX_FRAME_BYTES))
            .expect("a frame at the cap is written");
        assert_eq!(
            out.len(),
            MAX_FRAME_BYTES,
            "the whole frame, terminator included"
        );
    }

    /// The two halves agree at the boundary: what [`write_frame`] emits, [`read_frame`] accepts.
    ///
    /// This is the property, rather than the number. A writer bound set one byte adrift from the
    /// reader's would still satisfy a test that only asserted `TooLarge` at some size; it cannot
    /// satisfy this one, because the same two frames are put to both functions and the verdicts
    /// have to match.
    #[test]
    fn what_write_frame_emits_read_frame_accepts() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame_of_exactly(MAX_FRAME_BYTES))
            .expect("the writer emits a frame at the cap");
        assert_eq!(
            read_frame(&mut Cursor::new(wire)).unwrap().map(|f| f.len()),
            Some(MAX_FRAME_BYTES - 1),
            "and the reader accepts exactly what was emitted"
        );

        let over = frame_of_exactly(MAX_FRAME_BYTES + 1);
        let mut refused = Vec::new();
        assert!(
            matches!(write_frame(&mut refused, &over), Err(FrameError::TooLarge)),
            "the writer refuses one byte over"
        );
        let mut bytes = serde_json::to_vec(&over).unwrap();
        bytes.push(b'\n');
        assert!(
            matches!(
                read_frame(&mut Cursor::new(bytes)),
                Err(FrameError::TooLarge)
            ),
            "and what it refused is what the reader would have refused, not some other length"
        );
    }
}
