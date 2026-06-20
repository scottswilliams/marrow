//! `values` / `entries` materialization over primary roots and keyed child
//! layers, forward and reversed, plus nested keyed-leaf and keyed-group layers.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::tree::TreeStore;

const NESTED_KEYED_LAYERS: &str = "\
resource Table
    rows(row: int)
        fields(col: int): string
        cells(col: int)
            required value: string
store ^tables(name: string): Table

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
        "resource Table\n    rows(row: int)\n        fields(col: int): string\nstore ^tables(name: string): Table\n\npub fn mutateNestedLeafDuringTraversal()\n    for col in keys(^tables(\"t\").rows(1).fields)\n        ^tables(\"t\").rows(1).fields(3) = \"c\"\n",
        "check.loop_mutates_traversed_layer",
    );
}

/// `values`/`entries` over a primary root materialize whole records; over a
/// keyed/sequence layer they materialize each entry's value. `entries` feeds the
/// two-name `for id, x in entries(...)` binding.
const BOOK_VALUES: &str = "\
resource Book
    required title: string
    tags: sequence[string]
store ^books(id: int): Book

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
fn saved_values_as_values_are_checker_rejected() {
    // Binding `values(^books)` or `values(^books(id).tags)` to a local materializes a
    // saved collection — an in-place stream with no local value. Both are check errors,
    // not runtime faults.
    checker_rejects(
        "resource Book\n    required title: string\n    tags: sequence[string]\nstore ^books(id: int): Book\n\npub fn titlesValue()\n    const books = values(^books)\n    for book in books\n        print(book.title)\n",
        "check.collection_unsupported",
    );
    checker_rejects(
        "resource Book\n    required title: string\n    tags: sequence[string]\nstore ^books(id: int): Book\n\npub fn tagValuesValue(id: int)\n    const tags = values(^books(id).tags)\n    for tag in tags\n        print(tag)\n",
        "check.collection_unsupported",
    );
}

#[test]
fn saved_entries_as_values_are_checker_rejected() {
    checker_rejects(
        "resource Book\n    required title: string\n    tags: sequence[string]\nstore ^books(id: int): Book\n\npub fn titleEntriesValue()\n    const books = entries(^books)\n    for id, book in books\n        print($\"{id}: {book.title}\")\n",
        "check.collection_unsupported",
    );
    checker_rejects(
        "resource Book\n    required title: string\n    tags: sequence[string]\nstore ^books(id: int): Book\n\npub fn tagEntriesValue(id: int)\n    const tags = entries(^books(id).tags)\n    for pos, tag in tags\n        print($\"{pos}={tag}\")\n",
        "check.collection_unsupported",
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
