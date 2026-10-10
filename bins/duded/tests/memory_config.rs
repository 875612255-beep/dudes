//! Startup memory settings share the operator's validated configuration snapshot.

use duded::config::load_operator_and_memory;

#[test]
fn combined_loader_reads_defaults_overrides_and_rejects_invalid_memory_values() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let (_, memory) = load_operator_and_memory(&path).unwrap();
    assert_eq!(
        (memory.threshold_bytes(), memory.keep_sections()),
        (65_536, 8)
    );
    std::fs::write(
        &path,
        "[memory]\ncondense_after_kib = 2\ncondense_keep_sections = 3\n",
    )
    .unwrap();
    let (_, memory) = load_operator_and_memory(&path).unwrap();
    assert_eq!(
        (memory.threshold_bytes(), memory.keep_sections()),
        (2_048, 3)
    );
    for body in [
        "[memory]\ncondense_after_kib = 0\n",
        "[memory]\ncondense_keep_sections = 0\n",
        "[memory]\ncondense_after_kib = 1025\n",
        "[memory]\ncondense_keep_sections = 129\n",
        "[memory]\nunknown = 9\n",
    ] {
        std::fs::write(&path, body).unwrap();
        assert!(load_operator_and_memory(&path).is_err());
    }
    assert_eq!(
        (memory.threshold_bytes(), memory.keep_sections()),
        (2_048, 3),
        "loaded typed settings do not reread later file edits"
    );
}
