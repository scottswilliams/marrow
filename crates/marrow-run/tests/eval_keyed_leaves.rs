//! Keyed-leaf collections: append at the next position, explicit keyed-leaf
//! writes and the holes they leave, read-your-writes inside a transaction, and
//! the unguarded absent-element rejection.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

#[test]
fn an_unguarded_absent_element_read_is_rejected() {
    checker_rejects(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\npub fn titleOrCode(id: int): string\n    try\n        return ^books(id).title\n    catch err: Error\n        return err.code\n",
        "check.bare_maybe_present_read",
    );
}

#[test]
fn reads_inside_a_transaction_see_earlier_writes() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn rww(id: int): string\n    transaction\n        ^books(id).title = \"fresh\"\n        return ^books(id).title\n"
    ));
    let store = TreeStore::memory();
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::rww", Value::Int(1))).expect("run");
    assert_eq!(outcome.value, Some(Value::Str("fresh".into())));
}

#[test]
fn append_writes_at_the_next_position() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n"
    ));
    let store = TreeStore::memory();
    let appended = |t: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add_tag",
                Value::Int(5),
                Value::Str(t.into())
            ),
        )
        .expect("run")
        .value
    };
    // Successive appends take positions 1 then 2 (no hole-filling).
    assert_eq!(appended("a"), Some(Value::Int(1)));
    assert_eq!(appended("b"), Some(Value::Int(2)));
    // The values landed at `^books(5).tags(1)` and `tags(2)`.
    let tag = |pos: i64| -> Option<SavedValue> {
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(5)],
            &keyed_data_path(
                &program,
                "books",
                &[("tags", vec![SavedKey::Int(pos)])],
                &[],
            ),
            ScalarType::Str,
        )
    };
    assert_eq!(tag(1), Some(SavedValue::Str("a".into())));
    assert_eq!(tag(2), Some(SavedValue::Str("b".into())));
}

#[test]
fn appends_then_reads_back_keyed_leaf_entries() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_tag",
            Value::Int(5),
            Value::Str("a".into())
        ),
    )
    .expect("append");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_tag",
            Value::Int(5),
            Value::Str("b".into())
        ),
    )
    .expect("append");
    let tag = |pos: i64| {
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(5), Value::Int(pos)),
        )
        .expect("read")
        .value
    };
    assert_eq!(tag(1), Some(Value::Str("a".into())));
    assert_eq!(tag(2), Some(Value::Str("b".into())));
    assert_eq!(tag(3), Some(Value::Str(String::new())));
}

#[test]
fn explicit_keyed_leaf_write_then_reads_back() {
    // `^books(id).tags(pos) = value` writes one keyed-leaf entry directly, and a
    // string-keyed leaf `scores(key) = value` writes through the same path.
    let program = checked_program(
        "resource Book\n    required title: string\n    tags(pos: int): string\n    scores(key: string): int\nstore ^books(id: int): Book\n\npub fn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\npub fn set_score(id: int, key: string, n: int)\n    ^books(id).scores(key) = n\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n\npub fn score_at(id: int, key: string): int\n    return ^books(id).scores(key) ?? 0\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(5),
            Value::Int(3),
            Value::Str("fiction".into())
        ),
    )
    .expect("explicit keyed-leaf write");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_score",
            Value::Int(5),
            Value::Str("alice".into()),
            Value::Int(7)
        ),
    )
    .expect("string-keyed leaf write");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(5), Value::Int(3))
        )
        .expect("read")
        .value,
        Some(Value::Str("fiction".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::score_at",
                Value::Int(5),
                Value::Str("alice".into())
            )
        )
        .expect("read")
        .value,
        Some(Value::Int(7))
    );
}

#[test]
fn explicit_keyed_leaf_write_creates_a_hole_that_append_skips() {
    // An explicit write past the dense range leaves a hole; append chooses one
    // past the highest positive key, not the first gap.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\npub fn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    // Write position 5 directly, leaving 1..=4 as holes.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(9),
            Value::Int(5),
            Value::Str("hi".into())
        ),
    )
    .expect("explicit write");
    // Append lands at 6 (one past the highest positive key), skipping the holes.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add_tag",
                Value::Int(9),
                Value::Str("next".into())
            )
        )
        .expect("append")
        .value,
        Some(Value::Int(6))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(9), Value::Int(6))
        )
        .expect("read")
        .value,
        Some(Value::Str("next".into()))
    );
}
