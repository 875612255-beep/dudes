//! Closed-range memory summaries, raw read retention, and bounded recall.

use dudes_journal::{MemoryApply, MemoryCondenseConfig, MemoryCondenseError, MemoryProjection};
use dudes_proto::{PeerProvenance, Record};
use dudes_types::{AgentId, Lsn};

fn append(lsn: u64, text: &str, projection: &mut MemoryProjection) -> Record {
    let record = Record::MemoryAppend {
        agent: AgentId::NIL,
        text: text.to_owned(),
        body: None,
        provenance: Some(PeerProvenance::Agent(AgentId::NIL)),
    };
    assert_eq!(
        projection.apply_record(Lsn(lsn), &record).unwrap(),
        MemoryApply::Applied
    );
    record
}

fn history(projection: &mut MemoryProjection, count: u64) -> Vec<(Lsn, Record)> {
    (1..=count)
        .map(|lsn| {
            let text = format!(
                "Decision {lsn}: use typed argv\n{}\nCause: paths contain spaces",
                "detail ".repeat(280)
            );
            (Lsn(lsn), append(lsn, &text, projection))
        })
        .collect()
}

fn write_and_fold_memory(
    writer: &mut dudes_journal::Writer,
    memory: &mut MemoryProjection,
    record: Record,
) -> Lsn {
    let entry = dudes_proto::Entry {
        at: dudes_types::Timestamp::UNIX_EPOCH,
        record,
    };
    let lsn = writer.append(&entry).unwrap();
    memory.apply_entry(lsn, &entry).unwrap();
    lsn
}

#[test]
fn a_bare_admission_consumes_the_allowance_live_and_from_zero_replay() {
    for allowed in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let journal = root.path().join("journal");
        let (mut writer, _) =
            dudes_journal::open(&journal, dudes_journal::JournalOptions::default()).unwrap();
        let mut live = MemoryProjection::new(root.path()).unwrap();
        write_and_fold_memory(
            &mut writer,
            &mut live,
            Record::MemoryAppend {
                agent: AgentId::NIL,
                text: "ORIGINAL 🦀".to_owned(),
                body: None,
                provenance: Some(PeerProvenance::Agent(AgentId::NIL)),
            },
        );
        let before = (
            live.digest(AgentId::NIL),
            live.work_digest(AgentId::NIL),
            live.read(AgentId::NIL, None, 8192).unwrap(),
        );
        assert!(!live.model_condense_already_admitted_this_turn(AgentId::NIL));
        write_and_fold_memory(
            &mut writer,
            &mut live,
            Record::MemoryCondenseModelAdmitted {
                agent: AgentId::NIL,
                allowed,
                ceiling: None,
                model: "fixture".to_owned(),
            },
        );
        assert!(
            live.model_condense_already_admitted_this_turn(AgentId::NIL),
            "a bare durable admission consumes even without a summary or turn"
        );
        drop(writer);
        let mut replay = MemoryProjection::new(root.path()).unwrap();
        for item in dudes_journal::Reader::new(&journal)
            .iter_from(Lsn::ZERO)
            .unwrap()
        {
            let (lsn, entry) = item.unwrap();
            replay.apply_entry(lsn, &entry).unwrap();
        }
        assert!(replay.model_condense_already_admitted_this_turn(AgentId::NIL));
        assert_eq!(
            (
                replay.digest(AgentId::NIL),
                replay.work_digest(AgentId::NIL),
                replay.read(AgentId::NIL, None, 8192).unwrap()
            ),
            before
        );
        let (mut writer, _) =
            dudes_journal::open(&journal, dudes_journal::JournalOptions::default()).unwrap();
        write_and_fold_memory(
            &mut writer,
            &mut replay,
            Record::GoalCleared {
                agent: AgentId::NIL,
                goal: dudes_types::GoalId::NIL,
            },
        );
        assert!(replay.model_condense_already_admitted_this_turn(AgentId::NIL));
        write_and_fold_memory(
            &mut writer,
            &mut replay,
            Record::TurnIssued {
                agent: AgentId::NIL,
                turn: dudes_types::TurnId::NIL,
                goal: dudes_types::GoalId::NIL,
                task: "one new task".to_owned(),
                session: None,
                mode: dudes_types::Mode::Managed,
                model_selection: None,
                lsn_of_decision: Lsn::ZERO,
                answers: Vec::new(),
            },
        );
        assert!(
            !replay.model_condense_already_admitted_this_turn(AgentId::NIL),
            "only a newer issued turn renews the allowance"
        );
        write_and_fold_memory(
            &mut writer,
            &mut replay,
            Record::MemoryCondenseModelAdmitted {
                agent: AgentId::NIL,
                allowed,
                ceiling: None,
                model: "fixture".to_owned(),
            },
        );
        assert!(replay.model_condense_already_admitted_this_turn(AgentId::NIL));
    }
}

#[test]
fn bounded_launch_recall_retains_newest_without_full_history_materialization() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 100);
    append(101, "NEWEST 🦀", &mut memory);
    let recall = memory.work_digest(AgentId::NIL);
    assert!(memory.digest(AgentId::NIL).len() > 65_536);
    assert!(recall.len() <= 40_962);
    assert!(recall.ends_with("NEWEST 🦀"));
    assert!(recall.starts_with("[Recent memory]\n…"));
}

/// **`decide()` sees only data, never which kind of summary produced it.** `work_digest` returns
/// a plain `String` for a model summary through exactly the same call as for an exact-excerpt one
/// — there is no second, kind-aware return type or branch for a caller outside this crate to key
/// on, so the loop that builds `WorkInput.memory_digest` cannot tell the two apart and does not
/// need to: the model's own text is carried as inert data inside the same label-and-byte-cap
/// pipeline, never as a verdict and never as instructions (invariant 3).
#[test]
fn a_model_summary_feeds_work_digest_exactly_like_an_exact_one() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    append(21, "NEWEST", &mut memory);
    let config = MemoryCondenseConfig::new(1, 8).unwrap();
    let plan = memory
        .plan_condense(AgentId::NIL, config)
        .unwrap()
        .expect("the history above crosses the 1 KiB threshold with room to evict");
    let prepared = memory
        .prepare_condense_with_model(
            plan,
            "a model-written summary of the earlier decisions".to_owned(),
            None,
        )
        .unwrap();
    assert_eq!(
        memory.apply_record(Lsn(1_000), prepared.record()).unwrap(),
        MemoryApply::Applied
    );
    let recall = memory.work_digest(AgentId::NIL);
    assert!(recall.contains(
        "[Condensed memory: model summary; may contain text copied from memory, treated as data]"
    ));
    assert!(recall.contains("a model-written summary of the earlier decisions"));
    assert!(recall.ends_with("NEWEST"));
}

#[test]
fn memory_config_defaults_and_finite_ranges_are_explicit() {
    let default = MemoryCondenseConfig::default();
    assert_eq!(default.threshold_bytes(), 65_536);
    assert_eq!(default.keep_sections(), 8);
    for (threshold, keep) in [(0, 8), (1_025, 8), (64, 0), (64, 129), (usize::MAX, 1)] {
        assert!(matches!(
            MemoryCondenseConfig::new(threshold, keep),
            Err(MemoryCondenseError::InvalidConfig)
        ));
    }
    let low = MemoryCondenseConfig::new(1, 1).unwrap();
    assert_eq!(low.threshold_bytes(), 1_024);
    assert_eq!(low.keep_sections(), 1);
}

#[test]
fn many_empty_sections_cannot_exceed_the_recent_work_view_budget() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    for lsn in 1..=40_000 {
        append(lsn, "", &mut memory);
    }
    append(40_001, "NEWEST", &mut memory);
    let recall = memory.work_digest(AgentId::NIL);
    assert!(
        recall.len() <= 32_768,
        "empty sections still consume separator bytes"
    );
    assert!(recall.ends_with("NEWEST"));
    assert_eq!(
        memory
            .read(AgentId::NIL, Some(Lsn(39_999)), 65_536)
            .unwrap(),
        ("\nNEWEST".to_owned(), Lsn(40_001))
    );
}

#[cfg(unix)]
#[test]
fn prepared_summary_revalidates_every_named_ancestor_before_admission() {
    for ancestor in ["agents", "agent", "memory"] {
        let root = tempfile::tempdir().unwrap();
        let mut memory = MemoryProjection::new(root.path()).unwrap();
        history(&mut memory, 20);
        let plan = memory
            .plan_condense(AgentId::NIL, MemoryCondenseConfig::new(1, 8).unwrap())
            .unwrap()
            .unwrap();
        let prepared = memory.prepare_condense(plan).unwrap();
        let path = match ancestor {
            "agents" => root.path().join("agents"),
            "agent" => root.path().join("agents").join(AgentId::NIL.to_string()),
            _ => root
                .path()
                .join("agents")
                .join(AgentId::NIL.to_string())
                .join("memory"),
        };
        std::fs::rename(&path, path.with_file_name(format!("{ancestor}-old"))).unwrap();
        std::fs::create_dir(&path).unwrap();
        let before = memory.digest(AgentId::NIL);
        assert!(
            prepared.revalidate().is_err(),
            "{ancestor} replacement must invalidate admission"
        );
        assert_eq!(memory.digest(AgentId::NIL), before);
    }
}

#[cfg(unix)]
#[test]
fn closed_condense_preserves_late_appends_raw_pages_and_replay_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let mut records = history(&mut memory, 20);
    let config = MemoryCondenseConfig::new(1, 8).unwrap();
    let plan = memory.plan_condense(AgentId::NIL, config).unwrap().unwrap();
    let late = append(21, "NEWEST appended after capture", &mut memory);
    records.push((Lsn(21), late));
    let raw = memory.read(AgentId::NIL, None, 65_536).unwrap();
    let before = memory.digest(AgentId::NIL);
    let prepared = memory.prepare_condense(plan).unwrap();
    prepared.revalidate().unwrap();
    let Record::MemoryCondense {
        from_lsn,
        to_lsn,
        summary_ref,
        ..
    } = prepared.record()
    else {
        panic!("condense")
    };
    assert_eq!((*from_lsn, *to_lsn), (Lsn(1), Lsn(12)));
    assert!(root.path().join(summary_ref).is_file());
    assert_eq!(
        memory.digest(AgentId::NIL),
        before,
        "prepared bytes are not active state"
    );
    let record = prepared.record().clone();
    assert_eq!(
        memory.apply_record(Lsn(100), &record).unwrap(),
        MemoryApply::Applied
    );
    prepared.revalidate().unwrap();
    records.push((Lsn(100), record.clone()));
    assert_eq!(memory.read(AgentId::NIL, None, 65_536).unwrap(), raw);
    assert!(
        memory
            .digest(AgentId::NIL)
            .ends_with("NEWEST appended after capture")
    );
    assert!(
        memory
            .work_digest(AgentId::NIL)
            .contains("[Condensed memory: exact excerpts;")
    );
    assert!(
        memory
            .work_digest(AgentId::NIL)
            .contains("NEWEST appended after capture")
    );
    let active = memory.digest(AgentId::NIL);
    assert!(active.len() < before.len());
    assert_eq!(
        memory.apply_record(Lsn(100), &record).unwrap(),
        MemoryApply::Ignored
    );
    assert_eq!(
        memory.apply_record(Lsn(101), &record).unwrap(),
        MemoryApply::Ignored
    );
    assert_eq!(memory.digest(AgentId::NIL), active);
    let mut replay = MemoryProjection::new(root.path()).unwrap();
    for (lsn, record) in &records {
        replay.apply_record(*lsn, record).unwrap();
    }
    assert_eq!(replay.digest(AgentId::NIL), active);
    assert_eq!(
        replay.export(AgentId::NIL).unwrap(),
        memory.export(AgentId::NIL).unwrap()
    );
    assert_eq!(
        replay.work_digest(AgentId::NIL),
        memory.work_digest(AgentId::NIL)
    );
    assert_eq!(
        replay.read(AgentId::NIL, Some(Lsn(5)), 65_536),
        memory.read(AgentId::NIL, Some(Lsn(5)), 65_536)
    );
    assert_eq!(
        replay.active_bytes(AgentId::NIL),
        memory.digest(AgentId::NIL).len() as u64
    );
}

#[cfg(unix)]
#[test]
fn corrupt_summary_and_stale_plan_leave_active_history_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    let config = MemoryCondenseConfig::new(1, 8).unwrap();
    let plan = memory.plan_condense(AgentId::NIL, config).unwrap().unwrap();
    let stale = plan.clone();
    let prepared = memory.prepare_condense(plan).unwrap();
    let record = prepared.record().clone();
    let Record::MemoryCondense { summary_ref, .. } = &record else {
        panic!("condense")
    };
    let path = root.path().join(summary_ref);
    let bytes = std::fs::read(&path).unwrap();
    let before = memory.digest(AgentId::NIL);
    let mut altered = bytes.clone();
    altered[0] ^= 1;
    std::fs::write(&path, &altered).unwrap();
    assert!(memory.apply_record(Lsn(100), &record).is_err());
    assert_eq!(memory.digest(AgentId::NIL), before);
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        memory.apply_record(Lsn(100), &record).unwrap(),
        MemoryApply::Applied
    );
    let active = memory.digest(AgentId::NIL);
    assert!(memory.prepare_condense(stale).is_err());
    assert_eq!(memory.digest(AgentId::NIL), active);
}

#[cfg(unix)]
#[test]
fn an_equal_size_first_summary_is_refused_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    append(1, &"x".repeat(618), &mut memory);
    append(2, &"tail".repeat(110), &mut memory);
    let before = memory.digest(AgentId::NIL);
    let plan = memory
        .plan_condense(AgentId::NIL, MemoryCondenseConfig::new(1, 1).unwrap())
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            memory.prepare_condense(plan),
            Err(MemoryCondenseError::NoUsefulSummary)
        ),
        "first equal-size summary must be refused"
    );
    assert_eq!(memory.digest(AgentId::NIL), before);
    assert!(
        !root.path().join("agents").exists(),
        "no file or record may publish a non-shrinking replacement"
    );
}

#[cfg(unix)]
#[test]
fn legacy_nul_excerpt_has_no_prefix_label_and_explicitly_strengthens_deep_delivery() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let source = format!("{}\0later", "A".repeat(10_000));
    let prepared = memory
        .prepare_append(
            AgentId::NIL,
            source.as_bytes(),
            PeerProvenance::Agent(AgentId::NIL),
        )
        .unwrap();
    memory.apply_record(Lsn(1), prepared.record()).unwrap();
    let recall = memory.work_digest(AgentId::NIL);
    assert_eq!(recall, format!("{}\0", "A".repeat(8_191)));
    assert!(
        !source[..8_192].contains('\0'),
        "the old prefix omitted the offending byte"
    );
    assert!(recall.ends_with('\0'));
    assert!(!recall.starts_with('['));
    assert_eq!(memory.read(AgentId::NIL, None, 65_536).unwrap().0, source);
}

#[cfg(unix)]
#[test]
fn noncanonical_paths_and_hardlinked_summaries_are_never_trusted() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    let plan = memory
        .plan_condense(AgentId::NIL, MemoryCondenseConfig::new(1, 8).unwrap())
        .unwrap()
        .unwrap();
    let prepared = memory.prepare_condense(plan).unwrap();
    let record = prepared.record().clone();
    let Record::MemoryCondense { summary_ref, .. } = &record else {
        panic!("condense")
    };
    let before = memory.digest(AgentId::NIL);
    std::fs::hard_link(root.path().join(summary_ref), root.path().join("alias")).unwrap();
    assert!(memory.apply_record(Lsn(100), &record).is_err());
    assert_eq!(memory.digest(AgentId::NIL), before);
    let mut wrong = record;
    let Record::MemoryCondense { summary_ref, .. } = &mut wrong else {
        unreachable!()
    };
    *summary_ref = std::path::PathBuf::from("../other.summary");
    assert!(memory.apply_record(Lsn(100), &wrong).is_err());
    assert_eq!(memory.digest(AgentId::NIL), before);
}

#[cfg(unix)]
#[test]
fn a_valid_envelope_under_a_nonmatching_digest_name_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    let prepared = memory
        .prepare_condense(
            memory
                .plan_condense(AgentId::NIL, MemoryCondenseConfig::new(1, 8).unwrap())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    let mut record = prepared.record().clone();
    let Record::MemoryCondense { summary_ref, .. } = &mut record else {
        panic!("condense")
    };
    let bytes = std::fs::read(root.path().join(&*summary_ref)).unwrap();
    let bad = summary_ref.with_file_name(format!("{}.{}.summary", "0".repeat(64), bytes.len()));
    assert_ne!(&bad, summary_ref);
    std::fs::write(root.path().join(&bad), bytes).unwrap();
    *summary_ref = bad;
    let before = memory.digest(AgentId::NIL);
    assert!(matches!(
        memory.apply_record(Lsn(100), &record),
        Err(MemoryCondenseError::Corrupt(
            dudes_journal::MemoryCorruption::DigestMismatch
        ))
    ));
    assert_eq!(memory.digest(AgentId::NIL), before);
}

#[cfg(unix)]
#[test]
fn normalized_alias_spellings_do_not_satisfy_the_canonical_reference_grammar() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    let prepared = memory
        .prepare_condense(
            memory
                .plan_condense(AgentId::NIL, MemoryCondenseConfig::new(1, 8).unwrap())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    let mut record = prepared.record().clone();
    let Record::MemoryCondense { summary_ref, .. } = &mut record else {
        panic!("condense")
    };
    *summary_ref = summary_ref
        .parent()
        .unwrap()
        .join(".")
        .join(summary_ref.file_name().unwrap());
    let before = memory.digest(AgentId::NIL);
    assert!(memory.apply_record(Lsn(100), &record).is_err());
    assert_eq!(memory.digest(AgentId::NIL), before);
}

#[cfg(unix)]
#[test]
fn a_late_model_answer_cannot_replace_a_moved_predecessor() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    let config = MemoryCondenseConfig::new(1, 8).unwrap();
    let captured = memory.plan_condense(AgentId::NIL, config).unwrap().unwrap();
    let winner = memory.prepare_condense(captured.clone()).unwrap();
    memory.apply_record(Lsn(100), winner.record()).unwrap();
    let before = (
        memory.digest(AgentId::NIL),
        memory.work_digest(AgentId::NIL),
        memory.read(AgentId::NIL, None, 65_536).unwrap(),
    );
    let late = memory.prepare_condense_with_model(captured, "late model answer".to_owned(), None);
    assert!(matches!(
        late,
        Err(MemoryCondenseError::InvalidSummary("predecessor changed"))
    ));
    assert_eq!(
        before,
        (
            memory.digest(AgentId::NIL),
            memory.work_digest(AgentId::NIL),
            memory.read(AgentId::NIL, None, 65_536).unwrap(),
        )
    );
}

#[cfg(unix)]
#[test]
fn repeated_condense_ranges_extend_once_and_keep_a_flat_predecessor_chain() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    let mut records = history(&mut memory, 20);
    let config = MemoryCondenseConfig::new(1, 8).unwrap();
    let first = memory
        .prepare_condense(memory.plan_condense(AgentId::NIL, config).unwrap().unwrap())
        .unwrap();
    memory.apply_record(Lsn(100), first.record()).unwrap();
    records.push((Lsn(100), first.record().clone()));
    records.push((Lsn(101), append(101, "NEWEST", &mut memory)));
    let second = memory
        .prepare_condense(memory.plan_condense(AgentId::NIL, config).unwrap().unwrap())
        .unwrap();
    let Record::MemoryCondense {
        from_lsn, to_lsn, ..
    } = second.record()
    else {
        panic!("condense")
    };
    assert_eq!((*from_lsn, *to_lsn), (Lsn(13), Lsn(13)));
    memory.apply_record(Lsn(102), second.record()).unwrap();
    records.push((Lsn(102), second.record().clone()));
    let active = memory.digest(AgentId::NIL);
    assert_eq!(
        active.matches("[Condensed memory: exact excerpts;").count(),
        1
    );
    assert!(active.ends_with("NEWEST"));
    let mut replay = MemoryProjection::new(root.path()).unwrap();
    for (lsn, record) in records {
        replay.apply_record(lsn, &record).unwrap();
    }
    assert_eq!(
        replay.export(AgentId::NIL).unwrap(),
        memory.export(AgentId::NIL).unwrap()
    );
    let mut missing_predecessor = MemoryProjection::new(root.path()).unwrap();
    history(&mut missing_predecessor, 20);
    append(101, "NEWEST", &mut missing_predecessor);
    assert!(
        missing_predecessor
            .apply_record(Lsn(102), second.record())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn capture_limits_sections_and_counts_the_prior_envelope_in_its_byte_budget() {
    for body_bytes in [1_024, 4_096] {
        let root = tempfile::tempdir().unwrap();
        let mut memory = MemoryProjection::new(root.path()).unwrap();
        for lsn in 1..=300 {
            append(lsn, &"x".repeat(body_bytes), &mut memory);
        }
        append(301, "NEWEST", &mut memory);
        let config = MemoryCondenseConfig::new(1, 8).unwrap();
        let first = memory
            .prepare_condense(memory.plan_condense(AgentId::NIL, config).unwrap().unwrap())
            .unwrap();
        let Record::MemoryCondense {
            from_lsn,
            to_lsn,
            summary_ref,
            ..
        } = first.record()
        else {
            panic!("condense")
        };
        assert_eq!((*from_lsn, *to_lsn), (Lsn(1), Lsn(128)));
        let prior_bytes = usize::try_from(
            std::fs::metadata(root.path().join(summary_ref))
                .unwrap()
                .len(),
        )
        .unwrap();
        memory.apply_record(Lsn(1_000), first.record()).unwrap();
        let second = memory
            .prepare_condense(memory.plan_condense(AgentId::NIL, config).unwrap().unwrap())
            .unwrap();
        let Record::MemoryCondense {
            from_lsn, to_lsn, ..
        } = second.record()
        else {
            panic!("condense")
        };
        let expected = 128.min((524_288 - prior_bytes) / body_bytes);
        assert_eq!(*from_lsn, Lsn(129));
        assert_eq!(*to_lsn, Lsn(128 + expected as u64));
        assert!(expected * body_bytes + prior_bytes <= 524_288);
        assert!(memory.work_digest(AgentId::NIL).ends_with("NEWEST"));
    }
}

#[cfg(unix)]
#[test]
fn fifo_summary_helper() {
    let Some(root) = std::env::var_os("DUDES_SUMMARY_FIFO_ROOT") else {
        return;
    };
    let record: Record =
        serde_json::from_str(&std::env::var("DUDES_SUMMARY_FIFO_RECORD").unwrap()).unwrap();
    let mut memory = MemoryProjection::new(root).unwrap();
    history(&mut memory, 20);
    assert!(matches!(
        memory.apply_record(Lsn(100), &record),
        Err(MemoryCondenseError::Corrupt(
            dudes_journal::MemoryCorruption::NonRegularBody
        ))
    ));
}

#[cfg(unix)]
#[test]
fn summary_fifo_without_a_writer_is_refused_within_a_finite_bound() {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut memory = MemoryProjection::new(root.path()).unwrap();
    history(&mut memory, 20);
    let plan = memory
        .plan_condense(AgentId::NIL, MemoryCondenseConfig::new(1, 8).unwrap())
        .unwrap()
        .unwrap();
    let prepared = memory.prepare_condense(plan).unwrap();
    let record = prepared.record().clone();
    let Record::MemoryCondense { summary_ref, .. } = &record else {
        panic!("condense")
    };
    let path = root.path().join(summary_ref);
    std::fs::remove_file(&path).unwrap();
    #[cfg(not(target_vendor = "apple"))]
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    #[cfg(target_vendor = "apple")]
    assert!(
        Command::new("/usr/bin/mkfifo")
            .args(["-m", "600"])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fifo_summary_helper", "--quiet"])
            .env_clear()
            .env("DUDES_SUMMARY_FIFO_ROOT", root.path())
            .env(
                "DUDES_SUMMARY_FIFO_RECORD",
                serde_json::to_string(&record).unwrap(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.is_some_and(|status| status.success()),
        "a no-writer FIFO must refuse before blocking"
    );
}
