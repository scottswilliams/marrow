//! Direct saved-root and saved-layer streaming loops: ids in store order, live
//! read-your-writes during iteration, early return before later records, and
//! `values` / `entries` short-circuiting before a malformed later row.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

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
        "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n\npub fn mutateFutureValue(): int\n    var total = 0\n    for id in ^books\n        if const title = ^books(id).title\n            if title == \"A\"\n                total = total * 10 + 1\n                ^books(2).title = \"Z\"\n            else if title == \"B\"\n                total = total * 10 + 2\n            else if title == \"Z\"\n                total = total * 10 + 9\n    return total\n"
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
fn bounded_saved_root_keyspace_streams_matching_keys() {
    let program = checked_program(
        "resource Cell\n    required value: int\nstore ^cells(x: int, y: int): Cell\n\npub fn seed(x: int, y: int, value: int)\n    ^cells(x, y).value = value\n\npub fn ysBetween(x: int, lo: int, hi: int)\n    for y in ^cells(x, lo..hi)\n        print($\"{y}\")\n\npub fn ysBetweenKeys(x: int, lo: int, hi: int)\n    for y in keys(^cells(x, lo..hi))\n        print($\"{y}\")\n\npub fn valuesBetween(x: int, lo: int, hi: int)\n    for cell in values(^cells(x, lo..hi))\n        print($\"{cell.value}\")\n\npub fn entriesBetween(x: int, lo: int, hi: int)\n    for y, cell in entries(^cells(x, lo..hi))\n        print($\"{y}={cell.value}\")\n\npub fn ysBetweenRev(x: int, lo: int, hi: int)\n    for y in reversed(^cells(x, lo..hi))\n        print($\"{y}\")\n",
    );
    let store = TreeStore::memory();
    for (x, y, value) in [(1, 1, 10), (1, 2, 20), (1, 3, 30), (1, 4, 40), (2, 2, 200)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::seed",
                Value::Int(x),
                Value::Int(y),
                Value::Int(value)
            ),
        )
        .expect("seed");
    }

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::ysBetween",
                Value::Int(1),
                Value::Int(2),
                Value::Int(4)
            )
        )
        .expect("bounded root")
        .output,
        "2\n3\n"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::valuesBetween",
                Value::Int(1),
                Value::Int(2),
                Value::Int(4)
            )
        )
        .expect("bounded root values")
        .output,
        "20\n30\n"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::entriesBetween",
                Value::Int(1),
                Value::Int(2),
                Value::Int(4)
            )
        )
        .expect("bounded root entries")
        .output,
        "2=20\n3=30\n"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::ysBetweenKeys",
                Value::Int(1),
                Value::Int(2),
                Value::Int(4)
            )
        )
        .expect("bounded root keys")
        .output,
        "2\n3\n"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::ysBetweenRev",
                Value::Int(1),
                Value::Int(2),
                Value::Int(4)
            )
        )
        .expect("bounded root reversed")
        .output,
        "3\n2\n"
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
        "resource Book\n    required title: string\n    note: string\nstore ^books(id: int): Book\n\npub fn seed()\n    ^books(1).title = \"Mort\"\n\npub fn firstTitle(): string\n    for book in values(^books)\n        return book.title\n    return \"\"\n",
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
        "resource Book\n    required title: string\n    note: string\nstore ^books(id: int): Book\n\npub fn seed()\n    ^books(1).title = \"Mort\"\n\npub fn firstTitle(): string\n    for id, book in ^books\n        return $\"{id}: {book.title}\"\n    return \"\"\n",
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
