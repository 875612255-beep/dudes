//! Torn-tail repair: a final
//! frame that is short or fails its checksum is one whose `append` never returned, so the
//! writer's open truncates it, logs `journal.torn{lsn}`, and the next append continues from
//! there. Anything that is not the tail is corruption and open refuses. Every test here goes
//! through `open` — the writer's path is the only one that repairs; the read path's half of the
//! rule is `tests/reader_read_only.rs`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use dudes_journal::frame::{FrameError, HEADER_LEN, Header, crc32c};
use dudes_journal::segment::segment_path;
use dudes_journal::{Error, Fsync, JournalOptions, OpenReport, Reader, Torn, open};
use dudes_proto::{Entry, Record};
use dudes_types::{AgentId, Lsn, Timestamp};
use proptest::prelude::*;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record as SpanRecord};
use tracing::{Event, Metadata, Subscriber};

type Captured = (String, String, Vec<(String, String)>);

/// A `tracing::Subscriber` that keeps `(target, message, fields)` of every event.
#[derive(Default)]
struct Capture(Arc<Mutex<Vec<Captured>>>);

struct Fields(String, Vec<(String, String)>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            self.0 = text;
        } else {
            self.1.push((field.name().to_string(), text));
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &SpanRecord<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields(String::new(), Vec::new());
        event.record(&mut fields);
        self.0
            .lock()
            .unwrap()
            .push((event.metadata().target().to_string(), fields.0, fields.1));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// A stamp far enough from the epoch to have the width a real one has.
const AT_BASE: i64 = 1_787_394_600_000;

fn record(i: u64) -> Entry {
    Entry {
        at: Timestamp::from_unix_millis(AT_BASE + i64::try_from(i).unwrap()),
        record: Record::Steer {
            agent: AgentId::from_parts(i, 3),
            text: format!("record {i}"),
            provenance: None,
        },
    }
}

fn options() -> JournalOptions {
    JournalOptions {
        fsync: Fsync::None,
        rotate_at: 1024,
        checkpoint_every: 0,
    }
}

/// Write `n` records, return their lsns and drop the writer.
fn fill(dir: &Path, n: u64) -> Vec<Lsn> {
    let (mut w, _) = open(dir, options()).unwrap();
    (1..=n).map(|i| w.append(&record(i)).unwrap()).collect()
}

fn records(dir: &Path) -> Vec<(Lsn, Entry)> {
    Reader::new(dir)
        .iter_from(Lsn::ZERO)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Open with a capturing subscriber; returns the outcome and every event it emitted.
fn open_with_events(dir: &Path) -> (Result<OpenReport, Error>, Vec<Captured>) {
    let capture = Capture::default();
    let events = Arc::clone(&capture.0);
    let outcome =
        tracing::subscriber::with_default(capture, || open(dir, options()).map(|(_, r)| r));
    let events = events.lock().unwrap().clone();
    (outcome, events)
}

/// Open with a capturing subscriber; returns the report and the `journal.torn` events' fields.
fn open_captured(dir: &Path) -> (OpenReport, Vec<Vec<(String, String)>>) {
    let (outcome, events) = open_with_events(dir);
    let torn = events
        .iter()
        .filter(|(target, message, _)| target == "journal" && message == "journal.torn")
        .map(|(_, _, fields)| fields.clone())
        .collect();
    (outcome.unwrap(), torn)
}

/// Truncating the last segment at any byte inside the last frame is a torn tail: the frame is
/// dropped, every earlier record survives, the `journal.torn` event names the lsn and the
/// dropped bytes, and the next append gets a larger lsn.
#[test]
fn truncated_last_frame_is_dropped_and_logged() {
    for cut_back in [1u64, 5, 20, 30] {
        let dir = tempfile::tempdir().unwrap();
        let lsns = fill(dir.path(), 40);
        let last = *lsns.last().unwrap();
        let path = segment_path(dir.path(), last.segment());
        let whole = fs::metadata(&path).unwrap().len();
        let cut = whole - cut_back;
        assert!(
            cut > last.offset(),
            "the cut must land inside the last frame"
        );
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(cut)
            .unwrap();

        let (report, torn) = open_captured(dir.path());
        let dropped = cut - last.offset();
        assert_eq!(
            report.torn,
            Some(Torn {
                lsn: last,
                dropped_bytes: dropped
            }),
            "cut back {cut_back}"
        );
        assert_eq!(report.frames, 39);
        assert_eq!(fs::metadata(&path).unwrap().len(), last.offset());
        assert_eq!(
            torn,
            vec![vec![
                ("lsn".to_string(), format!("{last}")),
                ("dropped_bytes".to_string(), dropped.to_string()),
            ]]
        );
        let survivors = records(dir.path());
        assert_eq!(survivors.len(), 39);
        assert_eq!(survivors.last().unwrap().1, record(39));

        let (mut w, second) = open(dir.path(), options()).unwrap();
        assert_eq!(second.torn, None, "repair is done once");
        let next = w.append(&record(40)).unwrap();
        assert!(next > last);
        assert_eq!(records(dir.path()).len(), 40);
    }
}

/// A cut exactly on a frame boundary is not torn: nothing to drop, nothing logged.
#[test]
fn cut_on_a_boundary_is_not_torn() {
    let dir = tempfile::tempdir().unwrap();
    let lsns = fill(dir.path(), 10);
    let last = *lsns.last().unwrap();
    let path = segment_path(dir.path(), last.segment());
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(last.offset())
        .unwrap();
    let (report, torn) = open_captured(dir.path());
    assert_eq!(report.torn, None);
    assert!(torn.is_empty());
    assert_eq!(records(dir.path()).len(), 9);
}

/// A crc failure in the final frame (bytes garbled, length intact) is also a torn tail.
#[test]
fn garbled_last_frame_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let lsns = fill(dir.path(), 10);
    let last = *lsns.last().unwrap();
    let path = segment_path(dir.path(), last.segment());
    let mut bytes = fs::read(&path).unwrap();
    let whole = bytes.len();
    bytes[whole - 5] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();
    let (report, torn) = open_captured(dir.path());
    assert_eq!(report.torn.map(|t| t.lsn), Some(last));
    assert_eq!(torn.len(), 1);
    assert_eq!(records(dir.path()).len(), 9);
}

/// A zero-filled tail (the file extended, the data never flushed) is a torn tail even when it
/// is longer than one header; every real record survives.
#[test]
fn zero_filled_tail_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let lsns = fill(dir.path(), 10);
    let last = *lsns.last().unwrap();
    let path = segment_path(dir.path(), last.segment());
    let whole = fs::metadata(&path).unwrap().len();
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&[0u8; 1000]).unwrap();
    drop(file);
    let (report, _) = open_captured(dir.path());
    assert_eq!(
        report.torn,
        Some(Torn {
            lsn: Lsn::from_parts(last.segment(), whole).unwrap(),
            dropped_bytes: 1000,
        })
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), whole);
    assert_eq!(records(dir.path()).len(), 10);
}

/// Torn is the *final* frame only: a damaged `len` on the first frame of the last segment
/// claims an extent past EOF exactly like a cut does, but the intact frames after it were
/// acknowledged. Every single-bit flip of the four `len` bytes is refused as `Corrupt` at that
/// frame's lsn, and the file is left untouched — not one byte truncated.
#[test]
fn damaged_len_before_the_tail_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let lsns = fill(dir.path(), 40);
    let last = *lsns.last().unwrap();
    assert!(
        lsns.iter()
            .filter(|l| l.segment() == last.segment())
            .count()
            > 1,
        "the last segment needs more than one frame"
    );
    let path = segment_path(dir.path(), last.segment());
    let original = fs::read(&path).unwrap();
    for byte in 0..4 {
        for bit in 0..8 {
            let mut bytes = original.clone();
            bytes[byte] ^= 1 << bit;
            fs::write(&path, &bytes).unwrap();
            let err = open(dir.path(), options()).unwrap_err();
            assert!(
                matches!(err, Error::Corrupt { lsn, .. } if lsn == Lsn::from_parts(last.segment(), 0).unwrap()),
                "byte {byte} bit {bit}: {err:?}"
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                bytes,
                "byte {byte} bit {bit}: nothing truncated"
            );
        }
    }
    fs::write(&path, &original).unwrap();
    assert_eq!(
        records(dir.path()).len(),
        40,
        "every acknowledged record survives"
    );
}

/// Corruption that is not the tail — a garbled frame with intact frames after it, or any
/// failure in an earlier segment — is refused, never truncated: those frames were acknowledged.
#[test]
fn corruption_before_the_tail_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let lsns = fill(dir.path(), 40);
    let last = *lsns.last().unwrap();
    assert!(last.segment() > 1, "needs several segments");

    // First frame of the last segment, with frames after it.
    let path = segment_path(dir.path(), last.segment());
    let original = fs::read(&path).unwrap();
    let mut bytes = original.clone();
    bytes[14] ^= 0x01;
    fs::write(&path, &bytes).unwrap();
    let err = open(dir.path(), options()).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                lsn,
                reason: FrameError::Crc { .. }
            } if lsn == Lsn::from_parts(last.segment(), 0).unwrap()
        ),
        "{err:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes, "nothing truncated");
    fs::write(&path, &original).unwrap();

    // A short frame at the end of an earlier segment.
    let first = segment_path(dir.path(), 1);
    let len = fs::metadata(&first).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&first)
        .unwrap()
        .set_len(len - 1)
        .unwrap();
    let err = open(dir.path(), options()).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                reason: FrameError::Short { .. },
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(fs::metadata(&first).unwrap().len(), len - 1);
}

/// Invariant 1: a tail the reader cannot prove torn is refused by name and left
/// byte-identical — promptly. A header claiming `u32::MAX` always opens the frame search, and
/// `TAIL` bytes of `[1, 0, 0xFF, 0]` after it make every fourth offset a ~16 MiB checksum
/// candidate; unbounded, that search is days of work. The search gives up at its checksum
/// budget and says so, and open returns `Corrupt` at the failure lsn without truncating.
#[test]
fn adversarial_tail_is_refused_not_searched() {
    const TAIL: usize = 32 * 1024 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let lsns = fill(dir.path(), 3);
    let last = *lsns.last().unwrap();
    let path = segment_path(dir.path(), last.segment());
    let whole = fs::metadata(&path).unwrap().len();
    // `len = u32::MAX`, then the period-4 bytes.
    let mut tail = vec![0xFFu8, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0, 0, 0, 0];
    tail.extend([1u8, 0, 0xFF, 0].iter().cycle().take(TAIL));
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&tail).unwrap();
    drop(file);
    let started = Instant::now();
    let (outcome, events) = open_with_events(dir.path());
    let elapsed = started.elapsed();
    let err = outcome.unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                lsn,
                reason: FrameError::TooLong { len: u32::MAX }
            } if lsn == Lsn::from_parts(last.segment(), whole).unwrap()
        ),
        "{err:?}"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        whole + tail.len() as u64,
        "nothing truncated"
    );
    let journal_events: Vec<&str> = events
        .iter()
        .filter(|(target, _, _)| target == "journal")
        .map(|(_, message, _)| message.as_str())
        .collect();
    assert_eq!(journal_events, ["journal.search_budget"], "no journal.torn");
    // An order of magnitude inside the bound, three outside the unbounded cost; the nextest
    // duration is the evidence for the sub-second figure.
    assert!(elapsed.as_secs() < 10, "open took {elapsed:?}");
}

/// An intact frame with a future header version is version skew, not a torn tail: the reader
/// must not destroy a newer daemon's records.
#[test]
fn version_skew_is_refused_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    fill(dir.path(), 1);
    let path = segment_path(dir.path(), 1);
    let payload = serde_json::to_vec(&record(2)).unwrap();
    let kind = record(2).record.kind().tag();
    let header = Header {
        len: u32::try_from(payload.len()).unwrap(),
        ver: 3,
        kind,
        crc: crc32c(3, kind, &payload),
    };
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&header.to_bytes()).unwrap();
    file.write_all(&payload).unwrap();
    drop(file);
    let before = fs::metadata(&path).unwrap().len();
    let err = open(dir.path(), options()).unwrap_err();
    assert!(
        matches!(
            err,
            Error::VersionSkew {
                found: 3,
                expected: 2,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        before,
        "nothing truncated"
    );
}

/// Bytes the frame of `record(n)` occupies, header included.
fn frame_len(n: u64) -> usize {
    HEADER_LEN + serde_json::to_vec(&record(n)).unwrap().len()
}

/// `(n_records, cut, garble)`: the last frame is cut to `cut` bytes (`frame_len` = not cut),
/// then one bit of byte `garble` of what is left is flipped. The `len` bytes (0..4) are never
/// garbled here — a shrunk `len` is corruption, a grown one is torn unless a frame follows, and
/// both are pinned by the fixed tests above. At least one mutation is always applied.
fn mutation() -> impl Strategy<Value = (u64, usize, Option<usize>)> {
    (2u64..=40).prop_flat_map(|n| {
        let len = frame_len(n);
        (0..=len).prop_flat_map(move |cut| {
            let garble = if cut == len {
                (4..cut).prop_map(Some).boxed()
            } else if cut > 4 {
                prop_oneof![Just(None), (4..cut).prop_map(Some)].boxed()
            } else {
                Just(None).boxed()
            };
            (Just(n), Just(cut), garble)
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// Torn tail:
    /// for any record count, any cut inside the last frame and any garbled bit after the `len`
    /// bytes, the writer's open reports the torn lsn and the dropped byte count, truncates it to the
    /// last good frame, logs one `journal.torn` with the same fields, keeps every earlier
    /// record, and the next append continues from a position no smaller than the torn one.
    #[test]
    fn any_torn_tail_is_repaired((n, cut, garble) in mutation()) {
        let dir = tempfile::tempdir().unwrap();
        let lsns = fill(dir.path(), n);
        let last = *lsns.last().unwrap();
        let path = segment_path(dir.path(), last.segment());
        let mut bytes = fs::read(&path).unwrap();
        let start = usize::try_from(last.offset()).unwrap();
        prop_assert_eq!(bytes.len() - start, frame_len(n));
        bytes.truncate(start + cut);
        if let Some(i) = garble {
            bytes[start + i] ^= 1 << (i % 8);
        }
        fs::write(&path, &bytes).unwrap();

        let (report, torn) = open_captured(dir.path());
        if cut == 0 {
            prop_assert_eq!(report.torn, None, "a cut on the boundary is not torn");
            prop_assert!(torn.is_empty());
        } else {
            let dropped = u64::try_from(cut).unwrap();
            prop_assert_eq!(report.torn, Some(Torn { lsn: last, dropped_bytes: dropped }));
            prop_assert_eq!(
                torn,
                vec![vec![
                    ("lsn".to_string(), format!("{last}")),
                    ("dropped_bytes".to_string(), dropped.to_string()),
                ]]
            );
        }
        prop_assert_eq!(report.frames, n - 1);
        prop_assert_eq!(fs::metadata(&path).unwrap().len(), last.offset());
        let survivors = records(dir.path());
        prop_assert_eq!(survivors.len(), usize::try_from(n - 1).unwrap());
        prop_assert_eq!(survivors.iter().map(|(l, _)| *l).collect::<Vec<_>>(), lsns[..lsns.len() - 1].to_vec());

        let (mut w, second) = open(dir.path(), options()).unwrap();
        prop_assert_eq!(second.torn, None, "repair is done once");
        let next = w.append(&record(n)).unwrap();
        // The torn position is reused only when it was the start of a segment (the segment is
        // then empty); otherwise open starts a fresh segment past it.
        prop_assert!(next >= last);
        prop_assert!(next > last || last.offset() == 0, "{next} after {last}");
        prop_assert_eq!(records(dir.path()).len(), usize::try_from(n).unwrap());
    }
}
