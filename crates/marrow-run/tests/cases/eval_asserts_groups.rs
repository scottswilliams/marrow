//! Conditionals, the std::assert builtins, whole and nested group-entry writes,
//! and absence-coalesce fault propagation.

use crate::support;
use support::*;

use marrow_run::{RUN_ASSERT, RUN_TYPE, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

#[test]
fn evaluates_conditionals() {
    let source = "pub fn max(a: int, b: int): int\n    if a > b\n        return a\n    return b\n";
    assert_eq!(
        eval_source(source, "max", vec![Value::Int(7), Value::Int(3)]),
        Ok(Some(Value::Int(7)))
    );
    assert_eq!(
        eval_source(source, "max", vec![Value::Int(3), Value::Int(7)]),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn std_assert_is_true_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isTrue(1 == 1)\n");
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isTrue(1 == 2)\n");
    assert_run_error(run(checked_entry!(&program, "test::bad")), RUN_ASSERT);
}

#[test]
fn std_assert_is_false_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isFalse(1 == 2)\n");
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isFalse(1 == 1)\n");
    assert_run_error(run(checked_entry!(&program, "test::bad")), RUN_ASSERT);
}

#[test]
fn std_assert_equal_passes_and_renders_scalar_mismatches() {
    let program = checked_program(
        "pub fn ok()\n    std::assert::equal(1, 1)\n    std::assert::equal(\"same\", \"same\")\n",
    );
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad_int()\n    std::assert::equal(1, 2)\n");
    let error = run_expecting_error(checked_entry!(&program, "test::bad_int"));
    assert_eq!(error.code(), RUN_ASSERT);
    let (_code, message) = error_throw_fields(&error);
    assert_eq!(message, "expected 2, got 1");

    let program =
        checked_program("pub fn bad_str()\n    std::assert::equal(\"actual\", \"expected\")\n");
    let error = run_expecting_error(checked_entry!(&program, "test::bad_str"));
    assert_eq!(error.code(), RUN_ASSERT);
    let (_code, message) = error_throw_fields(&error);
    assert_eq!(message, "expected \"expected\", got \"actual\"");
}

#[test]
fn std_assert_equal_renders_bytes_and_temporals_canonically() {
    // Distinct bytes must render distinctly as `0x`-hex, not a length-only form
    // that collapses different values to an identical, spurious-looking message.
    let program = checked_program(
        "pub fn bad()\n    std::assert::equal(std::bytes::fromText(\"abc\"), std::bytes::fromText(\"xyz\"))\n",
    );
    let error = run_expecting_error(checked_entry!(&program, "test::bad"));
    assert_eq!(error.code(), RUN_ASSERT);
    let (_code, message) = error_throw_fields(&error);
    assert_eq!(message, "expected 0x78797a, got 0x616263");

    // Temporals render as their canonical text, not a raw epoch integer.
    let program = checked_program(
        "pub fn bad()\n    std::assert::equal(date(\"2024-01-02\"), date(\"2024-01-03\"))\n",
    );
    let error = run_expecting_error(checked_entry!(&program, "test::bad"));
    assert_eq!(error.code(), RUN_ASSERT);
    let (_code, message) = error_throw_fields(&error);
    assert_eq!(message, "expected 2024-01-03, got 2024-01-02");
}

#[test]
fn std_assert_fail_raises_with_its_message() {
    let program = checked_program("pub fn bad()\n    std::assert::fail(\"boom\")\n");
    let error = run_expecting_error(checked_entry!(&program, "test::bad"));
    assert_eq!(error.code(), RUN_ASSERT);
    let (code, message) = error_throw_fields(&error);
    assert_eq!(code, RUN_ASSERT);
    assert_eq!(message, "boom");
}

#[test]
fn std_assert_absent_passes_when_nothing_is_saved() {
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn ok()\n    std::assert::isAbsent(^books(1))\n",
    );
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));
}

#[test]
fn std_assert_absent_fails_when_a_value_is_present() {
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn bad()\n    std::assert::isAbsent(^books(1))\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("present".into()),
    );
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::bad")),
        RUN_ASSERT,
    );
}

#[test]
fn std_assert_absent_passes_for_an_absent_local_optional() {
    // `isAbsent` accepts any optional value, resolving a local `T?` to its
    // `Option<Value>` like `exists` does.
    let program = checked_program(
        "pub fn ok()\n    const x: string? = absent\n    std::assert::isAbsent(x)\n",
    );
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));
}

#[test]
fn std_assert_absent_fails_for_a_present_local_optional() {
    let program = checked_program(
        "pub fn bad()\n    const x: string? = \"here\"\n    std::assert::isAbsent(x)\n",
    );
    assert_run_error(run(checked_entry!(&program, "test::bad")), RUN_ASSERT);
}

#[test]
fn std_assert_rejects_misused_arguments() {
    checker_rejects(
        "pub fn bad()\n    std::assert::isTrue(1)\n",
        "check.call_argument",
    );
    checker_rejects(
        "pub fn bad()\n    std::assert::fail(42)\n",
        "check.call_argument",
    );
    checker_rejects(
        "pub fn bad()\n    std::assert::equal(1, \"1\")\n",
        "check.call_argument",
    );
}

#[test]
fn a_passing_assert_lets_execution_continue() {
    // A passing assertion produces no value and falls through to later statements.
    let program =
        checked_program("pub fn ok(): int\n    std::assert::isTrue(1 == 1)\n    return 7\n");
    assert_eq!(
        run(checked_entry!(&program, "test::ok")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_whole_group_entry_write_creates_the_entry() {
    // `^books(1).versions(2) = b` writes the whole group entry from a resource
    // value; the runtime matches its fields against the group's members by name.
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v2\"\n\
         \x20\x20\x20\x20^books(1).versions(2) = b\n\
         \n\
         pub fn version_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::version_title"))
            .unwrap()
            .value,
        Some(Value::Str("v2".into()))
    );
}

#[test]
fn a_nested_group_field_round_trips() {
    // `versions(version)` entries hold a nested `comments(pos)` group; writing and
    // reading `^books(1).versions(2).comments(3).text` exercises a saved-tree path
    // deeper than one keyed layer.
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20comments(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20required text: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20^books(1).title = \"root\"\n\
         \x20\x20\x20\x20^books(1).versions(2).title = \"version\"\n\
         \x20\x20\x20\x20^books(1).versions(2).comments(3).text = \"deep\"\n\
         \n\
         pub fn comment(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).comments(3).text ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::comment"))
            .unwrap()
            .value,
        Some(Value::Str("deep".into()))
    );
}

#[test]
fn a_whole_group_entry_can_be_read_and_copied() {
    // `^books(1).versions(2) = ^books(1).versions(1)` reads the whole entry as a
    // value (RHS) and writes it to another key (LHS).
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v1\"\n\
         \x20\x20\x20\x20^books(1).versions(1) = b\n\
         \x20\x20\x20\x20if exists(^books(1).versions(1))\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).versions(2) = ^books(1).versions(1)\n\
         \n\
         pub fn copied_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::copied_title"))
            .unwrap()
            .value,
        Some(Value::Str("v1".into()))
    );
}

#[test]
fn coalesce_absorbs_only_an_absent_left_read() {
    // `^path ?? default` falls back only on an absent element. A populated read
    // yields its own value; the default is never reached when the element exists.
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\n\
         pub fn read(id: int): string\n    return ^books(id).title ?? \"none\"\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("Mort".into()),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::read", Value::Int(1))
        )
        .unwrap()
        .value,
        Some(Value::Str("Mort".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::read", Value::Int(2))
        )
        .unwrap()
        .value,
        Some(Value::Str("none".into()))
    );
}

#[test]
fn coalesce_propagates_a_non_absent_fault_on_its_left() {
    // The `??` default catches only `run.absent_element`. A present-but-corrupt
    // leaf decodes to a `run.type` fault, which must propagate rather than be
    // silently swallowed into the default: a hidden decode failure would mask
    // saved-data corruption behind a plausible fallback value.
    let program = checked_program(
        "resource Counter\n    n: int\nstore ^counters(id: int): Counter\n\n\
         pub fn read(id: int): int\n    return ^counters(id).n ?? -1\n",
    );
    let store = empty_store();
    // Store text bytes where the `int` leaf decoder expects a canonical integer,
    // so the read faults with `run.type` instead of returning absent.
    write_data_value(
        &program,
        &store,
        "counters",
        &[SavedKey::Int(1)],
        &data_path(&program, "counters", &["n"]),
        SavedValue::Str("not-an-int".into()),
    );
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::read", Value::Int(1)),
        ),
        RUN_TYPE,
    );
}
