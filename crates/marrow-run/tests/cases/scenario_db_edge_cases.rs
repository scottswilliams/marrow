//! Tier-2 scenarios over the production runtime pipeline that probe the durable
//! store at its boundaries: empty and first-run reads, single/many/deeply-nested
//! entries, the three shapes of `delete` (whole root, one keyed-layer entry, one
//! field), unique-index fail-closed writes, and counting over 0/1/many records.
//!
//! Each scenario characterizes current v0.1 behavior with typed oracles: runtime
//! `Value`s, typed `RuntimeError` codes, and direct store effects read back through
//! `read_data_value`.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::value::{SavedValue, ScalarType};

/// A keyed `Book` store with a counting entry that streams the whole root, used to
/// characterize aggregation at the 0/1/many boundaries.
const BOOK_LEDGER: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn total(): int
    var c = 0
    for id in ^books
        c = c + 1
    return c

pub fn titleOf(id: int): string
    return ^books(id).title ?? \"<absent>\"
";

#[test]
fn iterating_an_empty_root_yields_zero_records() {
    // A first-run store has no entries, so streaming the root visits nothing.
    let program = checked_program(BOOK_LEDGER);
    let store = empty_store();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::total"))
            .expect("count")
            .value,
        Some(Value::Int(0)),
    );
}

#[test]
fn counting_over_saved_records_tracks_zero_one_and_many() {
    // The aggregate moves through its boundaries as records are added.
    let program = checked_program(BOOK_LEDGER);
    let store = empty_store();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::total"))
            .expect("zero")
            .value,
        Some(Value::Int(0)),
    );
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("one");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::total"))
            .expect("one")
            .value,
        Some(Value::Int(1)),
    );
    for (id, title) in [(2, "Pyramids"), (3, "Guards"), (4, "Hogfather")] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("many");
    }
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::total"))
            .expect("many")
            .value,
        Some(Value::Int(4)),
    );
}

#[test]
fn reading_a_never_written_field_is_absent_not_an_error_in_the_store() {
    // A direct store read of a path that was never written returns absence rather
    // than a stored value; the runtime read site chooses its own explicit fallback.
    let program = checked_program(BOOK_LEDGER);
    let store = empty_store();
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(7)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        None,
        "the store holds no bytes at a never-written path",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(7)),
        )
        .expect("fallback")
        .value,
        Some(Value::Str("<absent>".into())),
    );
}

#[test]
fn a_single_keyed_entry_reads_back_and_then_many_coexist() {
    // One keyed entry round-trips on its own, and once siblings are added each entry
    // keeps its own value under its own identity key.
    let program = checked_program(BOOK_LEDGER);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("single");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into())),
    );
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(2),
            Value::Str("Pyramids".into())
        ),
    )
    .expect("second");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into())),
        "the first entry is untouched by the second write",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("Pyramids".into())),
    );
}

#[test]
fn a_deeply_nested_group_reads_back_through_its_full_path() {
    // A group nested inside a group (`address.geo.lat`) stores its leaf at the full
    // path and reads back both through the runtime and through a direct store read.
    let program = checked_program(
        "resource Org\n    address\n        geo\n            lat: string\n            lon: string\nstore ^orgs(id: int): Org\n\n\
         pub fn setGeo(id: int, lat: string, lon: string)\n    ^orgs(id).address.geo.lat = lat\n    ^orgs(id).address.geo.lon = lon\n\n\
         pub fn latOf(id: int): string\n    return ^orgs(id).address.geo.lat ?? \"<absent>\"\n",
    );
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setGeo",
            Value::Int(1),
            Value::Str("51.5".into()),
            Value::Str("-0.1".into())
        ),
    )
    .expect("write nested");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::latOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("51.5".into())),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "orgs",
            &[SavedKey::Int(1)],
            &data_path(&program, "orgs", &["address", "geo", "lat"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("51.5".into())),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "orgs",
            &[SavedKey::Int(1)],
            &data_path(&program, "orgs", &["address", "geo", "lon"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("-0.1".into())),
    );
}

/// A shelf whose `books` keyed leaf layer holds positional string entries, used to
/// exercise first/last/missing reads and per-entry delete.
const SHELF_LAYER: &str = "\
resource Shelf
    books(pos: int): string
store ^shelves(id: int): Shelf

pub fn put(id: int, pos: int, t: string)
    ^shelves(id).books(pos) = t

pub fn removeAt(id: int, pos: int)
    delete ^shelves(id).books(pos)

pub fn bookAt(id: int, pos: int): string
    return ^shelves(id).books(pos) ?? \"<none>\"

pub fn total(id: int): int
    return count(^shelves(id).books)
";

#[test]
fn keyed_layer_reads_first_last_and_a_missing_position() {
    // Across a populated keyed layer, the lowest and highest stored positions read
    // back, and a position that was never written reads as absent.
    let program = checked_program(SHELF_LAYER);
    let store = empty_store();
    for (pos, title) in [(1, "a"), (2, "b"), (3, "c")] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::put",
                Value::Int(1),
                Value::Int(pos),
                Value::Str(title.into())
            ),
        )
        .expect("put");
    }
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::total", Value::Int(1))
        )
        .expect("count")
        .value,
        Some(Value::Int(3)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::bookAt", Value::Int(1), Value::Int(1))
        )
        .expect("first")
        .value,
        Some(Value::Str("a".into())),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::bookAt", Value::Int(1), Value::Int(3))
        )
        .expect("last")
        .value,
        Some(Value::Str("c".into())),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::bookAt", Value::Int(1), Value::Int(99))
        )
        .expect("missing")
        .value,
        Some(Value::Str("<none>".into())),
    );
}

#[test]
fn deleting_one_keyed_layer_entry_leaves_its_siblings() {
    // Removing the middle position drops the count by one, turns that position
    // absent, and leaves the surrounding positions intact.
    let program = checked_program(SHELF_LAYER);
    let store = empty_store();
    for (pos, title) in [(1, "a"), (2, "b"), (3, "c")] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::put",
                Value::Int(1),
                Value::Int(pos),
                Value::Str(title.into())
            ),
        )
        .expect("put");
    }
    run_entry(
        &store,
        checked_entry!(&program, "test::removeAt", Value::Int(1), Value::Int(2)),
    )
    .expect("delete one");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::total", Value::Int(1))
        )
        .expect("count")
        .value,
        Some(Value::Int(2)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::bookAt", Value::Int(1), Value::Int(2))
        )
        .expect("gone")
        .value,
        Some(Value::Str("<none>".into())),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::bookAt", Value::Int(1), Value::Int(1))
        )
        .expect("sibling")
        .value,
        Some(Value::Str("a".into())),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::bookAt", Value::Int(1), Value::Int(3))
        )
        .expect("sibling")
        .value,
        Some(Value::Str("c".into())),
    );
}

/// A book with two scalar fields, used to separate whole-root delete from
/// single-field delete.
const BOOK_FIELDS: &str = "\
resource Book
    title: string
    shelf: string
store ^books(id: int): Book

pub fn put(id: int)
    ^books(id).title = \"Mort\"
    ^books(id).shelf = \"fiction\"

pub fn dropRoot(id: int)
    delete ^books(id)

pub fn dropShelf(id: int)
    delete ^books(id).shelf

pub fn has(id: int): bool
    return exists(^books(id))

pub fn titleOf(id: int): string
    return ^books(id).title ?? \"<none>\"

pub fn shelfOf(id: int): string
    return ^books(id).shelf ?? \"<none>\"
";

#[test]
fn whole_root_delete_removes_the_record() {
    // After `delete ^books(id)` the identity no longer exists and its fields are
    // gone from the store.
    let program = checked_program(BOOK_FIELDS);
    let store = empty_store();
    run_entry(&store, checked_entry!(&program, "test::put", Value::Int(1))).expect("put");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::has", Value::Int(1)))
            .expect("present")
            .value,
        Some(Value::Bool(true)),
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::dropRoot", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::has", Value::Int(1)))
            .expect("absent")
            .value,
        Some(Value::Bool(false)),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        None,
        "the deleted record's fields are gone from the store",
    );
}

#[test]
fn deleting_a_single_field_leaves_the_rest_of_the_record() {
    // `delete ^books(id).shelf` removes only that field; `title` and the record
    // itself survive.
    let program = checked_program(BOOK_FIELDS);
    let store = empty_store();
    run_entry(&store, checked_entry!(&program, "test::put", Value::Int(1))).expect("put");
    run_entry(
        &store,
        checked_entry!(&program, "test::dropShelf", Value::Int(1)),
    )
    .expect("delete");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::shelfOf", Value::Int(1))
        )
        .expect("gone")
        .value,
        Some(Value::Str("<none>".into())),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(1))
        )
        .expect("kept")
        .value,
        Some(Value::Str("Mort".into())),
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::has", Value::Int(1)))
            .expect("present")
            .value,
        Some(Value::Bool(true)),
        "the record still exists after a single-field delete",
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["shelf"]),
            ScalarType::Str,
        ),
        None,
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Mort".into())),
    );
}

/// A book with a unique `isbn` index plus an owner lookup, used to characterize
/// fail-closed behavior on a duplicate unique key.
const UNIQUE_ISBN: &str = "\
resource Book
    required title: string
    isbn: string
store ^books(id: int): Book

    index byIsbn(isbn) unique

pub fn seed(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

pub fn claim(id: int, isbn: string)
    ^books(id).isbn = isbn

pub fn isbnOf(id: int): string
    return ^books(id).isbn ?? \"<none>\"

pub fn ownerOf(isbn: string): Id(^books)
    for id in ^books.byIsbn(isbn)
        return id
    throw Error(code: \"test.no_owner\", message: \"no owner for isbn\")
";

#[test]
fn a_duplicate_unique_key_fails_closed_and_leaves_the_store_unchanged() {
    // Writing an isbn already owned by another record rejects with the typed
    // unique-conflict code, and neither the conflicting record's field nor the
    // unique index moves: the original owner is intact.
    let program = checked_program(UNIQUE_ISBN);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("i1".into())
        ),
    )
    .expect("seed 1");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("i2".into())
        ),
    )
    .expect("seed 2");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::claim",
                Value::Int(2),
                Value::Str("i1".into())
            ),
        ),
        "write.unique_conflict",
    );

    // The rejected write left no trace: book 2 keeps its own isbn in the store.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(2)],
            &data_path(&program, "books", &["isbn"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("i2".into())),
    );
    // The unique index still resolves the conflicting key to its original owner.
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::ownerOf", Value::Str("i1".into())),
        )
        .expect("owner")
        .value,
        "books",
        &[SavedKey::Int(1)],
    );
}
