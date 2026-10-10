#![allow(missing_docs)]

use std::fs;

use dudes_journal::frame::Header;
use dudes_journal::segment::segment_path;
use dudes_journal::{Fsync, JournalOptions, open};
use dudes_proto::Entry;

fn queued() -> Entry {
    serde_json::from_value(serde_json::json!({
        "at": 0,
        "t": "browser_note_queued",
        "agent": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "text": "run the tests",
        "browser": {
            "request_id_digest": "11".repeat(32),
            "request_digest": "22".repeat(32),
            "session_digest": "33".repeat(32)
        },
        "destination": {"kind": "attached_context"}
    }))
    .expect("the required browser-note payload is understood")
}

#[test]
fn a_chat_record_uses_required_framing_even_via_ordinary_append() {
    let dir = tempfile::tempdir().unwrap();
    let (mut writer, _) = open(
        dir.path(),
        JournalOptions {
            fsync: Fsync::None,
            checkpoint_every: 0,
            ..JournalOptions::default()
        },
    )
    .unwrap();
    let lsn = writer.append(&queued()).unwrap();
    drop(writer);
    let bytes = fs::read(segment_path(dir.path(), lsn.segment())).unwrap();
    let header = Header::parse(bytes[..12].try_into().unwrap());
    assert_eq!(header.ver, 2, "Chat cannot be skipped by an old v1 reader");
    assert_eq!(header.kind, 47);
}
