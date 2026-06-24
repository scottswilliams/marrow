//! Keyed-leaf collections: append at the next position, explicit keyed-leaf
//! writes and the holes they leave, read-your-writes inside a transaction, and
//! the unguarded absent-element rejection.

use crate::support;
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
        "{BOOK_PRIMARY_SCHEMA}pub fn rww(id: int): string\n    transaction\n        ^books(id).title = \"fresh\"\n        return ^books(id).title ?? \"\"\n"
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
fn if_const_reads_a_keyed_leaf_entry_value() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).tags(2) = \"funny\"\n\npub fn guarded_tag(id: int, pos: int): string\n    if const tag = ^books(id).tags(pos)\n        return tag\n    return \"missing\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");

    let present = run_entry(
        &store,
        checked_entry!(&program, "test::guarded_tag", Value::Int(1), Value::Int(2)),
    )
    .expect("present keyed leaf")
    .value;
    assert_eq!(present, Some(Value::Str("funny".into())));

    let absent = run_entry(
        &store,
        checked_entry!(&program, "test::guarded_tag", Value::Int(1), Value::Int(3)),
    )
    .expect("absent keyed leaf")
    .value;
    assert_eq!(absent, Some(Value::Str("missing".into())));
}

#[test]
fn if_const_reads_a_keyed_group_entry_value() {
    let program = checked_program(
        "resource Book\n    required title: string\n    versions(version: int)\n        required title: string\nstore ^books(id: int): Book\n\npub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).versions(2).title = \"second\"\n\npub fn guarded_version(id: int, version: int): string\n    if const entry = ^books(id).versions(version)\n        return entry.title\n    return \"missing\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");

    let present = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::guarded_version",
            Value::Int(1),
            Value::Int(2)
        ),
    )
    .expect("present keyed group")
    .value;
    assert_eq!(present, Some(Value::Str("second".into())));

    let absent = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::guarded_version",
            Value::Int(1),
            Value::Int(3)
        ),
    )
    .expect("absent keyed group")
    .value;
    assert_eq!(absent, Some(Value::Str("missing".into())));
}

#[test]
fn a_dynamic_non_positive_sequence_write_faults_and_persists_nothing() {
    // A sequence is 1-based, so a position below 1 addresses no node. A dynamic
    // write to such a position must raise the catchable absent fault and leave the
    // store untouched, never persisting an unreachable node.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n"
    ));
    for pos in [0, -5] {
        let store = TreeStore::memory();
        assert_run_error(
            run_entry(
                &store,
                checked_entry!(
                    &program,
                    "test::set_tag",
                    Value::Int(7),
                    Value::Int(pos),
                    Value::Str("x".into())
                ),
            ),
            "run.absent_element",
        );
        // Nothing is persisted at the non-positive position.
        assert_eq!(
            read_data_value(
                &program,
                &store,
                "books",
                &[SavedKey::Int(7)],
                &keyed_data_path(
                    &program,
                    "books",
                    &[("tags", vec![SavedKey::Int(pos)])],
                    &[]
                ),
                ScalarType::Str,
            ),
            None,
        );
    }
}

#[test]
fn a_dynamic_non_positive_store_root_int_key_write_faults_and_persists_nothing() {
    // A store keyed by a single integer is itself a 1-based sequence, so a position
    // below 1 addresses no record. A dynamic whole-record write to such a key must
    // raise the catchable absent fault and leave the store untouched, never persisting
    // an unreachable record.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}\npub fn put(id: int, t: string)\n    ^books(id) = Book(title: t)\n"
    ));
    for id in [0, -3] {
        let store = TreeStore::memory();
        assert_run_error(
            run_entry(
                &store,
                checked_entry!(
                    &program,
                    "test::put",
                    Value::Int(id),
                    Value::Str("x".into())
                ),
            ),
            "run.absent_element",
        );
        assert_eq!(
            read_data_value(
                &program,
                &store,
                "books",
                &[SavedKey::Int(id)],
                &data_path(&program, "books", &["title"]),
                ScalarType::Str,
            ),
            None,
            "a faulted store-root write must persist nothing at id {id}",
        );
    }
}

#[test]
fn a_read_of_a_non_positive_sequence_position_is_absent() {
    // The read side resolves a below-1 position to absent, the same contract the
    // write side enforces: `??` recovers it rather than faulting fatally.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed(id: int)\n    ^books(id).tags(1) = \"one\"\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"absent\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(7)),
    )
    .expect("seed");
    let tag = |pos: i64| {
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(7), Value::Int(pos)),
        )
        .expect("read")
        .value
    };
    assert_eq!(tag(0), Some(Value::Str("absent".into())));
    assert_eq!(tag(-2), Some(Value::Str("absent".into())));
    // The legitimate 1-based position is unaffected.
    assert_eq!(tag(1), Some(Value::Str("one".into())));
}

#[test]
fn count_and_neighbor_over_a_non_positive_sequence_position_resolve_as_absent() {
    // The presence-family `count` and the maybe-present `next`/`prev` resolve a
    // below-1 position exactly as the positional read does: addressed-no-node, the
    // same contract a positive out-of-range position already obeys. `count` returns
    // 0 and the neighbor probes recover through `??` rather than faulting fatally.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed(id: int)\n    ^books(id).tags(1) = \"one\"\n\npub fn count_at(id: int, pos: int): int\n    return count(^books(id).tags(pos))\n\npub fn next_at(id: int, pos: int): int\n    return next(^books(id).tags(pos)) ?? -999\n\npub fn prev_at(id: int, pos: int): int\n    return prev(^books(id).tags(pos)) ?? -999\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(7)),
    )
    .expect("seed");
    let probe = |func: &str, pos: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                &format!("test::{func}"),
                Value::Int(7),
                Value::Int(pos)
            ),
        )
        .unwrap_or_else(|error| panic!("{func}({pos}) faulted: {error:?}"))
        .value
    };
    // A non-positive position counts as 0, matching a positive out-of-range hole.
    assert_eq!(probe("count_at", 0), Some(Value::Int(0)));
    assert_eq!(probe("count_at", -3), Some(Value::Int(0)));
    assert_eq!(probe("count_at", 9), Some(Value::Int(0)));
    // The populated position still counts as 1.
    assert_eq!(probe("count_at", 1), Some(Value::Int(1)));
    // Neighbor navigation over a non-positive start falls off cleanly to the
    // fallback, the same as a positive out-of-range start does.
    assert_eq!(probe("next_at", 0), Some(Value::Int(-999)));
    assert_eq!(probe("next_at", -3), Some(Value::Int(-999)));
    assert_eq!(probe("prev_at", 0), Some(Value::Int(-999)));
    assert_eq!(probe("prev_at", -3), Some(Value::Int(-999)));
}

#[test]
fn a_non_positive_sequence_group_entry_read_is_absent_through_the_whole_spine() {
    // A sequence of groups raises the absent fault at the position segment, not the
    // trailing field. A guarded read of `^books(id).versions(pos).title`,
    // `if const`, and `exists` over a below-1 position must still resolve to absence
    // rather than letting the position fault escape past the read-site form.
    let program = checked_program(
        "resource Book\n    required title: string\n    versions(version: int)\n        required title: string\nstore ^books(id: int): Book\n\npub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).versions(1).title = \"first\"\n\npub fn version_title(id: int, version: int): string\n    return ^books(id).versions(version).title ?? \"absent\"\n\npub fn version_exists(id: int, version: int): bool\n    return exists(^books(id).versions(version))\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let title = |version: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::version_title",
                Value::Int(1),
                Value::Int(version)
            ),
        )
        .expect("read")
        .value
    };
    assert_eq!(title(0), Some(Value::Str("absent".into())));
    assert_eq!(title(-4), Some(Value::Str("absent".into())));
    assert_eq!(title(1), Some(Value::Str("first".into())));
    let exists = |version: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::version_exists",
                Value::Int(1),
                Value::Int(version)
            ),
        )
        .expect("exists")
        .value
    };
    assert_eq!(exists(0), Some(Value::Bool(false)));
    assert_eq!(exists(1), Some(Value::Bool(true)));
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
