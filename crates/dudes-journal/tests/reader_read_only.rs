//! The read path never changes the journal. `Reader::open` is what
//! a one-shot offline read — `dudes replay` — uses; if it truncates, it cuts the frame a running
//! daemon is in the middle of `write_all` and that record is lost on the daemon's next start.
//! The writer drops the torn bytes; the reader bounds itself before them.
//!
//! Every assertion here that something did *not* change carries a control proving it can see the
//! change: the same fixture on the writer's path does shrink, the same missing directory on the
//! writer's path is created, the same read-only segment the reader opens fine makes the writer
//! fail, and the same unreadable path reads fine once its mode is restored. A green run in which
//! the read path had quietly become a no-op is not possible.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use dudes_journal::segment::{list_segments, segment_path};
use dudes_journal::{
    Error, FIRST_SEGMENT, Fsync, JournalOptions, Location, OpenReport, Reader, Torn, Writer, frame,
    open,
};
use dudes_proto::{Entry, Record};
use dudes_types::{AgentId, Lsn, Timestamp};
use tempfile::TempDir;

/// Records acknowledged before the one that is still in flight.
const ACKED: u64 = 8;

/// A record whose frame is a few hundred bytes, so a 40-byte prefix of it stops inside the
/// payload with the header already parsable.
fn record(i: u64) -> Entry {
    Entry {
        at: Timestamp::from_unix_millis(1_787_394_600_000 + i64::try_from(i).unwrap()),
        record: Record::MemoryAppend {
            agent: AgentId::from_parts(i, 1),
            text: "y".repeat(300),
            body: None,
            provenance: None,
        },
    }
}

fn options() -> JournalOptions {
    JournalOptions {
        fsync: Fsync::None,
        ..JournalOptions::default()
    }
}

/// Every name in a directory with its length, sorted. Comparing one segment's bytes cannot see a
/// read path that *created* something — a lock file, a stray segment — and "the read path writes
/// nothing" is a claim about the directory, not about one file in it.
fn dir_snapshot(dir: &Path) -> Vec<(String, u64)> {
    let mut entries: Vec<(String, u64)> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.metadata().unwrap().len(),
            )
        })
        .collect();
    entries.sort();
    entries
}

/// A journal whose writer is in the middle of `write_all` for record `ACKED + 1`.
///
/// What makes this an append in flight rather than merely a short file: the earlier frames are
/// the real [`Writer`]'s own bytes; that writer is *still alive* and still holds the segment with
/// its own tracked length; and the bytes on disk are a strict prefix of a frame whose remainder
/// is held in `rest` and does get written afterwards. A reader that truncates here destroys work
/// that is still happening.
struct InFlight {
    /// Alive: this append has not returned.
    writer: Writer,
    /// The handle the in-flight `write_all` is using, still open.
    raw: File,
    path: PathBuf,
    /// Where the in-flight frame starts.
    tail_at: Lsn,
    /// The bytes of that frame that have not reached the file yet.
    rest: Vec<u8>,
    /// Last, so it is removed after `writer` and `raw` are closed: on Windows a directory with
    /// open handles in it cannot be removed, and `TempDir::drop` swallows the failure.
    dir: TempDir,
}

fn in_flight(prefix: usize) -> InFlight {
    let dir = tempfile::tempdir().unwrap();
    let (mut writer, _) = open(dir.path(), options()).unwrap();
    for i in 1..=ACKED {
        writer.append(&record(i)).unwrap();
    }
    let path = segment_path(dir.path(), writer.segment());
    let tail_at = writer.next_lsn().unwrap();

    let next = record(ACKED + 1);
    let payload = serde_json::to_vec(&next).unwrap();
    let mut whole = Vec::new();
    frame::encode(next.record.kind().tag(), &payload, &mut whole).unwrap();
    assert!(prefix < whole.len(), "a prefix of a frame, not a whole one");
    let mut raw = OpenOptions::new().append(true).open(&path).unwrap();
    raw.write_all(&whole[..prefix]).unwrap();

    InFlight {
        writer,
        raw,
        path,
        tail_at,
        rest: whole[prefix..].to_vec(),
        dir,
    }
}

/// Permission bits mean nothing to root, so the tests that rely on them would pass for the wrong
/// reason there. Probe rather than assume: can we create a file in a directory that forbids it?
#[cfg(unix)]
#[allow(
    clippy::print_stderr,
    reason = "a test that returns early still prints PASS; why it did not run must be recoverable"
)]
fn running_as_root() -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let probe = tempfile::tempdir().unwrap();
    fs::set_permissions(probe.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let created = File::create(probe.path().join("probe")).is_ok();
    fs::set_permissions(probe.path(), fs::Permissions::from_mode(0o755)).unwrap();
    if created {
        eprintln!("skipped: running as root, which ignores the mode bits this test relies on");
    }
    created
}

/// Invariant 1: `Reader::open` leaves the journal byte-identical, and adds no file
/// to the directory. The control is the same fixture handed to `open` — the writer's path, which
/// does truncate it — so a passing assertion above cannot be a fixture that was never torn in the
/// first place.
#[test]
fn reader_open_is_read_only() {
    for prefix in [6usize, 40] {
        let dropped = u64::try_from(prefix).unwrap();

        let fixture = in_flight(prefix);
        let before = fs::read(&fixture.path).unwrap();
        let before_dir = dir_snapshot(fixture.dir.path());
        let (_reader, report) = Reader::open(fixture.dir.path()).unwrap();
        let after = fs::read(&fixture.path).unwrap();
        assert_eq!(
            before.len(),
            after.len(),
            "prefix {prefix}: the read path changed the length"
        );
        assert!(
            before == after,
            "prefix {prefix}: the read path changed a byte"
        );
        assert_eq!(
            before_dir,
            dir_snapshot(fixture.dir.path()),
            "prefix {prefix}: the read path added, removed or resized something in the directory"
        );
        assert_eq!(report.frames, ACKED);
        assert_eq!(
            report.torn,
            Some(Torn {
                lsn: fixture.tail_at,
                dropped_bytes: dropped
            }),
            "prefix {prefix}: the tail is reported, not dropped"
        );

        // Control: the writer's path on an identical fixture does shrink the segment.
        let InFlight {
            writer,
            raw,
            path: control_path,
            tail_at: control_tail,
            dir: control_dir,
            ..
        } = in_flight(prefix);
        let whole = fs::metadata(&control_path).unwrap().len();
        // A live writer is what makes the *fixture* meaningful, not the *control*: holding one
        // here would make this `open` fail on the lock once the writer lock lands, and the control would
        // then be asserting the wrong thing.
        drop(raw);
        drop(writer);
        let (_writer, report) = open(control_dir.path(), options()).unwrap();
        let repaired = fs::metadata(&control_path).unwrap().len();
        assert_eq!(
            whole - repaired,
            dropped,
            "prefix {prefix}: control — the writer's path did not truncate, so the assertions \
             above prove nothing"
        );
        assert_eq!(
            report.torn,
            Some(Torn {
                lsn: control_tail,
                dropped_bytes: dropped
            })
        );
    }
}

/// A structural control that does not depend on comparing bytes: with the segment mode 0o444 the
/// read path still opens the journal and the writer's path cannot. It catches a write handle
/// taken on *that segment file* — the file the read path has any reason to open — which is where
/// truncation would have to happen; `reader_open_is_read_only`'s directory snapshot covers the
/// rest.
#[cfg(unix)]
#[test]
fn reader_open_takes_no_write_handle() {
    use std::os::unix::fs::PermissionsExt as _;
    if running_as_root() {
        return;
    }
    let read_only = fs::Permissions::from_mode(0o444);

    let fixture = in_flight(40);
    fs::set_permissions(&fixture.path, read_only.clone()).unwrap();
    let (_reader, report) =
        Reader::open(fixture.dir.path()).expect("the read path needs no write handle");
    assert_eq!(report.torn.map(|torn| torn.lsn), Some(fixture.tail_at));

    // Control: the writer's path must take one, and is refused. Its writer is dropped first for
    // the same reason as in `reader_open_is_read_only`.
    let InFlight {
        writer,
        raw,
        path: control_path,
        dir: control_dir,
        ..
    } = in_flight(40);
    drop(raw);
    drop(writer);
    fs::set_permissions(&control_path, read_only).unwrap();
    let err = open(control_dir.path(), options())
        .expect_err("control — the writer's path took no write handle either");
    assert!(
        matches!(&err, Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "{err:?}"
    );
}

/// A missing directory is a question, not an empty journal: the read path refuses it by name and
/// creates nothing. Control: the writer's path creates the very same directory, so "it was not
/// created" is asserted against a path that demonstrably can be.
#[test]
fn reader_open_refuses_a_missing_dir() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("no-journal-here");

    let err = Reader::open(&missing).unwrap_err();
    assert!(
        matches!(&err, Error::NoJournal { dir } if dir == &missing),
        "{err:?}"
    );
    assert!(!missing.exists(), "the read path created the directory");

    let (_writer, report) = open(&missing, options()).unwrap();
    assert!(
        missing.is_dir(),
        "control — the writer's path did not create the directory either"
    );
    assert_eq!(report, OpenReport::default());
}

/// An existing directory without a segment is not a journal yet. The read path refuses it by
/// name and leaves it empty; the writer's path initializes that same directory as a journal.
#[test]
fn reader_open_refuses_a_segmentless_directory_and_writer_initializes_it() {
    let dir = tempfile::tempdir().unwrap();

    let err = Reader::open(dir.path()).unwrap_err();
    assert!(
        matches!(&err, Error::NotAJournal { dir: refused } if refused == dir.path()),
        "{err:?}"
    );
    assert_eq!(
        list_segments(dir.path()).unwrap(),
        Vec::<u32>::new(),
        "the read path must not initialize the journal"
    );

    let (writer, report) = open(dir.path(), options()).unwrap();
    assert_eq!(report, OpenReport::default());
    assert_eq!(writer.segment(), FIRST_SEGMENT);
    assert_eq!(list_segments(dir.path()).unwrap(), vec![FIRST_SEGMENT]);
}

/// `NoJournal` means *absent*, and only absent. `Path::is_dir` is
/// `metadata(..).map(..).unwrap_or(false)`, so it answers `false` for a journal the OS merely
/// refused to describe — and telling an operator with a permissions problem that there is no
/// journal invites them to create one on top of the one that is there. Control: the same path and
/// the same call succeed once the mode is restored, so the error was the permissions and not the
/// path.
#[cfg(unix)]
#[test]
fn reader_open_tells_absent_from_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    if running_as_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    let journal = outer.join("journal");
    fs::create_dir_all(&journal).unwrap();
    let (writer, report) = open(&journal, options()).unwrap();
    assert_eq!(report, OpenReport::default());
    drop(writer);

    fs::set_permissions(&outer, fs::Permissions::from_mode(0o000)).unwrap();
    let refused = Reader::open(&journal).unwrap_err();
    fs::set_permissions(&outer, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(&refused, Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "a journal the OS refused must not be reported as absent: {refused:?}"
    );

    let (_reader, report) = Reader::open(&journal).expect("control — the path itself was fine");
    assert_eq!(
        report,
        OpenReport {
            torn: None,
            end: Lsn::from_parts(FIRST_SEGMENT, 0).unwrap(),
            frames: 0,
        }
    );

    // The other half of the distinction, on the same parent: absent really is `NoJournal`.
    let absent = outer.join("nothing-here");
    assert!(
        matches!(Reader::open(&absent).unwrap_err(), Error::NoJournal { .. }),
        "absent and refused must not collapse into one answer"
    );
}

/// End to end: a reader looks at the journal while an append is in
/// flight, the `write_all` completes, and the record survives. With a truncating reader the
/// completion lands at the shortened end as a headless payload and the record is lost.
#[test]
fn live_writer_completes_after_a_reader_open() {
    let InFlight {
        dir,
        writer,
        mut raw,
        path,
        tail_at,
        rest,
    } = in_flight(40);

    let (_reader, report) = Reader::open(dir.path()).unwrap();
    assert!(
        report.torn.is_some(),
        "the fixture must really look torn to the reader, or it proves nothing"
    );

    // The writer's `write_all` finishes.
    raw.write_all(&rest).unwrap();
    drop(raw);
    drop(writer);

    let (_writer, report) = open(dir.path(), options()).unwrap();
    assert_eq!(report.torn, None, "the completed frame is not a torn tail");
    assert_eq!(report.frames, ACKED + 1);
    assert!(fs::metadata(&path).unwrap().len() > tail_at.offset());

    let got: Vec<(Lsn, Entry)> = Reader::new(dir.path())
        .iter_from(Lsn::ZERO)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(u64::try_from(got.len()).unwrap(), ACKED + 1);
    assert_eq!(got[usize::try_from(ACKED).unwrap()].0, tail_at);
    assert_eq!(got.last().unwrap().1, record(ACKED + 1));
}

/// The tail the reader does not drop is the tail it does not read. Truncating would do this
/// job silently: a crc-failing final frame was cut, so iteration ended cleanly. Bounding the
/// reader at the last good frame does the same without writing to the journal — otherwise offline replay would
/// report `Corrupt` on exactly the crash it exists to recover from.
#[test]
fn reader_open_stops_at_an_unrepaired_tail() {
    let dir = tempfile::tempdir().unwrap();
    let (mut writer, _) = open(dir.path(), options()).unwrap();
    let lsns: Vec<Lsn> = (1..=ACKED)
        .map(|i| writer.append(&record(i)).unwrap())
        .collect();
    let path = segment_path(dir.path(), writer.segment());
    drop(writer);

    // Garble the last byte of the final frame: length intact, checksum wrong — the tail a
    // half-flushed write leaves behind, which `Short` tolerance alone does not catch.
    let mut bytes = fs::read(&path).unwrap();
    let last_byte = bytes.len() - 1;
    bytes[last_byte] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();
    let last = *lsns.last().unwrap();

    let (reader, report) = Reader::open(dir.path()).unwrap();
    assert_eq!(report.torn.map(|torn| torn.lsn), Some(last));
    assert!(fs::read(&path).unwrap() == bytes, "the read path wrote");

    let got: Vec<(Lsn, Entry)> = reader
        .iter_from(Lsn::ZERO)
        .unwrap()
        .collect::<Result<_, _>>()
        .expect("iteration ends at the tail instead of reporting it as corruption");
    assert_eq!(u64::try_from(got.len()).unwrap(), ACKED - 1);
    assert_eq!(
        got.last().unwrap().0,
        lsns[usize::try_from(ACKED).unwrap() - 2]
    );
}

/// A reader bounded at an unrepaired tail gives the same clean snapshot answer through both
/// public read routes. The unbounded control proves the torn bytes are present and corrupt at
/// the queried position rather than absent from the journal.
#[test]
fn reader_open_locate_and_iter_from_both_stop_at_the_detected_tail() {
    let fixture = in_flight(40);
    let (reader, report) = Reader::open(fixture.dir.path()).unwrap();
    assert_eq!(report.torn.map(|torn| torn.lsn), Some(fixture.tail_at));

    assert!(
        reader.iter_from(fixture.tail_at).unwrap().next().is_none(),
        "iteration must stop at the open-time tail bound"
    );
    assert_eq!(
        reader.locate(fixture.tail_at).unwrap(),
        Location::Missing,
        "locate must give the same bounded answer"
    );

    let err = Reader::new(fixture.dir.path())
        .locate(fixture.tail_at)
        .expect_err("control — the unbounded reader must inspect the torn bytes");
    assert!(
        matches!(err, Error::Corrupt { lsn, .. } if lsn == fixture.tail_at),
        "control — the queried tail bytes were not corrupt: {err:?}"
    );
}

/// `Reader::open` is a fixed offline snapshot: completing the frame it classified as torn does
/// not extend that reader. Opening again is the explicit way to observe the completed append.
#[test]
fn reader_open_keeps_its_detected_tail_bound_after_the_append_completes() {
    let InFlight {
        dir,
        writer,
        mut raw,
        tail_at,
        rest,
        ..
    } = in_flight(40);
    let (reader, report) = Reader::open(dir.path()).unwrap();
    assert_eq!(report.torn.map(|torn| torn.lsn), Some(tail_at));

    raw.write_all(&rest).unwrap();
    drop(raw);
    drop(writer);

    let (mut continuing, continuation_report) = open(
        dir.path(),
        JournalOptions {
            rotate_at: 1,
            ..options()
        },
    )
    .unwrap();
    assert_eq!(continuation_report.torn, None);
    assert_eq!(continuation_report.frames, ACKED + 1);
    let later = continuing.append(&record(ACKED + 2)).unwrap();
    assert!(
        later.segment() > tail_at.segment(),
        "control — the later frame must be in a segment after the bound"
    );

    let old: Vec<(Lsn, Entry)> = reader
        .iter_from(Lsn::ZERO)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        u64::try_from(old.len()).unwrap(),
        ACKED,
        "the existing reader must keep its open-time bound"
    );
    assert_eq!(
        reader.locate(later).unwrap(),
        Location::Missing,
        "the existing reader must hide a later valid frame"
    );
    assert!(
        reader.iter_from(later).unwrap().next().is_none(),
        "iteration from a later valid frame must still stop at the bound"
    );

    let (fresh, fresh_report) = Reader::open(dir.path()).unwrap();
    assert_eq!(fresh_report.torn, None);
    assert_eq!(fresh_report.frames, ACKED + 2);
    let current: Vec<(Lsn, Entry)> = fresh
        .iter_from(Lsn::ZERO)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(u64::try_from(current.len()).unwrap(), ACKED + 2);
    assert_eq!(current.last().unwrap(), &(later, record(ACKED + 2)));
}

/// An iterator owns the segment list captured when it was created. Rotation after that point is
/// visible through a fresh iterator from the reader, but it does not turn the held iterator into
/// a follower.
#[test]
fn held_iter_does_not_follow_segments_created_after_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut writer, _) = open(
        dir.path(),
        JournalOptions {
            rotate_at: 1,
            ..options()
        },
    )
    .unwrap();
    let first = writer.append(&record(1)).unwrap();
    let reader = Reader::new(dir.path());
    let held = reader.iter_from(Lsn::ZERO).unwrap();

    let second = writer.append(&record(2)).unwrap();
    assert!(
        second.segment() > first.segment(),
        "control — the second append must rotate"
    );

    let snapshot: Vec<(Lsn, Entry)> = held.collect::<Result<_, _>>().unwrap();
    assert_eq!(snapshot, vec![(first, record(1))]);

    let current: Vec<(Lsn, Entry)> = reader
        .iter_from(Lsn::ZERO)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(current, vec![(first, record(1)), (second, record(2))]);
}
