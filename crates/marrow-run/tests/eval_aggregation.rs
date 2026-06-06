//! Count over scalar, child, index and composite paths, direct saved-root
//! streaming loops, and values/entries materialization including nested keyed
//! layers and cross-boundary write faults.

#[macro_use]
mod support;

use support::*;

use marrow_run::{CheckedEntryCall, Host, RUN_ASSERT, RUN_UNSUPPORTED, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

#[test]
fn count_reports_scalar_presence_and_child_counts() {
    let program = checked_program(BOOK_COUNT);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let count = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry))
            .expect("count")
            .value
    };
    assert_eq!(count("test::countTitle"), Some(Value::Int(1)));
    assert_eq!(count("test::countTags"), Some(Value::Int(2)));
    assert_eq!(count("test::countMissingField"), Some(Value::Int(0)));
    assert_eq!(count("test::countMissingTags"), Some(Value::Int(0)));
}

#[test]
fn count_of_a_path_with_both_value_and_children_counts_children() {
    // When a path has BOTH a value and children, `count` returns the number of
    // immediate children, not children-plus-one.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags: sequence[string]\n\npub fn n(): int\n    return count(^books(1).tags)\n",
    );
    let store = TreeStore::memory();
    // Seed a value at `^books(1).tags` itself and two children below it.
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["tags"]),
        SavedValue::Str("self".into()),
    );
    for (pos, value) in [(1, "a"), (2, "b")] {
        write_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &keyed_data_path(
                &program,
                "books",
                &[("tags", vec![SavedKey::Int(pos)])],
                &[],
            ),
            SavedValue::Str(value.into()),
        );
    }
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::n"))
            .expect("run")
            .value,
        Some(Value::Int(2)),
    );
}

/// `count` over a declared index branch returns the number of entries under that
/// branch, exactly as `keys(...)` over the same branch would yield. The branch is
/// a non-unique index so several entries share one query key.
const BOOK_COUNT_INDEX: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
    tags: sequence[string]

    index byShelf(shelf, id)

pub fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

pub fn tag(id: int, t: string): int
    return append(^books(id).tags, t)

pub fn countBranch(shelf: string): int
    return count(^books.byShelf(shelf))

pub fn keysBranch(shelf: string): int
    var c = 0
    for id in keys(^books.byShelf(shelf))
        c = c + 1
    return c

pub fn countRoot(): int
    return count(^books)

pub fn countLayer(id: int): int
    return count(^books(id).tags)

pub fn countScalar(id: int): int
    return count(^books(id).title)

pub fn countRecord(id: int): int
    return count(^books(id))
";

#[test]
fn count_over_an_index_branch_matches_branch_entry_count() {
    let program = checked_program(BOOK_COUNT_INDEX);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let call = |entry: CheckedEntryCall| run_entry(&store, entry).expect("count").value;
    // Two tags on book 1, so its keyed/sequence layer has two entries.
    call(checked_entry!(
        &program,
        "test::tag",
        Value::Int(1),
        Value::Str("a".into())
    ));
    call(checked_entry!(
        &program,
        "test::tag",
        Value::Int(1),
        Value::Str("b".into())
    ));

    // `count(^books.byShelf(shelf))` returns the entry count under that index
    // branch, matching `keys(...)` over the same branch.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::countBranch",
            Value::Str("fiction".into())
        )),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::keysBranch",
            Value::Str("fiction".into())
        )),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::countBranch",
            Value::Str("history".into())
        )),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::keysBranch",
            Value::Str("history".into())
        )),
        Some(Value::Int(1))
    );
    // An empty branch counts as zero, like `keys(...)` of it.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::countBranch",
            Value::Str("romance".into())
        )),
        Some(Value::Int(0))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::keysBranch",
            Value::Str("romance".into())
        )),
        Some(Value::Int(0))
    );

    // Count shapes stay byte-identical: a keyed/sequence layer counts its
    // entries, a scalar counts as 1, and a whole record counts its populated
    // immediate children. These all keep the read/child-keys path.
    assert_eq!(
        call(checked_entry!(&program, "test::countLayer", Value::Int(1))),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::countLayer", Value::Int(3))),
        Some(Value::Int(0))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::countScalar", Value::Int(1))),
        Some(Value::Int(1))
    );
    assert!(matches!(
        call(checked_entry!(&program, "test::countRecord", Value::Int(1))),
        Some(Value::Int(n)) if n >= 1
    ));
    // A primary root counts the record identities that direct iteration yields,
    // not generated index branches stored beside the records.
    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(3))
    );
}

#[test]
fn count_over_an_indexed_root_ignores_populated_index_branches() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n    isbn: string\n\n    index byShelf(shelf, id)\n    index byIsbn(isbn) unique\n\npub fn add(id: int, t: string, s: string)\n    ^books(id).title = t\n    ^books(id).shelf = s\n\npub fn addIsbn(id: int, isbn: string)\n    ^books(id).isbn = isbn\n\npub fn countRoot(): int\n    return count(^books)\n\npub fn iterRoot(): int\n    var n = 0\n    for book in ^books\n        n = n + 1\n    return n\n",
    );
    let store = TreeStore::memory();
    let call = |entry: CheckedEntryCall| run_entry(&store, entry).expect("run").value;

    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(0))
    );
    call(checked_entry!(
        &program,
        "test::add",
        Value::Int(1),
        Value::Str("Mort".into()),
        Value::Str("fiction".into())
    ));
    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::iterRoot")),
        Some(Value::Int(1))
    );

    call(checked_entry!(
        &program,
        "test::addIsbn",
        Value::Int(1),
        Value::Str("ISBN-1".into())
    ));
    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::iterRoot")),
        Some(Value::Int(1))
    );
}

#[test]
fn count_over_a_direct_saved_root_matches_written_records() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n    ^books(3).title = \"C\"\n\npub fn countRoot(): int\n    return count(^books)\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countRoot"))
            .expect("count")
            .value,
        Some(Value::Int(3))
    );
}

#[test]
fn keys_saved_root_loop_returns_ids_in_store_order() {
    let program = checked_program(
        &[BOOK_PRIMARY_SCHEMA, "pub fn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n    ^books(3).title = \"C\"\n\npub fn idOrder()\n    for id in keys(^books)\n        print($\"{id}\")\n"].concat(),
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::idOrder"))
            .expect("id order")
            .output,
        "1\n2\n3\n"
    );
}

#[test]
fn direct_saved_root_loop_streams_ids_and_reads_current_values() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n\npub fn mutateFutureValue(): int\n    var total = 0\n    for id in ^books\n        if ^books(id).title == \"A\"\n            total = total * 10 + 1\n            ^books(2).title = \"Z\"\n        else if ^books(id).title == \"B\"\n            total = total * 10 + 2\n        else if ^books(id).title == \"Z\"\n            total = total * 10 + 9\n    return total\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::mutateFutureValue"))
            .expect("loop")
            .value,
        Some(Value::Int(19))
    );
}

#[test]
fn direct_saved_root_loop_returns_before_later_records() {
    let program = checked_program(
        &[BOOK_PRIMARY_SCHEMA, "pub fn seed(id: int)\n    ^books(id).title = $\"Book {id}\"\n\npub fn printFirstId()\n    for id in keys(^books)\n        print($\"{id}\")\n        return\n"].concat(),
    );
    let store = TreeStore::memory();
    for id in 1..=129 {
        run_entry(
            &store,
            checked_entry!(&program, "test::seed", Value::Int(id)),
        )
        .expect("seed");
    }

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::printFirstId"))
            .expect("first id")
            .output,
        "1\n"
    );
}

#[test]
fn direct_values_loop_returns_before_reading_later_records() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    note: string\n\npub fn seed()\n    ^books(1).title = \"Mort\"\n\npub fn firstTitle(): string\n    for book in values(^books)\n        return book.title\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(2)],
        &data_path(&program, "books", &["note"]),
        SavedValue::Str("incomplete row".into()),
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstTitle"))
            .expect("first title")
            .value,
        Some(Value::Str("Mort".into()))
    );
}

#[test]
fn direct_entries_loop_returns_before_reading_later_records() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    note: string\n\npub fn seed()\n    ^books(1).title = \"Mort\"\n\npub fn firstTitle(): string\n    for id, book in ^books\n        return $\"{id}: {book.title}\"\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(2)],
        &data_path(&program, "books", &["note"]),
        SavedValue::Str("incomplete row".into()),
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstTitle"))
            .expect("first title")
            .value,
        Some(Value::Str("1: Mort".into()))
    );
}

#[test]
fn keys_saved_layer_loops_return_keys_in_order() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"A\"\n    ^books(1).tags(1) = \"x\"\n    ^books(1).tags(2) = \"y\"\n    ^books(1).tags(3) = \"z\"\n\npub fn tagKeys(): int\n    var total = 0\n    for pos in keys(^books(1).tags)\n        total = total * 10 + pos\n    return total\n\npub fn tagKeysRev(): int\n    var total = 0\n    for pos in reversed(keys(^books(1).tags))\n        total = total * 10 + pos\n    return total\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tagKeys"))
            .expect("tag keys")
            .value,
        Some(Value::Int(123))
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tagKeysRev"))
            .expect("tag keys")
            .value,
        Some(Value::Int(321))
    );
}

#[test]
fn count_over_a_composite_root_matches_direct_iteration() {
    let program = checked_program(
        "resource Cell at ^cells(x: int, y: int)\n    required value: int\n\npub fn put(x: int, y: int, value: int)\n    ^cells(x, y).value = value\n\npub fn countRoot(): int\n    return count(^cells)\n\npub fn iterRoot(): int\n    var n = 0\n    for cell in ^cells\n        n = n + 1\n    return n\n",
    );
    let store = TreeStore::memory();
    for (x, y, value) in [(1, 1, 11), (1, 2, 12), (2, 1, 21)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::put",
                Value::Int(x),
                Value::Int(y),
                Value::Int(value)
            ),
        )
        .expect("put");
    }
    let call = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry))
            .expect(entry)
            .value
    };
    assert_eq!(call("test::countRoot"), Some(Value::Int(3)));
    assert_eq!(call("test::iterRoot"), Some(Value::Int(3)));
}

#[test]
fn count_over_a_partial_index_branch_matches_direct_iteration() {
    let program = checked_program(&format!(
        "{ENROLLMENT_STATUS}\
pub fn activeCount(): int\n    return count(^enrollments.byStatus(\"active\"))\n\n\
pub fn activeCountForStudent(student: string): int\n    return count(^enrollments.byStatus(\"active\", student))\n\n\
pub fn iterActive(): int\n    var n = 0\n    for id in ^enrollments.byStatus(\"active\")\n        n = n + 1\n    return n\n\n\
pub fn iterActiveForStudent(student: string): int\n    var n = 0\n    for id in ^enrollments.byStatus(\"active\", student)\n        n = n + 1\n    return n\n"
    ));
    let store = TreeStore::memory();
    for (s, c, st) in [
        ("student-1", "course-8", "active"),
        ("student-1", "course-9", "active"),
        ("student-2", "course-8", "active"),
        ("student-2", "course-7", "dropped"),
    ] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    }
    let int = |entry: &str, arg: Option<&str>| {
        let entry = match arg {
            Some(student) => checked_entry!(&program, entry, Value::Str(student.into())),
            None => checked_entry!(&program, entry),
        };
        run_entry(&store, entry).expect("run").value
    };

    // Walking two index levels (studentId, then courseId) counts every active
    // enrollment, matching what direct iteration yields.
    assert_eq!(int("test::activeCount", None), Some(Value::Int(3)));
    assert_eq!(int("test::iterActive", None), Some(Value::Int(3)));

    // Pinning the first identity key narrows the walk to one level (courseId).
    assert_eq!(
        int("test::activeCountForStudent", Some("student-1")),
        Some(Value::Int(2))
    );
    assert_eq!(
        int("test::iterActiveForStudent", Some("student-1")),
        Some(Value::Int(2))
    );
    assert_eq!(
        int("test::activeCountForStudent", Some("student-2")),
        Some(Value::Int(1))
    );
}

/// A resource carrying both a keyed-leaf layer (`tags(pos: int): string`) and a
/// GROUP layer (`versions(version: int)` with member fields). Used to prove that
/// `exists`, `count`, and `std::assert::absent` agree with the actual stored path
/// for a keyed layer entry — the paths a record/field read or write lowers to.
const BOOK_KEYED_PRESENCE: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags(pos: int): string
    versions(version: int)
        required note: string

pub fn seed()
    ^books(1).title = \"Mort\"
    ^books(1).tags(1) = \"fiction\"
    ^books(1).versions(2).note = \"draft\"

pub fn tagExists(id: int, pos: int): bool
    return exists(^books(id).tags(pos))

pub fn versionExists(id: int, ver: int): bool
    return exists(^books(id).versions(ver))

pub fn tagCount(id: int): int
    return count(^books(id).tags)

pub fn versionFieldCount(id: int, ver: int): int
    return count(^books(id).versions(ver))

pub fn topLevelExists(id: int): bool
    return exists(^books(id).title)

pub fn topLevelCount(id: int): int
    return count(^books(id).title)

pub fn assertTagAbsent(id: int, pos: int)
    std::assert::absent(^books(id).tags(pos))

pub fn assertVersionAbsent(id: int, ver: int)
    std::assert::absent(^books(id).versions(ver))
";

// A keyed-leaf layer entry and a group-layer entry are stored under
// `ChildLayer`/`IndexKey` segments — the same shape a normal read or write
// lowers to. `exists`, `count`, and `std::assert::absent` must read that same
// path, not a record-key mis-encoding, so they agree byte-for-byte with what is
// actually stored.
#[test]
fn exists_count_and_assert_absent_agree_over_a_present_keyed_layer_entry() {
    let program = checked_program(BOOK_KEYED_PRESENCE);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let call = |entry: CheckedEntryCall| run_entry(&store, entry).expect("run").value;

    // A present keyed-leaf entry `^books(1).tags(1)` exists, and the layer counts
    // its one entry.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::tagExists",
            Value::Int(1),
            Value::Int(1)
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::tagCount", Value::Int(1))),
        Some(Value::Int(1))
    );

    // A present group entry `^books(1).versions(2)` exists (it carries a `note`
    // child), and counting it counts that one populated member.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::versionExists",
            Value::Int(1),
            Value::Int(2)
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::versionFieldCount",
            Value::Int(1),
            Value::Int(2)
        )),
        Some(Value::Int(1))
    );

    // `std::assert::absent` over either written entry is a failed assertion, not a
    // silent pass: the entry is present.
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertTagAbsent",
                Value::Int(1),
                Value::Int(1)
            ),
        ),
        RUN_ASSERT,
    );
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertVersionAbsent",
                Value::Int(1),
                Value::Int(2)
            ),
        ),
        RUN_ASSERT,
    );

    // An absent keyed-leaf entry and an absent group entry report absent: `exists`
    // is false and `std::assert::absent` passes.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::tagExists",
            Value::Int(1),
            Value::Int(9)
        )),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::versionExists",
            Value::Int(1),
            Value::Int(9)
        )),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertTagAbsent",
                Value::Int(1),
                Value::Int(9)
            )
        )
        .expect("absent tag passes")
        .value,
        None
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertVersionAbsent",
                Value::Int(1),
                Value::Int(9)
            )
        )
        .expect("absent version passes")
        .value,
        None
    );

    // The already-correct top-level-field shapes stay green: a present field
    // exists and counts as one.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::topLevelExists",
            Value::Int(1)
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::topLevelCount",
            Value::Int(1)
        )),
        Some(Value::Int(1))
    );
}

const NESTED_KEYED_LAYERS: &str = "\
resource Table at ^tables(name: string)
    rows(row: int)
        fields(col: int): string
        cells(col: int)
            required value: string

pub fn setField(table: string, row: int, col: int, value: string)
    ^tables(table).rows(row).fields(col) = value

pub fn addField(table: string, row: int, value: string): int
    return append(^tables(table).rows(row).fields, value)

pub fn fieldAt(table: string, row: int, col: int): string
    return ^tables(table).rows(row).fields(col) ?? \"\"

pub fn seedCells()
    ^tables(\"t\").rows(1).cells(1).value = \"a\"
    ^tables(\"t\").rows(1).cells(2).value = \"b\"

pub fn countCells(): int
    return count(^tables(\"t\").rows(1).cells)

pub fn iterateCells(): int
    var total: int = 0
    for cell in ^tables(\"t\").rows(1).cells
        total = total + 1
    return total

pub fn cellEntries()
    for col, cell in entries(^tables(\"t\").rows(1).cells)
        print($\"{col}={cell.value}\")
";

#[test]
fn nested_keyed_leaf_entries_write_append_and_read_back() {
    let program = checked_program(NESTED_KEYED_LAYERS);
    let store = TreeStore::memory();

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setField",
            Value::Str("t".into()),
            Value::Int(1),
            Value::Int(1),
            Value::Str("a".into()),
        ),
    )
    .expect("nested keyed-leaf write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::fieldAt",
                Value::Str("t".into()),
                Value::Int(1),
                Value::Int(1)
            )
        )
        .expect("read nested keyed leaf")
        .value,
        Some(Value::Str("a".into()))
    );

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::addField",
                Value::Str("t".into()),
                Value::Int(1),
                Value::Str("b".into())
            )
        )
        .expect("append nested keyed leaf")
        .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::fieldAt",
                Value::Str("t".into()),
                Value::Int(1),
                Value::Int(2)
            )
        )
        .expect("read appended nested keyed leaf")
        .value,
        Some(Value::Str("b".into()))
    );
}

#[test]
fn nested_keyed_group_layers_iterate_and_materialize_entries() {
    let program = checked_program(NESTED_KEYED_LAYERS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seedCells")).expect("seed cells");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countCells"))
            .expect("count nested cells")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::iterateCells"))
            .expect("iterate nested cells")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::cellEntries"))
            .expect("entries over nested cells")
            .output,
        "1=a\n2=b\n"
    );
}

#[test]
fn writing_a_nested_keyed_leaf_while_traversing_it_is_a_traversal_fault() {
    checker_rejects(
        "resource Table at ^tables(name: string)\n    rows(row: int)\n        fields(col: int): string\n\npub fn mutateNestedLeafDuringTraversal()\n    for col in keys(^tables(\"t\").rows(1).fields)\n        ^tables(\"t\").rows(1).fields(3) = \"c\"\n",
        "check.loop_mutates_traversed_layer",
    );
}

/// `values`/`entries` over a primary root materialize whole records; over a
/// keyed/sequence layer they materialize each entry's value. `entries` feeds the
/// two-name `for id, x in entries(...)` binding.
const BOOK_VALUES: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn tag(id: int, t: string): int
    return append(^books(id).tags, t)

pub fn titles()
    for book in values(^books)
        print(book.title)

pub fn idsAndTitles()
    for id, book in entries(^books)
        print($\"{id}: {book.title}\")

pub fn tagValues(id: int)
    for tag in values(^books(id).tags)
        print(tag)

pub fn tagEntries(id: int)
    for pos, tag in entries(^books(id).tags)
        print($\"{pos}={tag}\")

pub fn titlesReversed()
    for book in reversed(values(^books))
        print(book.title)

pub fn idsAndTitlesReversed()
    for id, book in reversed(entries(^books))
        print($\"{id}: {book.title}\")

pub fn tagValuesReversed(id: int)
    for tag in reversed(values(^books(id).tags))
        print(tag)

pub fn tagEntriesReversed(id: int)
    for pos, tag in reversed(entries(^books(id).tags))
        print($\"{pos}={tag}\")

pub fn titlesValue()
    const books = values(^books)
    for book in books
        print(book.title)

pub fn titleEntriesValue()
    const books = entries(^books)
    for id, book in books
        print($\"{id}: {book.title}\")

pub fn tagValuesValue(id: int)
    const tags = values(^books(id).tags)
    for tag in tags
        print(tag)

pub fn tagEntriesValue(id: int)
    const tags = entries(^books(id).tags)
    for pos, tag in tags
        print($\"{pos}={tag}\")
";

#[test]
fn values_and_entries_materialize_whole_records_over_a_primary_root() {
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    let add = |id: i64, t: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::add", Value::Int(id), Value::Str(t.into())),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    // `values(^books)` yields each whole record, in key order, with field access.
    let titles = run_entry(&store, checked_entry!(&program, "test::titles")).expect("run");
    assert_eq!(titles.output, "Mort\nSourcery\n");

    // `entries(^books)` binds the identity and the materialized record together.
    let pairs = run_entry(&store, checked_entry!(&program, "test::idsAndTitles")).expect("run");
    assert_eq!(pairs.output, "1: Mort\n2: Sourcery\n");
}

#[test]
fn values_and_entries_materialize_entries_over_a_keyed_layer() {
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::tag",
            Value::Int(1),
            Value::Str("fiction".into())
        ),
    )
    .expect("tag");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::tag",
            Value::Int(1),
            Value::Str("funny".into())
        ),
    )
    .expect("tag");

    // `values(^books(1).tags)` yields each leaf value in key order.
    let values = run_entry(
        &store,
        checked_entry!(&program, "test::tagValues", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(values.output, "fiction\nfunny\n");

    // `entries(...)` binds each 1-based position to its leaf value.
    let entries = run_entry(
        &store,
        checked_entry!(&program, "test::tagEntries", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(entries.output, "1=fiction\n2=funny\n");
}

#[test]
fn saved_values_and_entries_as_values_are_rejected() {
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::tag",
            Value::Int(1),
            Value::Str("fiction".into())
        ),
    )
    .expect("tag");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::titlesValue")),
        RUN_UNSUPPORTED,
    );
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::titleEntriesValue")),
        RUN_UNSUPPORTED,
    );
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::tagValuesValue", Value::Int(1)),
        ),
        RUN_UNSUPPORTED,
    );
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::tagEntriesValue", Value::Int(1)),
        ),
        RUN_UNSUPPORTED,
    );
}

#[test]
fn reversed_values_and_entries_bind_values_and_pairs_descending() {
    // `for x in reversed(values(L))` must bind whole values descending — not the
    // bare child keys. Likewise `for k, v in reversed(entries(L))` binds (key,
    // value) pairs descending, not key-only segments (which would runtime-error).
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    let add = |id: i64, t: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::add", Value::Int(id), Value::Str(t.into())),
        )
        .expect("add");
    };
    add(1, "Mort");
    add(2, "Sourcery");

    // `reversed(values(^books))` yields whole records in descending key order.
    let titles = run_entry(&store, checked_entry!(&program, "test::titlesReversed")).expect("run");
    assert_eq!(titles.output, "Sourcery\nMort\n");

    // `reversed(entries(^books))` binds (identity, record) pairs descending.
    let pairs = run_entry(
        &store,
        checked_entry!(&program, "test::idsAndTitlesReversed"),
    )
    .expect("run");
    assert_eq!(pairs.output, "2: Sourcery\n1: Mort\n");
}

#[test]
fn reversed_values_and_entries_over_a_keyed_layer_descend() {
    // The same shaping over a keyed/sequence child layer: values and (pos, value)
    // pairs descend by key, rather than collapsing to bare position keys.
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    for tag in ["fiction", "funny", "fantasy"] {
        run_entry(
            &store,
            checked_entry!(&program, "test::tag", Value::Int(1), Value::Str(tag.into())),
        )
        .expect("tag");
    }

    // `reversed(values(^books(1).tags))` yields each leaf value in descending order.
    let values = run_entry(
        &store,
        checked_entry!(&program, "test::tagValuesReversed", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(values.output, "fantasy\nfunny\nfiction\n");

    // `reversed(entries(...))` binds each position to its value, descending.
    let entries = run_entry(
        &store,
        checked_entry!(&program, "test::tagEntriesReversed", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(entries.output, "3=fantasy\n2=funny\n1=fiction\n");
}

const BOOK_ISBN_SAVE: &str = "\
module test
resource Book at ^books(id: int)
    isbn: string
    index byIsbn(isbn) unique
fn save(i: int, code: string)
    ^books(i).isbn = code
";

#[test]
fn a_recoverable_write_fault_is_catchable_across_a_call_boundary() {
    // A write fault raised in a CALLED function must be catchable by the caller's
    // try/catch (the transaction-recovery contract), not only within the same frame.
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run(): string\n\
         \x20   save(1, \"x\")\n\
         \x20   try\n\
         \x20       save(2, \"x\")\n\
         \x20       return \"uncaught\"\n\
         \x20   catch e: Error\n\
         \x20       return e.code\n"
    ));
    let store = TreeStore::memory();
    let value = run_entry(&store, checked_entry!(&program, "test::run"))
        .expect("run")
        .value;
    assert_eq!(value, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn an_uncaught_cross_boundary_write_fault_keeps_its_dotted_code() {
    // Crossing a call boundary must not collapse an uncaught fault to
    // run.uncaught_error: it surfaces with its own dotted code (and exit code).
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run()\n\
         \x20   save(1, \"x\")\n\
         \x20   save(2, \"x\")\n"
    ));
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::run")),
        "write.unique_conflict",
    );
}

const PATIENT_SPARSE_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    name
        first: string
        last: string
";

const PATIENT_REQUIRED_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    name
        required first: string
        last: string
";

const PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    visits(pos: int)
        name
            required first: string
            last: string

pub fn seed()
    ^patients(\"p1\").visits(1).name.first = \"Sam\"
    ^patients(\"p1\").visits(1).name.last = \"Vimes\"

pub fn drop()
    delete ^patients(\"p1\").visits(1).name

pub fn visit_first(): string
    return ^patients(\"p1\").visits(1).name.first ?? \"\"
";

#[test]
fn deleting_a_sparse_field_inside_an_unkeyed_group_is_allowed() {
    // Field delete descends unkeyed-group layers. Sparse descendants may still be
    // deleted independently.
    let program = checked_program(&format!(
        "{PATIENT_SPARSE_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.last\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::drop"))
        .expect("sparse group-field delete is a no-op");
}

#[test]
fn deleting_a_required_field_inside_an_unkeyed_group_is_rejected() {
    let program = checked_program(&format!(
        "{PATIENT_REQUIRED_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.first\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert_run_error(result, "write.required_field");
}

#[test]
fn deleting_an_unkeyed_group_with_required_descendants_is_rejected() {
    let program = checked_program(&format!(
        "{PATIENT_REQUIRED_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert_run_error(result, "write.required_field");
}

#[test]
fn deleting_a_nested_unkeyed_group_with_required_descendants_is_rejected() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert_run_error(result, "write.required_field");
}

#[test]
fn maintenance_can_delete_a_nested_unkeyed_group_with_required_descendants() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::drop")).expect("drop");
    for field in ["first", "last"] {
        assert_eq!(
            read_data_bytes(
                &program,
                &store,
                "patients",
                &[SavedKey::Str("p1".into())],
                &keyed_data_path(
                    &program,
                    "patients",
                    &[("visits", vec![SavedKey::Int(1)])],
                    &["name", field],
                ),
            ),
            None,
            "{field} removed under maintenance"
        );
    }
}

#[test]
fn keyed_group_entry_read_materializes_unkeyed_group_descendants() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let outcome = run_entry(&store, checked_entry!(&program, "test::visit_first")).expect("read");
    assert_eq!(outcome.value, Some(Value::Str("Sam".into())));
}
