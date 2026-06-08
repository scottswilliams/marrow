//! Saved-path lowering corners: identity splice versus raw keys, keyed-root
//! arity, the unkeyed-group hop versus keyed-layer distinction, and the
//! index-branch terminal place classification.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

/// record a bare int key does.
#[test]
fn an_identity_argument_splices_in_as_the_record_key() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn save()\n    const id = nextId(^books)\n    ^books(id).title = \"a\"\n\n\
         pub fn read(): string\n    return ^books(1).title\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Str("a".into()))
    );
}

#[test]
fn a_wrong_typed_key_faults_at_lowering_and_does_not_write() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn save(bad: string)\n    ^books(bad).title = \"a\"\n",
        "check.key_type",
    );
}

/// A single-key identity from a string-keyed store cannot be spliced into an
/// int-keyed root; lowering rejects the scalar mismatch before writing.
#[test]
fn a_wrong_scalar_spliced_identity_faults_and_does_not_write() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         resource Magazine at ^magazines(issn: string)\n    required title: string\n\n\
         pub fn seed()\n    ^magazines(\"issn\").title = \"m\"\n\n\
         pub fn save()\n    for id in ^magazines\n        ^books(id).title = \"a\"\n",
        "check.key_type",
    );
}

/// A single-key identity produced from the target store still writes through the
/// saved path lowering.
#[test]
fn a_single_key_store_identity_splice_still_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn seed()\n    ^books(7).title = \"seed\"\n\n\
         pub fn save()\n    for id in ^books\n        ^books(id).title = \"a\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::save"))
        .expect("store identity splice writes");
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(7)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("a".into()))
    );
}

/// A composite identity cannot be one component among raw keys: `^pairs(id, 5)`
/// mixing the spliced identity with a trailing raw key is rejected as unsupported.
#[test]
fn an_identity_mixed_with_a_raw_key_is_rejected() {
    checker_rejects(
        "resource Pair at ^pairs(a: int, b: int)\n    required title: string\n\n\
         pub fn seed()\n    ^pairs(7, 8).title = \"seed\"\n\n\
         pub fn save()\n    for id in ^pairs\n        ^pairs(id, 5).title = \"a\"\n",
        "check.key_type",
    );
}

/// Addressing a keyed root without an identity is a type error naming the
/// expected key count, not a silent read of the keyless path.
#[test]
fn a_keyed_root_without_an_identity_is_a_type_error() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn read(): string\n    return ^books.title\n",
        "check.untyped_value",
    );
}

/// An unkeyed group hop (`^patients(id).name.first`) lowers `name` as a zero-key
/// group layer, landing the field under a `ChildLayer`, not as a top-level field.
#[test]
fn an_unkeyed_group_hop_lowers_to_a_child_layer() {
    let program = checked_program(
        "resource Patient at ^patients(id: int)\n    mrn: string\n    name\n        first: string\n\n\
         pub fn save()\n    ^patients(1).name.first = \"Sam\"\n\n\
         pub fn read(): string\n    return ^patients(1)?.name?.first ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Str("Sam".into()))
    );
    // The field landed under the `name` group layer, not beside `mrn`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "patients",
            &[SavedKey::Int(1)],
            &data_path(&program, "patients", &["name", "first"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Sam".into()))
    );
}

/// rejected, the same unsupported-path classification the lowering gives it.
#[test]
fn an_index_branch_is_not_an_assignable_place() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\n\
         pub fn give(inout s: string)\n    s = \"x\"\n\n\
         pub fn run_it()\n    give(inout ^books.byShelf(\"a\"))\n",
        "check.untyped_value",
    );
}
