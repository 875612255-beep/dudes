//! The journal: the only persistent state, and its only writer (invariant 1).
//!
//! An [`Entry`](dudes_proto::Entry) — a [`Record`](dudes_proto::Record) stamped with the time it
//! entered the machine — is appended as one frame (`len · ver · kind ·
//! crc32c · serde_json payload`, [`frame`]) to the current segment file (`journal/NNNNNN.log`,
//! [`segment`]); [`Writer::append`] returns its [`Lsn`](dudes_types::Lsn) only after the bytes
//! are synced ([`Fsync`]), which is what invariant 2 ("persist before spawn") rests on. On open,
//! The writer's [`open()`] validates every frame and modifies the journal by
//! truncating a torn tail; [`Reader::open`] validates the same way but reports
//! the tail instead of dropping it and stops reading there, so a one-shot offline read
//! (`dudes replay`) never cuts a live daemon's in-flight frame. That bound is fixed at open time,
//! so following a live writer (`journal.tail`) is [`Reader::new`] over a directory [`open()`] has
//! already repaired. [`Reader::iter_from`] streams entries for replay and the tail. [`replay()`]
//! applies entries after `last_applied_lsn` to the redb [`Projections`] (`agents`, `turns`,
//! `goals`, `budget_by_goal`, `budget_by_day`, `mail`), through the same fold live and on replay.
//! [`Projections::spend_views`] is the one read gate 5's admission is
//! built from; the day it buckets by is the caller's, because this crate reads no clock.
//!
//! The one record the writer produces itself is [`Record::Checkpoint`](dudes_proto::Record),
//! written every [`JournalOptions::checkpoint_every`] appends and naming its own frame's
//! position. **Nothing reads one**: [`replay()`] applies it as a no-op, `agents`, `goals` and
//! `turns` name it in an arm that does nothing, and `budget` never reaches a record arm for it
//! at all — see [`Writer::emit_checkpoint`] for what it does and does not attest, and
//! `tests/checkpoint.rs` for the pin that keeps it inert.
//!
//! This crate does no async work and reads no config file: the daemon parses `[daemon] fsync`
//! into [`JournalOptions`] and owns the writer in its journal task.

pub mod error;
pub mod frame;
mod integrity;
pub mod memory;
pub mod projections;
pub mod reader;
pub mod replay;
pub mod segment;
pub mod writer;

use std::fs;
use std::path::{Path, PathBuf};

pub use error::{Error, Result};
pub use integrity::{body_integrity, mail_body_digest, mail_recipient_id};
pub use memory::{
    CondensePlan, INLINE_MEMORY_BYTES, MemoryApply, MemoryCondenseConfig, MemoryCondenseError,
    MemoryCorruption, MemoryEntry, MemoryPage, MemoryProjection, MemorySearchError, MemorySummary,
    MemorySummaryKind, PreparedMemoryAppend, PreparedMemoryCondense, Withheld, WithheldAppend,
};
pub use projections::{
    AgentRow, ChatOverlay, GoalRow, PROJECTION_VERSION, Projections, SpendViews, TurnRow,
    WorktreeRow, admits_pending,
};
pub use reader::{Iter, Location, OpenReport, Reader, Torn};
pub use replay::{ReplayReport, replay};
pub use segment::{DEFAULT_ROTATE_AT, FIRST_SEGMENT};
pub use writer::{Fsync, JournalOptions, Writer};

/// Open the journal in `dir` for writing — the single writer, and the only path that creates the
/// directory or truncates: validate every segment, repair a torn tail, then
/// start a fresh segment — unless the last one is
/// empty, which is reused so restarts do not pile up empty files.
pub fn open(dir: &Path, options: JournalOptions) -> Result<(Writer, OpenReport)> {
    open_in(dir, options, None)
}

/// Open a writer using `scratch_directory` for temporary completed-contract validation.
///
/// The caller owns the scratch choice. The directory must already exist, be writable, and be
/// outside `dir`. Validation uses an anonymous file with an 8 MiB database cache and mode `0600`
/// on Unix. It may contain task or note text from the canonical projection. A tmpfs directory
/// consumes filesystem memory beyond process RSS. Storage failures name the directory and step.
/// The writer's readers retain this choice when a changed prefix needs validation again.
pub fn open_with_scratch(
    dir: &Path,
    options: JournalOptions,
    scratch_directory: &Path,
) -> Result<(Writer, OpenReport)> {
    let scratch = reader::resolve_validation_scratch(scratch_directory)?;
    open_in(dir, options, Some(scratch))
}

fn open_in(
    dir: &Path,
    options: JournalOptions,
    validation_scratch: Option<PathBuf>,
) -> Result<(Writer, OpenReport)> {
    fs::create_dir_all(dir)?;
    let (report, layout) = reader::repair(dir, validation_scratch.as_deref())?;
    let segment = match layout.segments.last() {
        Some(&last) if layout.last_len == 0 => last,
        Some(&last) => last + 1,
        None => FIRST_SEGMENT,
    };
    let mut writer = Writer::start(dir, segment, options)?;
    writer.validated = Some(layout.validated);
    writer.validation_scratch = validation_scratch;
    Ok((writer, report))
}

#[cfg(test)]
mod tests {
    use dudes_proto::{Entry, Record};
    use dudes_types::{AgentId, Lsn, Timestamp};

    use super::*;

    fn record() -> Entry {
        Entry {
            at: Timestamp::from_unix_millis(1_787_394_600_000),
            record: Record::Resume {
                agent: AgentId::NIL,
                browser: None,
            },
        }
    }

    fn options() -> JournalOptions {
        JournalOptions {
            fsync: Fsync::None,
            ..JournalOptions::default()
        }
    }

    /// Segments: a daemon start rotates — a reopened journal with records starts a new
    /// segment and never appends to the old one; an empty last segment is reused.
    #[test]
    fn open_rotates_on_start_unless_last_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (w, report) = open(dir.path(), options()).unwrap();
        assert_eq!(report, OpenReport::default());
        assert_eq!(w.segment(), 1);
        drop(w);
        // Nothing was written: segment 1 is empty and reused.
        let (mut w, _) = open(dir.path(), options()).unwrap();
        assert_eq!(w.segment(), 1);
        let lsn = w.append(&record()).unwrap();
        drop(w);
        let (w, report) = open(dir.path(), options()).unwrap();
        assert_eq!(w.segment(), 2);
        assert_eq!(report.frames, 1);
        assert_eq!(report.end.segment(), 1);
        assert!(report.end > lsn);
        assert_eq!(w.next_lsn().unwrap(), Lsn::from_parts(2, 0).unwrap());
        assert_eq!(segment::list_segments(dir.path()).unwrap(), vec![1, 2]);
    }

    /// Segments are contiguous: a missing segment between present ones is missing
    /// records, and `open` refuses by name before touching anything.
    #[test]
    fn segment_gap_is_refused_on_open() {
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..3 {
            let (mut w, _) = open(dir.path(), options()).unwrap();
            w.append(&record()).unwrap();
        }
        assert_eq!(segment::list_segments(dir.path()).unwrap(), vec![1, 2, 3]);
        std::fs::remove_file(segment::segment_path(dir.path(), 2)).unwrap();
        let err = open(dir.path(), options()).unwrap_err();
        assert!(
            matches!(err, Error::SegmentGap { after: 1, found: 3 }),
            "{err:?}"
        );
        let err = Reader::open(dir.path()).unwrap_err();
        assert!(matches!(err, Error::SegmentGap { .. }), "{err:?}");
    }
}
