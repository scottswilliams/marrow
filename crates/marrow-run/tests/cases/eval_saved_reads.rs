//! Output builtins, saved scalar field reads through the runtime, the
//! whole-resource required-field rule, and exists/coalesce presence checks.

use crate::support;
use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{RUN_ABSENT, RUN_TYPE, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

#[test]
fn print_writes_a_line_to_output() {
    let program = checked_program("pub fn main()\n    print($\"hello {1}\")\n");
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "hello 1\n");
}

#[test]
fn output_accumulates_across_calls() {
    let program = checked_program(
        "pub fn greet(name: string)\n    print($\"hi {name}\")\n\npub fn main()\n    greet(\"a\")\n    greet(\"b\")\n",
    );
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.output, "hi a\nhi b\n");
}

#[test]
fn print_takes_one_argument() {
    let program = checked_program("pub fn main()\n    print()\n");
    let result = run_full(checked_entry!(&program, "test::main"));
    assert_run_error(result, RUN_TYPE);
}

/// A program with a saved `Book` resource and functions that read a title.
const BOOK_READER: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn title_of(id: int): string
    if const title = ^books(id).title
        return title
    return \"\"

pub fn show(id: int)
    if const title = ^books(id).title
        print($\"title: {title}\")
";

fn store_with_title(program: &CheckedRuntimeProgram, id: i64, title: &str) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &["title"]),
        SavedValue::Str(title.into()),
    );
    store
}

#[test]
fn reads_a_scalar_field_from_saved_data() {
    let program = checked_program(BOOK_READER);
    let store = store_with_title(&program, 1, "Mort");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn a_binding_guard_skips_an_absent_field() {
    let program = checked_program(BOOK_READER);
    let store = TreeStore::memory(); // empty: the title is absent
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    )
    .expect("guarded read");
    assert_eq!(outcome.value, Some(Value::Str(String::new())));
}

#[test]
fn if_const_binding_guard_reads_present_values_and_skips_absent_values() {
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20subtitle: string\nstore ^books(id: int): Book\n\n\
         pub fn write_subtitle(id: int, subtitle: string)\n\
         \x20\x20\x20\x20^books(id).subtitle = subtitle\n\n\
         pub fn subtitle_or_missing(id: int): string\n\
         \x20\x20\x20\x20if const subtitle = ^books(id).subtitle\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return subtitle\n\
         \x20\x20\x20\x20return \"missing\"\n",
    );
    let store = empty_store();

    let missing = run_entry(
        &store,
        checked_entry!(&program, "test::subtitle_or_missing", Value::Int(1)),
    )
    .expect("missing branch");
    assert_eq!(missing.value, Some(Value::Str("missing".into())));

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::write_subtitle",
            Value::Int(1),
            Value::Str("Guards".into())
        ),
    )
    .expect("write");

    let present = run_entry(
        &store,
        checked_entry!(&program, "test::subtitle_or_missing", Value::Int(1)),
    )
    .expect("present branch");
    assert_eq!(present.value, Some(Value::Str("Guards".into())));
}

#[test]
fn a_saved_read_interpolates_and_prints() {
    let program = checked_program(BOOK_READER);
    let store = store_with_title(&program, 7, "Mort");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::show", Value::Int(7)),
    )
    .expect("run");
    assert_eq!(outcome.output, "title: Mort\n");
}

#[test]
fn whole_resource_read_rejects_missing_required_durable_fields() {
    let program = checked_program(
        "resource Book\n    required title: string\n    required shelf: string\nstore ^books(id: int): Book\n\npub fn read(id: int): Book\n    var fallback: Book\n    fallback.title = \"\"\n    fallback.shelf = \"\"\n    return ^books(id) ?? fallback\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["shelf"]),
        SavedValue::Str("fiction".into()),
    );
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    );

    assert_run_error(result, RUN_ABSENT);
}

#[test]
fn whole_resource_coalesce_returns_default_for_an_absent_record() {
    let program = checked_program(
        "resource Book\n    required title: string\n    required shelf: string\nstore ^books(id: int): Book\n\npub fn read(id: int): Book\n    var fallback: Book\n    fallback.title = \"missing\"\n    fallback.shelf = \"none\"\n    return ^books(id) ?? fallback\n",
    );
    let store = empty_store();
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    )
    .expect("read");

    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("title".into(), Value::Str("missing".into())),
            ("shelf".into(), Value::Str("none".into())),
        ]))
    );
}

#[test]
fn whole_resource_coalesce_rejects_a_present_malformed_record() {
    let program = checked_program(
        "resource Book\n    required title: string\n    required shelf: string\nstore ^books(id: int): Book\n\npub fn read(id: int): Book\n    var fallback: Book\n    fallback.title = \"missing\"\n    fallback.shelf = \"none\"\n    return ^books(id) ?? fallback\n",
    );
    let store = empty_store();
    store
        .write_data_value(
            &store_catalog_id(&program, "books"),
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            vec![0xff],
        )
        .expect("raw corrupt data write succeeds");
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["shelf"]),
        SavedValue::Str("fiction".into()),
    );

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::read", Value::Int(1)),
        ),
        RUN_TYPE,
    );
}

/// A program that reads saved `Book` data with `exists` and the `??`
/// absence-default.
const BOOK_READ: &str = "\
resource Book
    required title: string
    subtitle: string
store ^books(id: int): Book

pub fn has_book(id: int): bool
    return exists(^books(id))

pub fn has_title(id: int): bool
    return exists(^books(id).title)

pub fn has_subtitle(id: int): bool
    return exists(^books(id).subtitle)

pub fn subtitle_or(id: int, fallback: string): string
    return ^books(id).subtitle ?? fallback
";

#[test]
fn exists_reports_record_and_field_presence() {
    let program = checked_program(BOOK_READ);
    let store = store_with_title(&program, 1, "Mort");
    let value = |entry, id| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(id)))
            .expect("run")
            .value
    };
    // Record 1 exists (it has the title child); record 2 does not.
    assert_eq!(value("test::has_book", 1), Some(Value::Bool(true)));
    assert_eq!(value("test::has_book", 2), Some(Value::Bool(false)));
    // Its title field is present; its sparse subtitle is absent.
    assert_eq!(value("test::has_title", 1), Some(Value::Bool(true)));
    assert_eq!(value("test::has_subtitle", 1), Some(Value::Bool(false)));
}

#[test]
fn coalesce_returns_the_default_for_an_absent_field() {
    let program = checked_program(BOOK_READ);
    let store = store_with_title(&program, 1, "Mort"); // subtitle is absent
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::subtitle_or",
            Value::Int(1),
            Value::Str("(none)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(none)".into())));
}

#[test]
fn coalesce_returns_the_value_when_present() {
    let program = checked_program(BOOK_READ);
    let store = store_with_title(&program, 1, "Mort");
    // Populate the sparse subtitle directly.
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["subtitle"]),
        SavedValue::Str("A Discworld Novel".into()),
    );
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::subtitle_or",
            Value::Int(1),
            Value::Str("(none)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("A Discworld Novel".into())));
}
