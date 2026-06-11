//! Optional `?.` chains defaulted by `??` over unkeyed groups, and `nextId`
//! identity allocation including the roots it refuses.

#[macro_use]
mod support;

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

#[test]
fn an_unguarded_optional_chain_that_ends_absent_is_rejected() {
    checker_rejects(
        "resource Patient\n    name\n        first: string\n        last: string\nstore ^patients(id: int): Patient\n\npub fn first_name(id: int): string\n    return ^patients(id)?.name?.first\n",
        "check.bare_maybe_present_read",
    );
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

/// `nextId` over a composite-identity root faults with `write.next_id_unsupported`
/// rather than inventing a bogus `Int(1)`: composite identities have no default
/// allocation policy.
#[test]
fn next_id_over_a_composite_root_faults() {
    checker_rejects(
        "resource Enrollment\n    required grade: string\nstore ^enrollments(studentId: int, courseId: int): Enrollment\n\npub fn fresh(): int\n    return nextId(^enrollments)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` over a keyless singleton root faults: a singleton has no generated
/// identity to allocate.
#[test]
fn next_id_over_a_singleton_root_faults() {
    checker_rejects(
        "resource Settings\n    required theme: string\nstore ^settings: Settings\n\npub fn fresh(): int\n    return nextId(^settings)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` over a single non-integer (string) identity key faults: only an
/// `int` identity has the default policy.
#[test]
fn next_id_over_a_string_keyed_root_faults() {
    checker_rejects(
        "resource Tag\n    required name: string\nstore ^tags(slug: string): Tag\n\npub fn fresh(): int\n    return nextId(^tags)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` of a saved root no store declares is a `run.unsupported`: there is
/// no schema to decide an allocation policy (mirrors `eval_append`'s unknown-root
/// path).
#[test]
fn next_id_over_an_undeclared_root_is_unsupported() {
    checker_rejects(
        &format!("{BOOK_PRIMARY_SCHEMA}pub fn fresh(): int\n    return nextId(^bogus)\n"),
        "check.untyped_value",
    );
}
