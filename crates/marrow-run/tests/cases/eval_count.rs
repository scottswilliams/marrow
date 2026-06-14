//! `count` over the presence shapes: scalar and child paths, index branches,
//! primary and composite roots, and agreement with `exists` / `std::assert::absent`.

use crate::support;
use support::*;

use marrow_run::{CheckedEntryCall, RUN_ASSERT, Value};
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

    // A layer carrying BOTH a self-value and children counts only its immediate
    // children, not children-plus-one. The runtime never writes a value at a
    // sequence-layer path, so this edge is seeded at the store directly.
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["tags"]),
        SavedValue::Str("self".into()),
    );
    assert_eq!(count("test::countTags"), Some(Value::Int(2)));
}

/// `count` over a declared index branch returns the number of entries under that
/// branch, exactly as `keys(...)` over the same branch would yield. The branch is
/// a non-unique index so several entries share one query key.
const BOOK_COUNT_INDEX: &str = "\
resource Book
    required title: string
    shelf: string
    tags: sequence[string]
store ^books(id: int): Book

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
        "resource Book\n    required title: string\n    shelf: string\n    isbn: string\nstore ^books(id: int): Book\n\n    index byShelf(shelf, id)\n    index byIsbn(isbn) unique\n\npub fn add(id: int, t: string, s: string)\n    ^books(id).title = t\n    ^books(id).shelf = s\n\npub fn addIsbn(id: int, isbn: string)\n    ^books(id).isbn = isbn\n\npub fn countRoot(): int\n    return count(^books)\n\npub fn iterRoot(): int\n    var n = 0\n    for book in ^books\n        n = n + 1\n    return n\n",
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
fn count_over_a_saved_root_matches_direct_iteration() {
    // A root count equals the record count direct iteration yields, for both a
    // primary single-key root and a composite-key root.
    let primary = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n    ^books(3).title = \"C\"\n\npub fn countRoot(): int\n    return count(^books)\n\npub fn iterRoot(): int\n    var n = 0\n    for book in ^books\n        n = n + 1\n    return n\n"
    ));
    let call = |store: &TreeStore, program, entry: &str| {
        run_entry(store, checked_entry!(program, entry))
            .expect(entry)
            .value
    };

    let primary_store = TreeStore::memory();
    run_entry(&primary_store, checked_entry!(&primary, "test::seed")).expect("seed");
    assert_eq!(
        call(&primary_store, &primary, "test::countRoot"),
        Some(Value::Int(3))
    );
    assert_eq!(
        call(&primary_store, &primary, "test::iterRoot"),
        Some(Value::Int(3))
    );

    let composite = checked_program(
        "resource Cell\n    required value: int\nstore ^cells(x: int, y: int): Cell\n\npub fn put(x: int, y: int, value: int)\n    ^cells(x, y).value = value\n\npub fn countRoot(): int\n    return count(^cells)\n\npub fn iterRoot(): int\n    var n = 0\n    for cell in ^cells\n        n = n + 1\n    return n\n",
    );
    let composite_store = TreeStore::memory();
    for (x, y, value) in [(1, 1, 11), (1, 2, 12), (2, 1, 21)] {
        run_entry(
            &composite_store,
            checked_entry!(
                &composite,
                "test::put",
                Value::Int(x),
                Value::Int(y),
                Value::Int(value)
            ),
        )
        .expect("put");
    }
    assert_eq!(
        call(&composite_store, &composite, "test::countRoot"),
        Some(Value::Int(3))
    );
    assert_eq!(
        call(&composite_store, &composite, "test::iterRoot"),
        Some(Value::Int(3))
    );
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
resource Book
    required title: string
    tags(pos: int): string
    versions(version: int)
        required note: string
store ^books(id: int): Book

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
