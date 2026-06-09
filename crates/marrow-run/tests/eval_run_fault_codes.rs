//! Genuinely runtime-reachable fault codes that a clean checked program still
//! raises at run time: integer-key-space exhaustion on `nextId`/`append`, the
//! entry-name resolution faults the CLI hits when it selects an entry by name,
//! and binding a no-value call result. Each program passes `check_project` with
//! no diagnostics; the fault only exists at run time, so the checker cannot pre-empt
//! it. The oracle is the typed `RuntimeError`/`WriteError` code (and, for the
//! catchable write faults, the bound `Error` value's code), never rendered prose.

#[macro_use]
mod support;

use support::*;

use marrow_run::{CheckedEntryCall, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

/// An `int`-keyed root whose `nextId` is reachable, plus an `int`-keyed leaf
/// sequence layer whose `append` allocates a position. Both allocate one past the
/// highest existing key, so seeding the key space at `i64::MAX` exhausts it.
const OVERFLOW_SCHEMA: &str = "\
resource Lib at ^libs(id: int)
    tags(pos: int): string

pub fn fresh(): Id(^libs)
    return nextId(^libs)

pub fn add_tag(): int
    return append(^libs(1).tags, \"t\")

pub fn fresh_caught(): string
    try
        const id = nextId(^libs)
        return \"uncaught\"
    catch e: Error
        return e.code

pub fn add_tag_caught(): string
    try
        const pos = append(^libs(1).tags, \"t\")
        return \"uncaught\"
    catch e: Error
        return e.code
";

#[test]
fn next_id_past_the_max_int_key_overflows() {
    // `nextId` over a single-`int` root seeded at `i64::MAX` has no successor key:
    // the integer key space is exhausted, so it faults with `write.id_overflow`
    // rather than wrapping or reusing a key.
    let program = checked_program(OVERFLOW_SCHEMA);
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "libs",
        &[SavedKey::Int(i64::MAX)],
        &keyed_data_path(&program, "libs", &[("tags", vec![SavedKey::Int(1)])], &[]),
        SavedValue::Str("seed".into()),
    );
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::fresh")),
        "write.id_overflow",
    );
}

#[test]
fn append_past_the_max_int_position_overflows() {
    // `append` to an `int`-keyed leaf-sequence layer allocates one past the highest
    // existing position. A layer seeded at `i64::MAX` exhausts the position space,
    // so the append faults with `write.id_overflow` and writes nothing.
    let program = checked_program(OVERFLOW_SCHEMA);
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "libs",
        &[SavedKey::Int(1)],
        &keyed_data_path(
            &program,
            "libs",
            &[("tags", vec![SavedKey::Int(i64::MAX)])],
            &[],
        ),
        SavedValue::Str("seed".into()),
    );
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::add_tag")),
        "write.id_overflow",
    );
}

#[test]
fn an_id_overflow_fault_is_catchable_with_its_dotted_code() {
    // The exhaustion fault is a recoverable write fault: a surrounding `try`/`catch`
    // binds the `Error`, whose `code` is the dotted `write.id_overflow`. This pins
    // the catchable contract — the same fault both an uncaught run and a handler see.
    let program = checked_program(OVERFLOW_SCHEMA);
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "libs",
        &[SavedKey::Int(i64::MAX)],
        &keyed_data_path(&program, "libs", &[("tags", vec![SavedKey::Int(1)])], &[]),
        SavedValue::Str("seed".into()),
    );
    let value = run_entry(&store, checked_entry!(&program, "test::fresh_caught"))
        .expect("the fault is caught")
        .value;
    assert_eq!(value, Some(Value::Str("write.id_overflow".into())));

    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "libs",
        &[SavedKey::Int(1)],
        &keyed_data_path(
            &program,
            "libs",
            &[("tags", vec![SavedKey::Int(i64::MAX)])],
            &[],
        ),
        SavedValue::Str("seed".into()),
    );
    let value = run_entry(&store, checked_entry!(&program, "test::add_tag_caught"))
        .expect("the fault is caught")
        .value;
    assert_eq!(value, Some(Value::Str("write.id_overflow".into())));
}

#[test]
fn a_bare_entry_name_matching_two_public_functions_is_ambiguous() {
    // The CLI selects an entry by name. A bare name that two `pub` functions in
    // different modules both expose has no single target, so entry resolution faults
    // with `run.ambiguous_function`. The qualified names still resolve uniquely, so
    // the ambiguity is specifically the unqualified name, not a missing function.
    let program = checked_program_modules(&[
        "module a\npub fn run(): int\n    return 1\n",
        "module b\npub fn run(): int\n    return 2\n",
    ]);
    let error =
        CheckedEntryCall::new(&program, "run", vec![]).expect_err("bare `run` is ambiguous");
    assert_eq!(error.code, "run.ambiguous_function");
    assert!(
        CheckedEntryCall::new(&program, "a::run", vec![]).is_ok(),
        "the qualified entry resolves"
    );
    assert!(
        CheckedEntryCall::new(&program, "b::run", vec![]).is_ok(),
        "the qualified entry resolves"
    );
}

#[test]
fn a_qualified_entry_naming_a_private_function_is_rejected() {
    // A qualified entry that names a function the module does not export is not a
    // callable entry: resolution faults with `run.private_function`, distinct from
    // an unknown function. The module's own `pub` entry still resolves.
    let program = checked_program_modules(&[
        "module a\nfn secret(): int\n    return 1\npub fn open(): int\n    return secret()\n",
    ]);
    let error = CheckedEntryCall::new(&program, "a::secret", vec![])
        .expect_err("a private function is not an entry");
    assert_eq!(error.code, "run.private_function");
    assert!(
        CheckedEntryCall::new(&program, "a::open", vec![]).is_ok(),
        "the public entry in the same module resolves"
    );
    let missing = CheckedEntryCall::new(&program, "a::ghost", vec![])
        .expect_err("an undeclared entry is unknown");
    assert_eq!(
        missing.code, "run.unknown_function",
        "a missing name is a distinct fault from a private one"
    );
}

#[test]
fn binding_a_no_value_call_result_faults_at_run_time() {
    // A function with no return type yields no value. Binding its call to a `const`
    // type-checks clean (the binding has no declared type to violate), but using the
    // unit result as a value faults at run time with `run.no_value`.
    let program = checked_program(
        "pub fn noop()\n    const seen = 1\n\npub fn f(): int\n    const y = noop()\n    return 0\n",
    );
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::f")),
        "run.no_value",
    );
}
