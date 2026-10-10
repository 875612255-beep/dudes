//! Journal-owned applied-memory projection and immutable out-of-line bodies.

use std::path::{Path, PathBuf};

use dudes_journal::{INLINE_MEMORY_BYTES, MemoryApply, MemoryProjection};
use dudes_proto::{PayloadRef, PeerProvenance, Record};
use dudes_types::{AgentId, Lsn};

fn agent() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a ULID")
}

fn projection(path: PathBuf) -> MemoryProjection {
    MemoryProjection::new(path).expect("open the memory state root")
}

fn append(text: &str, body: Option<PayloadRef>) -> Record {
    Record::MemoryAppend {
        agent: agent(),
        text: text.to_owned(),
        body,
        provenance: Some(PeerProvenance::Agent(agent())),
    }
}

fn body_ref(bytes: &[u8]) -> PayloadRef {
    let hash = blake3::hash(bytes).to_hex().to_string();
    PayloadRef {
        path: PathBuf::from("agents")
            .join(agent().to_string())
            .join("memory")
            .join(format!("{hash}.body")),
        decoded_len: bytes.len() as u64,
        blake3: hash,
    }
}

#[test]
fn all_three_presence_cases_are_distinct_and_live_replay_is_identical() {
    let root = tempfile::tempdir().expect("a body root");
    let live = projection(root.path().to_path_buf());
    let empty = live
        .prepare_append(agent(), b"", PeerProvenance::Agent(agent()))
        .expect("prepare empty inline body");
    let inline = live
        .prepare_append(
            agent(),
            b"an inline applied control",
            PeerProvenance::Agent(agent()),
        )
        .expect("prepare inline body");
    let external_text = "x".repeat(INLINE_MEMORY_BYTES + 1);
    let external = live.prepare_append(
        agent(),
        external_text.as_bytes(),
        PeerProvenance::Agent(agent()),
    );
    #[cfg(unix)]
    let external = external.expect("prepare external body");
    #[cfg(unix)]
    let external_record = external.record().clone();
    #[cfg(not(unix))]
    let external_record = {
        assert_eq!(
            external
                .expect_err("unsupported persistence is refused")
                .kind(),
            std::io::ErrorKind::Unsupported
        );
        let reference = body_ref(external_text.as_bytes());
        assert!(!root.path().join(&reference.path).exists());
        append("", Some(reference))
    };
    let external_outcome = if cfg!(unix) {
        MemoryApply::Applied
    } else {
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::UnsupportedPlatform)
    };

    assert!(
        matches!(empty.record(), Record::MemoryAppend { text, body: None, .. } if text.is_empty())
    );
    assert!(
        matches!(inline.record(), Record::MemoryAppend { text, body: None, .. } if text == "an inline applied control")
    );
    assert!(
        matches!(&external_record, Record::MemoryAppend { text, body: Some(_), .. } if text.is_empty())
    );

    let mut live = live;
    let mut replayed = projection(root.path().to_path_buf());
    for (lsn, record, expected) in [
        (Lsn(30), &external_record, external_outcome.clone()),
        (Lsn(10), empty.record(), MemoryApply::Applied),
        (Lsn(20), inline.record(), MemoryApply::Applied),
    ] {
        assert_eq!(live.apply(lsn, record), expected);
    }
    for (lsn, record, expected) in [
        (Lsn(10), empty.record(), MemoryApply::Applied),
        (Lsn(20), inline.record(), MemoryApply::Applied),
        (Lsn(30), &external_record, external_outcome),
    ] {
        assert_eq!(replayed.apply(lsn, record), expected);
    }

    assert_eq!(live.digest(agent()), replayed.digest(agent()));
    let expected = if cfg!(unix) {
        format!("an inline applied control\n{external_text}")
    } else {
        "an inline applied control".to_owned()
    };
    assert_eq!(live.digest(agent()), format!("\n{expected}"));
    assert_eq!(live.condense_input(agent(), Lsn(20), Lsn(30)), expected);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table makes every exact corruption outcome mutually visible"
)]
fn corrupt_presence_paths_and_bytes_never_enter_memory_or_condense() {
    let root = tempfile::tempdir().expect("a body root");
    let bytes = vec![b'v'; INLINE_MEMORY_BYTES + 1];
    let valid_ref = body_ref(&bytes);
    let prepared = projection(root.path().to_path_buf()).prepare_append(
        agent(),
        &bytes,
        PeerProvenance::Agent(agent()),
    );
    #[cfg(unix)]
    let prepared = prepared.expect("prepare one valid ref");
    #[cfg(unix)]
    assert!(
        matches!(prepared.record(), Record::MemoryAppend { body: Some(reference), .. } if reference == &valid_ref)
    );
    #[cfg(not(unix))]
    {
        assert_eq!(
            prepared
                .expect_err("unsupported persistence is refused")
                .kind(),
            std::io::ErrorKind::Unsupported
        );
        assert!(!root.path().join(&valid_ref.path).exists());
    }

    let mut cases = vec![
        (
            append("both populated", Some(valid_ref.clone())),
            dudes_journal::MemoryCorruption::CompetingBodies,
        ),
        (
            append(&"i".repeat(INLINE_MEMORY_BYTES + 1), None),
            dudes_journal::MemoryCorruption::InlineOverThreshold,
        ),
        (
            append(
                "",
                Some(PayloadRef {
                    path: PathBuf::from("../escape"),
                    decoded_len: valid_ref.decoded_len,
                    blake3: valid_ref.blake3.clone(),
                }),
            ),
            dudes_journal::MemoryCorruption::InvalidPath,
        ),
        (
            append(
                "",
                Some(PayloadRef {
                    path: root.path().join("absolute"),
                    decoded_len: valid_ref.decoded_len,
                    blake3: valid_ref.blake3.clone(),
                }),
            ),
            dudes_journal::MemoryCorruption::InvalidPath,
        ),
        (
            append(
                "",
                Some(PayloadRef {
                    decoded_len: INLINE_MEMORY_BYTES as u64,
                    ..valid_ref.clone()
                }),
            ),
            dudes_journal::MemoryCorruption::ReferencedAtOrUnderThreshold,
        ),
    ];
    #[cfg(unix)]
    {
        let invalid_utf8 = vec![0xff; INLINE_MEMORY_BYTES + 1];
        let invalid_utf8_ref = body_ref(&invalid_utf8);
        std::fs::write(root.path().join(&invalid_utf8_ref.path), &invalid_utf8)
            .expect("plant hash-matched invalid UTF-8");
        let digest_ref = body_ref(&vec![b'h'; INLINE_MEMORY_BYTES + 1]);
        std::fs::write(
            root.path().join(&digest_ref.path),
            vec![b'j'; INLINE_MEMORY_BYTES + 1],
        )
        .expect("plant same-length digest mismatch");
        cases.extend([
            (
                append("", Some(body_ref(&vec![b'm'; INLINE_MEMORY_BYTES + 1]))),
                dudes_journal::MemoryCorruption::MissingBody,
            ),
            (
                append(
                    "",
                    Some(PayloadRef {
                        decoded_len: valid_ref.decoded_len + 1,
                        ..valid_ref.clone()
                    }),
                ),
                dudes_journal::MemoryCorruption::LengthMismatch,
            ),
            (
                append("", Some(digest_ref)),
                dudes_journal::MemoryCorruption::DigestMismatch,
            ),
            (
                append("", Some(invalid_utf8_ref)),
                dudes_journal::MemoryCorruption::InvalidUtf8,
            ),
        ]);
    }
    #[cfg(not(unix))]
    {
        // Even a well-formed reference with matching bytes cannot bypass the missing secure backend.
        let path = root.path().join(&valid_ref.path);
        std::fs::create_dir_all(path.parent().expect("body parent")).expect("plant body directory");
        std::fs::write(path, &bytes).expect("plant matching external bytes");
        cases.push((
            append("", Some(valid_ref)),
            dudes_journal::MemoryCorruption::UnsupportedPlatform,
        ));
    }

    let mut projection = projection(root.path().to_path_buf());
    let retained = "retained inline control";
    assert_eq!(
        projection.apply(Lsn::ZERO, &append(retained, None)),
        MemoryApply::Applied
    );
    for (index, (record, expected)) in cases.iter().enumerate() {
        assert_eq!(
            projection.apply(Lsn(index as u64 + 1), record),
            MemoryApply::Corrupt(*expected),
            "case {index} reached the wrong corruption guard"
        );
    }
    assert_eq!(projection.digest(agent()), retained);
    assert_eq!(
        projection.condense_input(agent(), Lsn::ZERO, Lsn(u64::MAX)),
        retained
    );
}

#[test]
fn absent_body_key_and_explicit_null_are_distinct_wire_forms_with_inline_semantics() {
    let absent = serde_json::json!({
        "t": "memory_append",
        "agent": agent(),
        "text": "absent-key"
    });
    let explicit_null = serde_json::json!({
        "t": "memory_append",
        "agent": agent(),
        "text": "explicit-null",
        "body": null
    });
    assert!(
        absent.get("body").is_none(),
        "the key is absent on this wire arm"
    );
    assert_eq!(
        explicit_null.get("body"),
        Some(&serde_json::Value::Null),
        "the key is explicitly present with JSON null on this wire arm"
    );

    let absent: Record = serde_json::from_value(absent).expect("absent body defaults");
    let explicit_null: Record = serde_json::from_value(explicit_null).expect("null body decodes");
    assert!(matches!(absent, Record::MemoryAppend { body: None, .. }));
    assert!(matches!(
        explicit_null,
        Record::MemoryAppend { body: None, .. }
    ));

    let mut projection = projection(tempfile::tempdir().unwrap().path().to_path_buf());
    assert_eq!(projection.apply(Lsn(1), &absent), MemoryApply::Applied);
    assert_eq!(
        projection.apply(Lsn(2), &explicit_null),
        MemoryApply::Applied
    );
    assert_eq!(projection.digest(agent()), "absent-key\nexplicit-null");
}

#[test]
fn body_reads_are_structurally_bounded_to_the_claimed_length_plus_one() {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/memory.rs"))
            .expect("memory source")
            .replace("\r\n", "\n");
    let bounded = source
        .split_once("fn read_bounded(")
        .and_then(|(_, tail)| tail.split_once("#[cfg(unix)]"))
        .map(|(body, _)| body)
        .expect("the bounded body reader");
    assert!(
        bounded.contains("file.metadata()?.len() != expected_len"),
        "oversize files are refused before allocating for their contents"
    );
    assert!(
        bounded.contains(".take(expected_len.saturating_add(1))"),
        "a raced size change still cannot read more than the claimed length plus one"
    );
    assert!(
        !bounded.contains("fs::read") && !bounded.contains("read_to_end(&mut bytes)?;")
            || bounded.contains(".take(expected_len.saturating_add(1))"),
        "no unbounded whole-file read reaches body resolution"
    );

    let planted = "file.take(expected_len.saturating_add(1)).read_to_end(&mut bytes)?;";
    assert!(
        planted.contains(".take(expected_len.saturating_add(1))"),
        "positive control: the scan recognizes the required read bound"
    );
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::symlink_metadata(path)
        .expect("the artifact exists")
        .permissions()
        .mode()
        & 0o777
}

#[cfg(unix)]
#[test]
fn body_store_is_owner_only_under_umask_zero_and_renarrows_existing_artifacts() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().expect("a body root");
    let store = projection(root.path().to_path_buf());
    let bytes = vec![b'p'; INLINE_MEMORY_BYTES + 1];
    let previous = rustix::process::umask(rustix::fs::Mode::empty());
    let prepared = store.prepare_append(agent(), &bytes, PeerProvenance::Agent(agent()));
    rustix::process::umask(previous);
    let prepared = prepared.expect("prepare body");
    let Record::MemoryAppend {
        body: Some(reference),
        ..
    } = prepared.record()
    else {
        panic!("large body is referenced")
    };

    let agents = root.path().join("agents");
    let agent_dir = agents.join(agent().to_string());
    let memory = agent_dir.join("memory");
    let body = root.path().join(&reference.path);
    for dir in [&agents, &agent_dir, &memory] {
        assert_eq!(mode(dir), 0o700, "{} is owner-only", dir.display());
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(mode(&body), 0o600, "the body is owner-only");
    std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o644)).unwrap();

    store
        .prepare_append(agent(), &bytes, PeerProvenance::Agent(agent()))
        .expect("reusing a body re-establishes the host modes");
    for dir in [&agents, &agent_dir, &memory] {
        assert_eq!(mode(dir), 0o700, "{} was re-narrowed", dir.display());
    }
    assert_eq!(mode(&body), 0o600, "the body was re-narrowed");
}

#[cfg(unix)]
#[test]
fn body_store_and_projection_refuse_symlinks_nonregular_files_and_oversize_bodies() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("a body root");
    let store = projection(root.path().to_path_buf());
    let seed = vec![b's'; INLINE_MEMORY_BYTES + 1];
    store
        .prepare_append(agent(), &seed, PeerProvenance::Agent(agent()))
        .expect("positive control creates the body tree");
    let memory_dir = root
        .path()
        .join("agents")
        .join(agent().to_string())
        .join("memory");

    let stage_control = vec![b'r'; INLINE_MEMORY_BYTES + 1];
    let stage_hash = blake3::hash(&stage_control).to_hex().to_string();
    let predictable_stage = memory_dir.join(format!(".{stage_hash}.{}.stage", std::process::id()));
    let predictable_nonce_stage = memory_dir.join(format!(
        ".memory-{:016x}.stage",
        u64::from(std::process::id())
    ));
    let stage_target = root.path().join("predictable-stage-target");
    std::fs::write(&stage_target, b"unchanged").unwrap();
    symlink(&stage_target, &predictable_stage).expect("plant the old predictable stage name");
    symlink(&stage_target, &predictable_nonce_stage)
        .expect("plant a predictable nonce-only stage name");
    store
        .prepare_append(agent(), &stage_control, PeerProvenance::Agent(agent()))
        .expect("exclusive unpredictable staging ignores a planted former name");
    assert_eq!(std::fs::read(&stage_target).unwrap(), b"unchanged");

    let symlink_bytes = vec![b'l'; INLINE_MEMORY_BYTES + 1];
    let symlink_ref = body_ref(&symlink_bytes);
    let outside = root.path().join("outside");
    std::fs::write(&outside, &symlink_bytes).unwrap();
    symlink(&outside, root.path().join(&symlink_ref.path)).expect("plant final-path symlink");
    assert!(
        store
            .prepare_append(agent(), &symlink_bytes, PeerProvenance::Agent(agent()))
            .is_err(),
        "an existing final symlink is never followed"
    );
    let mut replay = projection(root.path().to_path_buf());
    assert_eq!(
        replay.apply(Lsn(1), &append("", Some(symlink_ref))),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::NonRegularBody)
    );

    let directory_bytes = vec![b'd'; INLINE_MEMORY_BYTES + 1];
    let directory_ref = body_ref(&directory_bytes);
    std::fs::create_dir(root.path().join(&directory_ref.path))
        .expect("plant directory at body path");
    assert_eq!(
        replay.apply(Lsn(2), &append("", Some(directory_ref))),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::NonRegularBody)
    );

    let expected = vec![b'o'; INLINE_MEMORY_BYTES + 1];
    let oversized_ref = body_ref(&expected);
    let mut oversized = expected;
    oversized.extend(std::iter::repeat_n(b'!', 8 * 1024 * 1024));
    std::fs::write(root.path().join(&oversized_ref.path), oversized).expect("plant oversized body");
    assert_eq!(
        replay.apply(Lsn(3), &append("", Some(oversized_ref))),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::LengthMismatch)
    );
    assert_eq!(
        replay.digest(agent()),
        "",
        "no adversarial body entered memory"
    );

    let linked_root = tempfile::tempdir().expect("a second body root");
    let outside_tree = tempfile::tempdir().expect("an outside tree");
    symlink(outside_tree.path(), linked_root.path().join("agents")).expect("plant agents symlink");
    assert!(
        projection(linked_root.path().to_path_buf())
            .prepare_append(agent(), &seed, PeerProvenance::Agent(agent()))
            .is_err(),
        "directory components are never followed through symlinks"
    );
    assert!(
        std::fs::read_dir(outside_tree.path())
            .expect("outside remains a directory")
            .next()
            .is_none(),
        "the linked outside tree was not populated"
    );
}

#[cfg(unix)]
#[test]
fn body_store_and_projection_refuse_hardlink_aliases() {
    let root = tempfile::tempdir().expect("a body root");
    let store = projection(root.path().to_path_buf());
    let bytes = vec![b'a'; INLINE_MEMORY_BYTES + 1];
    let record = store
        .prepare_append(agent(), &bytes, PeerProvenance::Agent(agent()))
        .expect("positive control creates one immutable body");
    let Record::MemoryAppend {
        body: Some(reference),
        ..
    } = &*record
    else {
        panic!("large body is referenced")
    };
    let body = root.path().join(&reference.path);
    std::fs::hard_link(&body, root.path().join("body-alias"))
        .expect("plant a second name for the body inode");

    assert!(
        record.revalidate().is_err(),
        "the retained guard detects an alias introduced before admission acknowledgement"
    );
    assert!(
        store
            .prepare_append(agent(), &bytes, PeerProvenance::Agent(agent()))
            .is_err(),
        "an aliased existing body is not reused"
    );
    let mut replay = projection(root.path().to_path_buf());
    assert_eq!(
        replay.apply(Lsn(1), &record),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::AliasedBody),
        "an aliased body never enters a replayed projection"
    );
    assert_eq!(replay.digest(agent()), "");
}

#[cfg(unix)]
#[test]
fn same_inode_equal_length_rewrite_is_refused_by_guard_live_and_replay() {
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;

    let root = tempfile::tempdir().expect("a body root");
    let mut live = projection(root.path().to_path_buf());
    let original = vec![b'o'; INLINE_MEMORY_BYTES + 1];
    let changed = vec![b'x'; original.len()];
    let prepared = live
        .prepare_append(agent(), &original, PeerProvenance::Agent(agent()))
        .expect("prepare one guarded body");
    let Record::MemoryAppend {
        body: Some(reference),
        ..
    } = prepared.record()
    else {
        panic!("large body is referenced")
    };
    let body = root.path().join(&reference.path);
    let before = std::fs::metadata(&body).expect("the original body metadata");
    let mut writable = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&body)
        .expect("open the same inode for rewrite");
    writable
        .write_all(&changed)
        .expect("rewrite equal-length bytes");
    writable.sync_all().expect("sync the hostile rewrite");
    drop(writable);
    let after = std::fs::metadata(&body).expect("the rewritten body metadata");
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(before.len(), after.len());

    assert!(
        prepared.revalidate().is_err(),
        "the retained descriptor verifies content, not metadata alone"
    );
    assert_eq!(
        live.apply(Lsn(1), prepared.record()),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::DigestMismatch),
        "the live fold refuses the rewritten bytes"
    );
    let mut replay = projection(root.path().to_path_buf());
    assert_eq!(
        replay.apply(Lsn(1), prepared.record()),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::DigestMismatch),
        "replay refuses the same rewritten bytes"
    );
}

#[cfg(unix)]
#[test]
fn projection_keeps_the_boot_opened_state_root_after_its_name_is_replaced() {
    let parent = tempfile::tempdir().expect("a state parent");
    let state = parent.path().join("state");
    let parked = parent.path().join("original-state");
    std::fs::create_dir(&state).expect("create the original state directory");
    let projection = projection(state.clone());
    std::fs::rename(&state, &parked).expect("move the boot-opened state directory");
    std::fs::create_dir(&state).expect("replace the state pathname");

    let bytes = vec![b'k'; INLINE_MEMORY_BYTES + 1];
    let record = projection
        .prepare_append(agent(), &bytes, PeerProvenance::Agent(agent()))
        .expect("the retained state root remains usable");
    let Record::MemoryAppend {
        body: Some(reference),
        ..
    } = &*record
    else {
        panic!("large body is referenced")
    };
    assert!(parked.join(&reference.path).is_file());
    assert!(
        std::fs::read_dir(&state)
            .expect("replacement root is readable")
            .next()
            .is_none(),
        "no operation reopened the replacement pathname"
    );

    let mut projection = projection;
    assert_eq!(projection.apply(Lsn(1), &record), MemoryApply::Applied);
}

#[cfg(unix)]
#[test]
fn malformed_digest_is_rejected_before_nested_symlink_traversal() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("a body root");
    let live = projection(root.path().to_path_buf());
    let seed = vec![b's'; INLINE_MEMORY_BYTES + 1];
    live.prepare_append(agent(), &seed, PeerProvenance::Agent(agent()))
        .expect("positive control creates the memory directory");
    let outside = tempfile::tempdir().expect("an outside directory");
    let bytes = vec![b'n'; INLINE_MEMORY_BYTES + 1];
    let hash = blake3::hash(&bytes).to_hex().to_string();
    std::fs::write(outside.path().join(format!("{hash}.body")), &bytes)
        .expect("plant bytes beyond the nested symlink");
    let memory = root
        .path()
        .join("agents")
        .join(agent().to_string())
        .join("memory");
    symlink(outside.path(), memory.join("nested")).expect("plant the intermediate symlink");
    let malformed = format!("nested/{hash}");
    let reference = PayloadRef {
        path: memory
            .strip_prefix(root.path())
            .unwrap()
            .join(format!("{malformed}.body")),
        decoded_len: bytes.len() as u64,
        blake3: malformed,
    };

    let mut replay = live;
    assert_eq!(
        replay.apply(Lsn(1), &append("", Some(reference))),
        MemoryApply::Corrupt(dudes_journal::MemoryCorruption::InvalidPath)
    );
}

#[cfg(unix)]
#[test]
fn replay_renarrows_widened_directories_and_body_before_exposure() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().expect("a body root");
    let live = projection(root.path().to_path_buf());
    let bytes = vec![b'w'; INLINE_MEMORY_BYTES + 1];
    let record = live
        .prepare_append(agent(), &bytes, PeerProvenance::Agent(agent()))
        .expect("prepare a body");
    let Record::MemoryAppend {
        body: Some(reference),
        ..
    } = &*record
    else {
        panic!("large body is referenced")
    };
    let agents = root.path().join("agents");
    let agent_dir = agents.join(agent().to_string());
    let memory = agent_dir.join("memory");
    let body = root.path().join(&reference.path);
    for directory in [&agents, &agent_dir, &memory] {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o644)).unwrap();

    let mut replay = projection(root.path().to_path_buf());
    assert_eq!(replay.apply(Lsn(1), &record), MemoryApply::Applied);
    for directory in [&agents, &agent_dir, &memory] {
        assert_eq!(
            mode(directory),
            0o700,
            "replay narrowed {}",
            directory.display()
        );
    }
    assert_eq!(
        mode(&body),
        0o600,
        "replay narrowed the body before exposure"
    );
}

#[test]
fn quarantine_is_a_digest_only_audit_fact_not_memory_or_condense_input() {
    let planted = "QUARANTINED-PLANTED-TEXT";
    let applied = "APPLIED-PLANTED-CONTROL";
    let mut projection = projection(tempfile::tempdir().unwrap().path().to_path_buf());
    let quarantined = Record::MemoryAppendQuarantined {
        agent: agent(),
        payload: dudes_proto::PayloadDigest {
            decoded_len: planted.len() as u64,
            blake3: blake3::hash(planted.as_bytes()).to_hex().to_string(),
        },
        provenance: PeerProvenance::NoRecordedAgent,
        reason: "not the owner".to_owned(),
    };
    assert_eq!(projection.apply(Lsn(1), &quarantined), MemoryApply::Ignored);
    assert_eq!(
        projection.apply(Lsn(2), &append(applied, None)),
        MemoryApply::Applied
    );

    assert!(!projection.digest(agent()).contains(planted));
    assert!(projection.digest(agent()).contains(applied));
    assert!(
        !projection
            .condense_input(agent(), Lsn::ZERO, Lsn(2))
            .contains(planted)
    );
    assert!(
        projection
            .condense_input(agent(), Lsn::ZERO, Lsn(2))
            .contains(applied)
    );
}

#[test]
fn memory_pages_keep_whole_entries_and_an_exclusive_cursor() {
    let root = tempfile::tempdir().unwrap();
    let mut memory = projection(root.path().to_path_buf());
    for (lsn, text) in [(3, "🦀"), (7, "line\n"), (9, ""), (12, "last")] {
        assert_eq!(
            memory.apply(Lsn(lsn), &append(text, None)),
            MemoryApply::Applied
        );
    }
    assert_eq!(
        memory.read(agent(), None, 10),
        Ok(("🦀\nline\n".into(), Lsn(7)))
    );
    assert_eq!(
        memory.read(agent(), Some(Lsn(7)), 10),
        Ok(("\nlast".into(), Lsn(12)))
    );
    assert_eq!(
        memory.read(agent(), Some(Lsn(12)), 10),
        Ok((String::new(), Lsn(0)))
    );
    assert_eq!(
        memory.read(agent(), Some(Lsn(u64::MAX)), 10),
        Ok((String::new(), Lsn(0)))
    );
    assert_eq!(
        memory.read(AgentId::NIL, None, 10),
        Ok((String::new(), Lsn(0)))
    );
    assert_eq!(memory.read(agent(), None, 3), Err(4));
}
