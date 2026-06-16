//! Optional `?.` chains defaulted by `??` over unkeyed groups, and `nextId`
//! identity allocation including the roots it refuses.

use crate::support;
use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

/// A `Patient` with an unkeyed `name` group, queried with a `?.` chain defaulted
/// by `??`. The chain `^patients(id)?.name?.first` reads the first name when the
/// whole record, the `name` group, and the field are all populated, and is absent
/// (caught by `??`) when any step along the way is missing.
const PATIENT_CHAIN: &str = "\
resource Patient
    name
        first: string
        last: string
store ^patients(id: int): Patient

pub fn first_name_or(id: int, fallback: string): string
    return ^patients(id)?.name?.first ?? fallback
";

fn store_with_first_name(program: &CheckedRuntimeProgram, id: i64, first: &str) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &["name", "first"]),
        SavedValue::Str(first.into()),
    );
    store
}

#[test]
fn optional_chain_with_default_reads_a_present_value() {
    let program = checked_program(PATIENT_CHAIN);
    let store = store_with_first_name(&program, 1, "Granny");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(1),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("Granny".into())));
}

#[test]
fn optional_chain_defaults_when_the_record_is_absent() {
    let program = checked_program(PATIENT_CHAIN);
    // Record 2 was never written: the whole record is absent, so the chain
    // short-circuits and `??` supplies the default.
    let store = store_with_first_name(&program, 1, "Granny");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(2),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(unknown)".into())));
}

#[test]
fn optional_chain_defaults_when_an_intermediate_field_is_absent() {
    let program = checked_program(PATIENT_CHAIN);
    // The record exists (it has a `last` name) but `name.first` does not: the
    // final hop short-circuits the chain, and `??` supplies the default.
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "patients",
        &[SavedKey::Int(1)],
        &data_path(&program, "patients", &["name", "last"]),
        SavedValue::Str("Weatherwax".into()),
    );
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(1),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(unknown)".into())));
}

/// The same `Patient` schema, but a chain whose final hop may be absent is read
/// without a `??` default. A maybe-present read that escapes its guard is a
/// checker rejection, not a runtime fault.
const UNGUARDED_PATIENT_CHAIN: &str = "\
resource Patient
    name
        first: string
        last: string
store ^patients(id: int): Patient

pub fn first_name(id: int): string
    return ^patients(id)?.name?.first
";

#[test]
fn an_unguarded_optional_chain_that_ends_absent_is_rejected() {
    checker_rejects(UNGUARDED_PATIENT_CHAIN, "check.bare_maybe_present_read");
}

#[test]
fn next_id_allocates_past_the_highest_record() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn fresh(): Id(^books)\n    return nextId(^books)\n"
    ));
    let store = empty_store();
    // Empty root: the next id is 1.
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        "books",
        &[SavedKey::Int(1)],
    );
    // Seed records 1 and 4; the next id is one past the highest.
    for id in [1, 4] {
        write_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(id)],
            &data_path(&program, "books", &["title"]),
            SavedValue::Str("t".into()),
        );
    }
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        "books",
        &[SavedKey::Int(5)],
    );
}

#[test]
fn next_id_skips_ahead_after_restore() {
    // After a restore the store may hold records far above any contiguous run.
    // `nextId` chooses one past the highest existing key, never reusing a gap.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn fresh(): Id(^books)\n    return nextId(^books)\n"
    ));
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(900)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("t".into()),
    );
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        "books",
        &[SavedKey::Int(901)],
    );
}

/// `nextId` over a composite-identity root is rejected: composite identities
/// have no default allocation policy, so no single int can be allocated.
const COMPOSITE_ROOT_NEXT_ID: &str = "\
resource Enrollment
    required grade: string
store ^enrollments(studentId: int, courseId: int): Enrollment

pub fn fresh(): int
    return nextId(^enrollments)
";

#[test]
fn next_id_over_a_composite_root_faults() {
    checker_rejects(COMPOSITE_ROOT_NEXT_ID, "check.next_id_requires_single_int");
}

/// `nextId` over a keyless singleton root is rejected: a singleton has no
/// generated identity to allocate.
const SINGLETON_ROOT_NEXT_ID: &str = "\
resource Settings
    required theme: string
store ^settings: Settings

pub fn fresh(): int
    return nextId(^settings)
";

#[test]
fn next_id_over_a_singleton_root_faults() {
    checker_rejects(SINGLETON_ROOT_NEXT_ID, "check.next_id_requires_single_int");
}

/// `nextId` over a single non-integer (string) identity key is rejected: only
/// an `int` identity has the default policy.
const STRING_KEYED_ROOT_NEXT_ID: &str = "\
resource Tag
    required name: string
store ^tags(slug: string): Tag

pub fn fresh(): int
    return nextId(^tags)
";

#[test]
fn next_id_over_a_string_keyed_root_faults() {
    checker_rejects(
        STRING_KEYED_ROOT_NEXT_ID,
        "check.next_id_requires_single_int",
    );
}

/// `nextId` of a `^root` no store declares is rejected by the checker: the root
/// has no schema or type, so the reference is an untyped value before any run.
#[test]
fn next_id_over_an_undeclared_root_is_rejected() {
    checker_rejects(
        &format!("{BOOK_PRIMARY_SCHEMA}pub fn fresh(): int\n    return nextId(^bogus)\n"),
        "check.untyped_value",
    );
}
