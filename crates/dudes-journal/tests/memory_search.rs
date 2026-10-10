//! Literal search pages over the real applied-memory fold.

use dudes_journal::{MemoryApply, MemoryProjection};
#[cfg(unix)]
use dudes_journal::{MemoryCondenseConfig, MemorySearchError};
use dudes_proto::rpc::memory::{MemorySearchHit, MemorySearchOptions, MemorySearchResult};
use dudes_proto::{PeerProvenance, Record};
use dudes_types::{AgentId, Lsn};

fn append(memory: &mut MemoryProjection, agent: AgentId, lsn: u64, text: &str) -> Record {
    let prepared = memory
        .prepare_append(agent, text.as_bytes(), PeerProvenance::Agent(agent))
        .unwrap();
    let record = prepared.record().clone();
    assert_eq!(
        memory.apply_record(Lsn(lsn), &record).unwrap(),
        MemoryApply::Applied
    );
    record
}

fn search(
    memory: &MemoryProjection,
    query: &str,
    since: Option<u64>,
    limit: u64,
) -> MemorySearchResult {
    memory
        .search(
            AgentId::NIL,
            &MemorySearchOptions::new(query.into(), since.map(Lsn), Some(limit)).unwrap(),
        )
        .unwrap()
}

fn raw(lsn: u64, text: &str, match_offset: usize) -> MemorySearchHit {
    MemorySearchHit::Raw {
        lsn: Lsn(lsn),
        match_offset,
        text: text.into(),
        text_offset: 0,
        source_bytes: text.len(),
        truncated: false,
        read_since_lsn: Lsn(lsn.saturating_sub(1)),
    }
}

#[test]
fn empty_literal_and_sparse_cursor_results_are_exact() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    assert_eq!(
        search(&memory, "x", None, 10),
        MemorySearchResult::default()
    );
    append(&mut memory, AgentId::NIL, 3, "Alpha needle needle");
    append(&mut memory, AgentId::NIL, 20, "");
    append(&mut memory, AgentId::NIL, 100, "needle  beta");
    let other = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
    append(&mut memory, other, 101, "needle private sibling");
    assert_eq!(
        search(&memory, "needle", None, 1),
        MemorySearchResult {
            hits: vec![raw(3, "Alpha needle needle", 6)],
            next_since_lsn: Some(Lsn(3))
        }
    );
    assert_eq!(
        search(&memory, "needle", Some(3), 1),
        MemorySearchResult {
            hits: vec![raw(100, "needle  beta", 0)],
            next_since_lsn: None
        }
    );
    assert_eq!(
        search(&memory, "Needle", None, 100),
        MemorySearchResult::default()
    );
    assert_eq!(
        search(&memory, "needle beta", None, 100),
        MemorySearchResult::default()
    );
    assert_eq!(
        search(&memory, "needle  beta", None, 100).hits,
        vec![raw(100, "needle  beta", 0)]
    );
    assert_eq!(
        search(&memory, "needle", Some(u64::MAX), 100),
        MemorySearchResult::default()
    );
    append(&mut memory, AgentId::NIL, 102, "cross");
    append(&mut memory, AgentId::NIL, 103, "entry");
    assert_eq!(
        search(&memory, "cross\nentry", None, 100),
        MemorySearchResult::default()
    );
}

#[test]
fn quarantine_and_corrupt_inline_bodies_never_become_candidates() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let withheld = Record::MemoryAppendQuarantined {
        agent: AgentId::NIL,
        payload: dudes_proto::PayloadDigest {
            decoded_len: 15,
            blake3: blake3::hash(b"withheld needle").to_hex().to_string(),
        },
        provenance: PeerProvenance::NoRecordedAgent,
        reason: "withheld needle".into(),
    };
    assert_eq!(
        memory.apply_record(Lsn(2), &withheld).unwrap(),
        MemoryApply::Ignored
    );
    let corrupt = Record::MemoryAppend {
        agent: AgentId::NIL,
        text: format!("needle {}", "x".repeat(4096)),
        body: None,
        provenance: None,
    };
    assert!(matches!(
        memory.apply_record(Lsn(3), &corrupt).unwrap(),
        MemoryApply::Corrupt(_)
    ));
    assert_eq!(
        search(&memory, "needle", None, 100),
        MemorySearchResult::default()
    );
    append(&mut memory, AgentId::NIL, 4, "valid needle");
    assert_eq!(
        search(&memory, "needle", None, 100).hits,
        vec![raw(4, "valid needle", 6)]
    );
}

#[test]
fn excerpts_include_the_entire_first_match_and_use_utf8_byte_offsets() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let text = format!("{}needle{}", "a".repeat(300), "z".repeat(2000));
    append(&mut memory, AgentId::NIL, 7, &text);
    assert_eq!(
        search(&memory, "needle", None, 10).hits,
        vec![MemorySearchHit::Raw {
            lsn: Lsn(7),
            match_offset: 300,
            text: format!("{}needle{}", "a".repeat(256), "z".repeat(762)),
            text_offset: 44,
            source_bytes: 2306,
            truncated: true,
            read_since_lsn: Lsn(6),
        }]
    );
    let text = format!("{}β{}", "🦀".repeat(100), "🦀".repeat(400));
    append(&mut memory, AgentId::NIL, 9, &text);
    assert_eq!(
        search(&memory, "β", None, 10).hits,
        vec![MemorySearchHit::Raw {
            lsn: Lsn(9),
            match_offset: 400,
            text: format!("{}β{}", "🦀".repeat(64), "🦀".repeat(191)),
            text_offset: 144,
            source_bytes: 2002,
            truncated: true,
            read_since_lsn: Lsn(8),
        }]
    );
    let query = "🦀".repeat(256);
    append(
        &mut memory,
        AgentId::NIL,
        11,
        &format!("prefix{query}suffix"),
    );
    let result = search(&memory, &query, Some(9), 10);
    assert_eq!(
        result.hits,
        vec![MemorySearchHit::Raw {
            lsn: Lsn(11),
            match_offset: 6,
            text: query,
            text_offset: 6,
            source_bytes: 1036,
            truncated: true,
            read_since_lsn: Lsn(10)
        }]
    );
}

#[test]
fn candidate_limit_advances_an_empty_nonexhausted_page() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    for lsn in 1..=257 {
        append(&mut memory, AgentId::NIL, lsn, "");
    }
    assert_eq!(
        search(&memory, "needle", None, 100),
        MemorySearchResult {
            hits: vec![],
            next_since_lsn: Some(Lsn(256))
        }
    );
    assert_eq!(
        search(&memory, "needle", Some(256), 100),
        MemorySearchResult::default()
    );
}

#[cfg(unix)]
#[test]
fn examined_byte_limit_stops_before_a_whole_candidate() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    append(&mut memory, AgentId::NIL, 3, &"x".repeat(524_288));
    append(&mut memory, AgentId::NIL, 8, &"x".repeat(524_288));
    append(&mut memory, AgentId::NIL, 10, "needle");
    assert_eq!(
        search(&memory, "needle", None, 100),
        MemorySearchResult {
            hits: vec![],
            next_since_lsn: Some(Lsn(8))
        }
    );
    assert_eq!(
        search(&memory, "needle", Some(8), 100).hits,
        vec![raw(10, "needle", 0)]
    );
}

#[cfg(unix)]
#[test]
fn a_legacy_raw_hit_does_not_promise_a_whole_entry_read() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    append(
        &mut memory,
        AgentId::NIL,
        1,
        &format!("needle{}", "x".repeat(65_531)),
    );
    assert!(matches!(
        search(&memory, "needle", None, 10).hits[0],
        MemorySearchHit::Raw {
            lsn: Lsn(1),
            source_bytes: 65_537,
            read_since_lsn: Lsn(0),
            truncated: true,
            ..
        }
    ));
    assert_eq!(memory.read(AgentId::NIL, None, 65_536), Err(65_537));
}

#[cfg(unix)]
#[test]
fn legacy_obstruction_names_the_position_and_can_only_be_explicitly_skipped() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    append(&mut memory, AgentId::NIL, 5, &"x".repeat(1_048_577));
    append(&mut memory, AgentId::NIL, 20, "needle");
    let options = MemorySearchOptions::new("needle".into(), None, None).unwrap();
    assert!(matches!(
        memory.search(AgentId::NIL, &options),
        Err(MemorySearchError::EntryTooLarge {
            lsn: Lsn(5),
            bytes: 1_048_577
        })
    ));
    assert_eq!(
        search(&memory, "needle", Some(5), 10).hits,
        vec![raw(20, "needle", 0)]
    );
    assert_eq!(memory.read(AgentId::NIL, None, 65_536), Err(1_048_577));
}

#[test]
fn escaped_result_bounds_leave_the_unconsumed_hit_for_the_next_page() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let text = "\u{1}".repeat(1024);
    for lsn in 1..=25 {
        append(&mut memory, AgentId::NIL, lsn, &text);
    }
    let first = search(&memory, "\u{1}", None, 100);
    assert_eq!(first.hits.len(), 10);
    assert_eq!(first.next_since_lsn, Some(Lsn(10)));
    assert!(serde_json::to_vec(&first).unwrap().len() <= 65_536);
    let second = search(&memory, "\u{1}", Some(10), 100);
    assert_eq!(second.hits[0], raw(11, &text, 0));
    assert_eq!(second.next_since_lsn, Some(Lsn(20)));
    let third = search(&memory, "\u{1}", Some(20), 100);
    assert_eq!(third.hits.len(), 5);
    assert_eq!(third.next_since_lsn, None);
    let positions: Vec<_> = first
        .hits
        .into_iter()
        .chain(second.hits)
        .chain(third.hits)
        .map(|hit| match hit {
            MemorySearchHit::Raw { lsn, .. } => lsn,
            MemorySearchHit::Summary { .. } => panic!("raw corpus"),
        })
        .collect();
    assert_eq!(positions, (1..=25).map(Lsn).collect::<Vec<_>>());
}

#[test]
fn the_result_cap_accounts_for_cursor_width_and_the_final_null_transition() {
    for base in [0, u64::MAX - 20] {
        let root = tempfile::tempdir().unwrap();
        let mut memory = MemoryProjection::new(root.path()).unwrap();
        let text = "\u{1}".repeat(1024);
        let mut expected = MemorySearchResult::default();
        for row in 1..=10 {
            append(&mut memory, AgentId::NIL, base + row, &text);
            expected.hits.push(raw(base + row, &text, 0));
        }
        let mut tail = "\u{1}".to_owned();
        expected.hits.push(raw(base + 11, &tail, 0));
        loop {
            let bytes = serde_json::to_vec(&expected).unwrap().len();
            if bytes == 65_536 {
                break;
            }
            assert!(bytes < 65_536 && tail.len() < 1024);
            tail.push(if bytes + 6 < 65_536 { '\u{1}' } else { 'x' });
            expected.hits[10] = raw(base + 11, &tail, 0);
        }
        append(&mut memory, AgentId::NIL, base + 11, &tail);
        assert!(
            search(&memory, "\u{1}", None, 100) == expected,
            "exact-cap exhausted page at base {base}"
        );
        append(&mut memory, AgentId::NIL, base + 12, "no match");
        let first = search(&memory, "\u{1}", None, 100);
        assert!(serde_json::to_vec(&first).unwrap().len() <= 65_536);
        if base == 0 {
            assert!(first == expected, "a two-digit cursor is smaller than null");
        } else {
            assert_eq!(first.hits, expected.hits[..10]);
            assert_eq!(first.next_since_lsn, Some(Lsn(base + 10)));
            assert_eq!(
                search(&memory, "\u{1}", Some(base + 10), 100),
                MemorySearchResult {
                    hits: vec![raw(base + 11, &tail, 0)],
                    next_since_lsn: None
                }
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn covered_raw_notes_and_only_the_current_summary_search_and_replay_identically() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let mut records = Vec::new();
    for lsn in 3..=22 {
        let text = format!("needle note {lsn} {}", "detail ".repeat(200));
        records.push((Lsn(lsn), append(&mut memory, AgentId::NIL, lsn, &text)));
    }
    let config = MemoryCondenseConfig::new(1, 8).unwrap();
    let plan = memory.plan_condense(AgentId::NIL, config).unwrap().unwrap();
    let prepared = memory
        .prepare_condense_with_model(plan, "needle prior summary".into(), None)
        .unwrap();
    let record = prepared.record().clone();
    assert_eq!(
        memory.apply_record(Lsn(50), &record).unwrap(),
        MemoryApply::Applied
    );
    records.push((Lsn(50), record));
    let first = search(&memory, "needle", None, 100);
    assert_eq!(first.hits.len(), 21);
    assert!(matches!(
        first.hits.last().unwrap(),
        MemorySearchHit::Summary {
            lsn: Lsn(50),
            read_since_lsn: Some(Lsn(2)),
            covered_from_lsn: Lsn(3),
            covered_to_lsn: Lsn(14),
            text_offset: 0,
            truncated: false,
            ..
        }
    ));
    assert!(matches!(
        first.hits[0],
        MemorySearchHit::Raw { lsn: Lsn(3), .. }
    ));
    let page = search(&memory, "needle", None, 10);
    assert_eq!(page.next_since_lsn, Some(Lsn(12)));
    for lsn in 51..=70 {
        let text = format!("needle late {lsn} {}", "detail ".repeat(200));
        records.push((Lsn(lsn), append(&mut memory, AgentId::NIL, lsn, &text)));
    }
    let plan = memory.plan_condense(AgentId::NIL, config).unwrap().unwrap();
    let prepared = memory
        .prepare_condense_with_model(plan, "needle current summary".into(), None)
        .unwrap();
    let record = prepared.record().clone();
    assert_eq!(
        memory.apply_record(Lsn(100), &record).unwrap(),
        MemoryApply::Applied
    );
    records.push((Lsn(100), record));
    let next = search(&memory, "needle", Some(12), 100);
    assert!(
        next.hits
            .iter()
            .all(|hit| !matches!(hit, MemorySearchHit::Summary { lsn: Lsn(50), .. }))
    );
    assert!(matches!(
        next.hits.last().unwrap(),
        MemorySearchHit::Summary { lsn: Lsn(100), .. }
    ));
    let before = (
        memory.export(AgentId::NIL).unwrap(),
        memory.digest(AgentId::NIL),
        memory.work_digest(AgentId::NIL),
    );
    let expected = search(&memory, "needle", None, 100);
    assert_eq!(
        before,
        (
            memory.export(AgentId::NIL).unwrap(),
            memory.digest(AgentId::NIL),
            memory.work_digest(AgentId::NIL)
        )
    );
    let mut replay = MemoryProjection::new(root.path()).unwrap();
    for (lsn, record) in &records {
        replay.apply_record(*lsn, record).unwrap();
    }
    assert_eq!(search(&replay, "needle", None, 100), expected);
    assert_eq!(
        search(&replay, "needle", None, 10),
        search(&memory, "needle", None, 10)
    );
    assert_eq!(
        before,
        (
            replay.export(AgentId::NIL).unwrap(),
            replay.digest(AgentId::NIL),
            replay.work_digest(AgentId::NIL)
        )
    );
}

fn exact_short_cursor_page(memory: &mut MemoryProjection) -> MemorySearchResult {
    let text = "\u{1}".repeat(1024);
    let mut expected = MemorySearchResult {
        hits: Vec::new(),
        next_since_lsn: Some(Lsn(11)),
    };
    for lsn in 1..=10 {
        append(memory, AgentId::NIL, lsn, &text);
        expected.hits.push(raw(lsn, &text, 0));
    }
    let mut tail = "\u{1}".to_owned();
    expected.hits.push(raw(11, &tail, 0));
    loop {
        let bytes = serde_json::to_vec(&expected).unwrap().len();
        if bytes == 65_536 {
            break;
        }
        assert!(bytes < 65_536 && tail.len() < 1024);
        tail.push(if bytes + 6 < 65_536 { '\u{1}' } else { 'x' });
        expected.hits[10] = raw(11, &tail, 0);
    }
    append(memory, AgentId::NIL, 11, &tail);
    expected
}

#[test]
fn later_nonmatches_cannot_grow_a_full_results_cursor_past_the_byte_bound() {
    for wide in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut memory = MemoryProjection::new(root.path()).unwrap();
        let expected = exact_short_cursor_page(&mut memory);
        if wide {
            for lsn in (u64::MAX - 256)..=u64::MAX {
                append(&mut memory, AgentId::NIL, lsn, "");
            }
        } else {
            append(&mut memory, AgentId::NIL, 12, "");
        }
        let actual = search(&memory, "\u{1}", None, 100);
        let bytes = serde_json::to_vec(&actual).unwrap().len();
        assert!(
            bytes <= 65_536,
            "wide={wide}; serialized page has {bytes} bytes"
        );
        assert!(
            actual == expected,
            "stop before inspecting a candidate whose continuation will not fit"
        );
        let next = search(&memory, "\u{1}", Some(11), 100);
        assert!(next.hits.is_empty());
        assert_eq!(
            next.next_since_lsn,
            if wide { Some(Lsn(u64::MAX - 1)) } else { None }
        );
        if wide {
            assert_eq!(
                search(&memory, "\u{1}", Some(u64::MAX - 1), 100),
                MemorySearchResult::default()
            );
        }
    }
}
