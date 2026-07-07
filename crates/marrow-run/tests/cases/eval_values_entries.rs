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
    for col, cell in ^tables(\"t\").rows(1).cells
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
        "resource Table\n    rows(row: int)\n        fields(col: int): string\nstore ^tables(name: string): Table\n\npub fn mutateNestedLeafDuringTraversal()\n    for col in ^tables(\"t\").rows(1).fields\n        ^tables(\"t\").rows(1).fields(3) = \"c\"\n",
        "check.loop_mutates_traversed_layer",
    );
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
