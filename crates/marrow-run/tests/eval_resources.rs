//! Local resource construction and reads, the clock/env/log host capabilities, and
//! group-entry field writes with call-argument binding.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{CheckedEntryCall, Host, RUN_ABSENT, RUN_CAPABILITY, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[test]
fn builds_a_local_resource_and_saves_it() {
    let program = checked_program(BOOK_ADD);
    let store = TreeStore::memory();
    let id = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Str("Mort".into()),
            Value::Str("fiction".into())
        ),
    )
    .expect("add")
    .value;
    assert_identity_value(id, "books", &[SavedKey::Int(1)]);
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(1)))
            .expect("run")
            .value
    };
    assert_eq!(read("test::title_of"), Some(Value::Str("Mort".into())));
    assert_eq!(read("test::shelf_of"), Some(Value::Str("fiction".into())));
}

#[test]
fn reads_a_local_resource_field() {
    let program = checked_program(&format!(
        "{BOOK_SHELF_SCHEMA}pub fn echo(t: string): string\n    var book: Book\n    book.title = t\n    return book.title\n"
    ));
    let store = TreeStore::memory();
    let value = run_entry(
        &store,
        checked_entry!(&program, "test::echo", Value::Str("Mort".into())),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

/// A program that records the run's clock instant into a saved `instant` field
/// and reads it back, exercising `std::clock::now()` through `const` and a managed
/// write.
const CLOCK_SAMPLE: &str = "\
resource Event at ^events(id: int)
    required changedAt: instant

pub fn record(id: int)
    const now: instant = std::clock::now()
    ^events(id).changedAt = now

pub fn changed_at_of(id: int): instant
    return ^events(id).changedAt
";

#[test]
fn clock_now_reads_the_host_clock_capability() {
    let program = checked_program(CLOCK_SAMPLE);
    let store = TreeStore::memory();
    // 1970-01-01T00:00:01Z, one second after the epoch.
    let host = Host::new().with_clock(1_000_000_000);
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::record", Value::Int(1)),
    )
    .expect("record");
    // The instant round-trips through the managed write and a typed read.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::changed_at_of", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(outcome.value, Some(Value::Instant(1_000_000_000)));
}

#[test]
fn clock_now_without_a_clock_capability_is_a_capability_error() {
    let program = checked_program("pub fn t(): instant\n    return std::clock::now()\n");
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::t"));
    assert_run_error(result, RUN_CAPABILITY);
}

/// A program that reads environment variables through the three `std::env`
/// builtins: presence, lookup with a default, and a required lookup.
const ENV_SAMPLE: &str = "\
pub fn has(name: string): bool
    return std::env::exists(name)

pub fn read(name: string, fallback: string): string
    return std::env::get(name, fallback)

pub fn must(name: string): string
    return std::env::require(name)
";

/// A host whose environment is the test's fixed variables.
fn env_host() -> Host {
    Host::new().with_environment(HashMap::from([
        ("HOME".to_string(), "/home/marrow".to_string()),
        ("EMPTY".to_string(), String::new()),
    ]))
}

#[test]
fn env_reads_variables_from_the_host_capability() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    let host = env_host();
    let call = |entry: CheckedEntryCall| {
        run_entry_with_host(&store, &host, entry)
            .expect("env call")
            .value
    };
    // `exists` reports presence, including a present-but-empty variable.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::has",
            Value::Str("HOME".into())
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::has",
            Value::Str("EMPTY".into())
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::has",
            Value::Str("MISSING".into())
        )),
        Some(Value::Bool(false))
    );
    // `require` returns a present variable's value.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::must",
            Value::Str("HOME".into())
        )),
        Some(Value::Str("/home/marrow".into()))
    );
}

#[test]
fn env_get_falls_back_to_the_default_when_absent() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    let host = env_host();
    let call = |name: &str, fallback: &str| {
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(
                &program,
                "test::read",
                Value::Str(name.into()),
                Value::Str(fallback.into())
            ),
        )
        .expect("env get")
        .value
    };
    // A present variable wins over the default; an empty one is still present.
    assert_eq!(
        call("HOME", "fallback"),
        Some(Value::Str("/home/marrow".into()))
    );
    assert_eq!(call("EMPTY", "fallback"), Some(Value::Str(String::new())));
    // An absent variable falls back to the default.
    assert_eq!(
        call("MISSING", "fallback"),
        Some(Value::Str("fallback".into()))
    );
}

#[test]
fn env_require_missing_variable_is_an_absent_error() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    let host = env_host();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::must", Value::Str("MISSING".into())),
    );
    assert_run_error(result, RUN_ABSENT);
}

#[test]
fn env_without_an_environment_capability_is_a_capability_error() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    // Without a capability the whole module is unavailable, even presence checks.
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::has", Value::Str("HOME".into())),
    );
    assert_run_error(result, RUN_CAPABILITY);
}

/// A program that logs at each level, including an `Error` value.
const LOG_SAMPLE: &str = "\
pub fn note(m: string)
    std::log::info(m)

pub fn careful(m: string)
    std::log::warn(m)

pub fn boom()
    std::log::error(Error(code: \"log.boom\", message: \"kaboom\"))
";

#[test]
fn log_writes_each_level_to_the_host_sink() {
    let program = checked_program(LOG_SAMPLE);
    let store = TreeStore::memory();
    let sink = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&sink));
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::note", Value::Str("hello".into())),
    )
    .expect("info");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::careful", Value::Str("watch out".into())),
    )
    .expect("warn");
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::boom")).expect("error");
    assert_eq!(
        sink.borrow().as_str(),
        "INFO hello\nWARN watch out\nERROR [log.boom] kaboom\n"
    );
}

#[test]
fn log_without_a_log_capability_is_a_capability_error() {
    let program = checked_program(LOG_SAMPLE);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::note", Value::Str("hi".into())),
    );
    assert_run_error(result, RUN_CAPABILITY);
}

#[test]
fn log_error_requires_an_error_value() {
    checker_rejects(
        "pub fn t()\n    std::log::error(\"not an error\")\n",
        "check.call_argument",
    );
}

#[test]
fn a_group_entry_field_write_lands_in_saved_data() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}    notes(noteId: string)\n        text: string\n\npub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n\npub fn add_note(id: int, note: string, t: string)\n    ^books(id).notes(note).text = t\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(5)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_note",
            Value::Int(5),
            Value::Str("n1".into()),
            Value::Str("hello".into()),
        ),
    )
    .expect("group-entry write");
    let value = read_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(5)],
        &keyed_data_path(
            &program,
            "books",
            &[("notes", vec![SavedKey::Str("n1".into())])],
            &["text"],
        ),
        ScalarType::Str,
    );
    assert_eq!(value, Some(SavedValue::Str("hello".into())));
}

fn book_group_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    layer: &str,
    key: SavedKey,
    field: &str,
) -> Option<SavedValue> {
    read_data_value(
        program,
        store,
        "books",
        &[SavedKey::Int(id)],
        &keyed_data_path(program, "books", &[(layer, vec![key])], &[field]),
        ScalarType::Str,
    )
}
#[test]
fn group_entry_field_writes_compose_in_a_transaction() {
    // The sample's `add` shape: a whole-record write plus group-entry history
    // writes, all inside one transaction.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}    versions(version: int)\n        required title: string\n        required shelf: string\n\npub fn add(id: int, t: string, s: string)\n    transaction\n        ^books(id).title = t\n        ^books(id).versions(1).title = t\n        ^books(id).versions(1).shelf = s\n\npub fn title_of(id: int): string\n    return ^books(id).title\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ),
    )
    .expect("transactional group writes");
    // The top-level field reads back through the runtime.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into()))
    );
    // The group-entry members committed alongside it.
    let version_member =
        |field: &str| book_group_field(&program, &store, 1, "versions", SavedKey::Int(1), field);
    assert_eq!(
        version_member("title"),
        Some(SavedValue::Str("Mort".into()))
    );
    assert_eq!(
        version_member("shelf"),
        Some(SavedValue::Str("fiction".into()))
    );
}

#[test]
fn a_call_binds_named_arguments_by_name() {
    // Named arguments may appear in any order; they bind by name, not position.
    // `sub(b: 10, a: 3)` is `3 - 10`, not `10 - 3`.
    let program = checked_program(
        "pub fn sub(a: int, b: int): int\n    return a - b\n\npub fn go(): int\n    return sub(b: 10, a: 3)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::go")),
        Ok(Some(Value::Int(-7)))
    );
}

#[test]
fn a_call_mixes_positional_then_named_arguments() {
    let program = checked_program(
        "pub fn sub(a: int, b: int): int\n    return a - b\n\npub fn go(): int\n    return sub(10, b: 3)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::go")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_call_with_an_unknown_parameter_name_is_rejected() {
    checker_rejects(
        "pub fn sub(a: int, b: int): int\n    return a - b\n\npub fn go(): int\n    return sub(a: 1, c: 2)\n",
        "check.call_argument",
    );
}

#[test]
fn a_call_missing_an_argument_is_rejected() {
    checker_rejects(
        "pub fn sub(a: int, b: int): int\n    return a - b\n\npub fn go(): int\n    return sub(a: 1)\n",
        "check.call_argument",
    );
}

#[test]
fn a_call_supplying_a_parameter_twice_is_rejected() {
    checker_rejects(
        "pub fn sub(a: int, b: int): int\n    return a - b\n\npub fn go(): int\n    return sub(1, a: 2)\n",
        "check.call_argument",
    );
}

// Positional-after-named (`sub(b: 1, 2)`) is a parser-owned syntax rule covered in
// marrow-syntax, so it never reaches the runtime.
