//! The canonical projection manifest has a length-independent public Rust type, so adding the
//! next projection table changes its value without breaking every downstream type annotation.

use dudes_journal::projections::EXPORTED_TABLES;

trait LengthIndependentManifest {}

impl LengthIndependentManifest for &'static [&'static str] {}

fn require_length_independent_manifest<T: LengthIndependentManifest>(_: T) {}

#[test]
fn exported_tables_public_type_is_length_independent() {
    // A generic argument does not apply array-to-slice coercion. This compiled witness therefore
    // fails if the public expression regresses from `&[&str]` to `[&str; N]`, whichever N it is.
    require_length_independent_manifest(EXPORTED_TABLES);
    assert!(
        EXPORTED_TABLES.contains(&"mail"),
        "the length-independent manifest still carries this member"
    );
}
