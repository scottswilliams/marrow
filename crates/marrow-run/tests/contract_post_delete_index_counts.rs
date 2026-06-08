//! After bulk and partial-prefix deletes, the non-unique index reflects exactly
//! the surviving records: `count` over an index branch, `count` over the whole
//! root, and the identities a branch loop yields all drop the deleted records and
//! keep the survivors. This pins index-maintenance accuracy under deletion, where
//! a missed teardown would leave a stale branch entry that still resolves to a
//! deleted record.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;
use marrow_store::tree::TreeStore;

/// Books on shelves, indexed by `(shelf, id)`. Whole-record `delete` must tear
/// down the index entry the record fed, so a branch count and the root count both
/// track only the survivors. `idsOnShelf` prints the surviving records under a
/// branch so an exclusion (a deleted id still resolving) is observable, not just a
/// count that could coincidentally match.
const SHELVED_BOOKS: &str = "\
resource Book at ^books(id: int)
    required title: string
    required shelf: string

    index byShelf(shelf, id)

pub fn add(id: int, t: string, s: string)
    var b: Book
    b.title = t
    b.shelf = s
    ^books(id) = b

pub fn remove(id: int)
    delete ^books(id)

pub fn countShelf(shelf: string): int
    return count(^books.byShelf(shelf))

pub fn countRoot(): int
    return count(^books)

pub fn idsOnShelf(shelf: string)
    for id in ^books.byShelf(shelf)
        print($\"{^books(id).title}\")
";

fn populated_store(program: &marrow_check::CheckedRuntimeProgram) -> TreeStore {
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into())
            ),
        )
        .expect("add book");
    };
    // Three on `fiction`, two on `history`.
    add(1, "Mort", "fiction");
    add(2, "Reaper", "fiction");
    add(3, "Pyramids", "fiction");
    add(4, "Rome", "history");
    add(5, "Athens", "history");
    store
}

fn count_shelf(
    program: &marrow_check::CheckedRuntimeProgram,
    store: &TreeStore,
    shelf: &str,
) -> i64 {
    let Some(Value::Int(n)) = run_entry(
        store,
        checked_entry!(program, "test::countShelf", Value::Str(shelf.into())),
    )
    .expect("count shelf")
    .value
    else {
        panic!("countShelf returns an int");
    };
    n
}

fn count_root(program: &marrow_check::CheckedRuntimeProgram, store: &TreeStore) -> i64 {
    let Some(Value::Int(n)) = run_entry(store, checked_entry!(program, "test::countRoot"))
        .expect("count root")
        .value
    else {
        panic!("countRoot returns an int");
    };
    n
}

fn remove(program: &marrow_check::CheckedRuntimeProgram, store: &TreeStore, id: i64) {
    run_entry(
        store,
        checked_entry!(program, "test::remove", Value::Int(id)),
    )
    .expect("remove");
}

#[test]
fn a_bulk_delete_of_several_records_drops_their_index_entries() {
    // Deleting two of the three `fiction` records leaves exactly one under the
    // branch and four under the root. The branch count is not the pre-delete count.
    let program = checked_program(SHELVED_BOOKS);
    let store = populated_store(&program);
    assert_eq!(count_shelf(&program, &store, "fiction"), 3);
    assert_eq!(count_root(&program, &store), 5);

    remove(&program, &store, 1);
    remove(&program, &store, 2);

    assert_eq!(
        count_shelf(&program, &store, "fiction"),
        1,
        "only the surviving fiction record remains in the index branch",
    );
    assert_eq!(count_root(&program, &store), 3);

    // The surviving fiction record is the one that was not deleted, and the deleted
    // titles no longer resolve through the branch.
    let survivors = run_entry(
        &store,
        checked_entry!(&program, "test::idsOnShelf", Value::Str("fiction".into())),
    )
    .expect("iterate survivors")
    .output;
    assert_eq!(survivors, "Pyramids\n");
}

#[test]
fn deleting_every_record_in_a_prefix_empties_that_branch_only() {
    // Deleting every `history` record empties that branch entirely while the
    // untouched `fiction` branch keeps its full count: a prefix delete is scoped to
    // its branch and does not perturb a sibling branch's index entries.
    let program = checked_program(SHELVED_BOOKS);
    let store = populated_store(&program);

    remove(&program, &store, 4);
    remove(&program, &store, 5);

    assert_eq!(
        count_shelf(&program, &store, "history"),
        0,
        "the fully-deleted prefix branch is empty",
    );
    assert_eq!(
        count_shelf(&program, &store, "fiction"),
        3,
        "the sibling branch is untouched by the prefix delete",
    );
    assert_eq!(count_root(&program, &store), 3);

    // The emptied branch yields nothing; the sibling still yields its records.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::idsOnShelf", Value::Str("history".into())),
        )
        .expect("iterate empty branch")
        .output,
        "",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::idsOnShelf", Value::Str("fiction".into())),
        )
        .expect("iterate sibling branch")
        .output,
        "Mort\nReaper\nPyramids\n",
    );
}

#[test]
fn the_root_and_every_branch_are_empty_after_deleting_all_records() {
    // Deleting all records leaves both index branches and the root at zero — no
    // orphaned index entry survives a record it pointed at.
    let program = checked_program(SHELVED_BOOKS);
    let store = populated_store(&program);

    for id in 1..=5 {
        remove(&program, &store, id);
    }

    assert_eq!(count_root(&program, &store), 0);
    assert_eq!(count_shelf(&program, &store, "fiction"), 0);
    assert_eq!(count_shelf(&program, &store, "history"), 0);
}
