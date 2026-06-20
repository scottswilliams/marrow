//! Saved-layer enumeration: primary and composite keyed roots, sequence and
//! keyed child layers, and keyed group layers, by direct and two-name loops.

use crate::support;
use support::*;

use marrow_run::{RUN_UNSUPPORTED, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

fn book_primary() -> String {
    format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn titles()
    for id, book in ^books
        print(book.title)

pub fn directIds()
    for id in ^books
        print($\"{{id}}\")

pub fn idsAndElementTitles()
    for id, book in ^books
        print($\"{{id}}: {{book.title}}\")

pub fn reversedElementTitles()
    for id, book in reversed(^books)
        print(book.title)

pub fn reversedIdsAsValue()
    const ids = reversed(^books)
    for id in ids
        print($\"{{id}}\")

pub fn ids()
    const all = keys(^books)
    for id in all
        print($\"{{id}}\")
"
    )
}

#[test]
fn iterates_a_primary_keyed_root() {
    let program = checked_program(&book_primary());
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    // Direct root iteration yields ids in key order, each addressing its record.
    let outcome = run_entry(&store, checked_entry!(&program, "test::titles")).expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

#[test]
fn primary_root_loop_yields_identities() {
    let program = checked_program(&book_primary());
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let outcome = run_entry(&store, checked_entry!(&program, "test::directIds")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

#[test]
fn two_name_primary_root_loop_yields_id_and_resource() {
    let program = checked_program(&book_primary());
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::idsAndElementTitles"),
    )
    .expect("run");
    assert_eq!(outcome.output, "1: Mort\n2: Sourcery\n");
}

#[test]
fn reversed_two_name_primary_root_loop_yields_resources() {
    let program = checked_program(&book_primary());
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::reversedElementTitles"),
    )
    .expect("run");
    assert_eq!(outcome.output, "Sourcery\nMort\n");
}

#[test]
fn reversed_primary_root_as_a_value_is_rejected() {
    let program = checked_program(&book_primary());
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::reversedIdsAsValue")),
        RUN_UNSUPPORTED,
    );
}

#[test]
fn keys_of_a_primary_root_as_a_value_is_rejected() {
    let program = checked_program(&book_primary());
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
            "test::add",
            Value::Int(2),
            Value::Str("Sourcery".into())
        ),
    )
    .expect("add");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::ids")),
        RUN_UNSUPPORTED,
    );
}

/// Iterating a composite primary root yields materialized records in identity order.
const ENROLLMENT_PRIMARY: &str = "\
resource Enrollment
    status: string
store ^enrollments(studentId: string, courseId: string): Enrollment

pub fn enroll(s: string, c: string, st: string)
    ^enrollments(s, c).status = st

pub fn statuses()
    for id, enrollment in ^enrollments
        print(enrollment.status ?? \"\")
";

#[test]
fn iterates_a_composite_primary_root() {
    let program = checked_program(ENROLLMENT_PRIMARY);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
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
    };
    enroll("student-1", "course-9", "active");
    enroll("student-2", "course-1", "dropped");

    // Two-name iteration reads each materialized enrollment record.
    let outcome = run_entry(&store, checked_entry!(&program, "test::statuses")).expect("run");
    assert_eq!(outcome.output, "active\ndropped\n");
}

#[test]
fn reversed_over_a_composite_root_is_a_true_reverse() {
    // A composite identity reverses at every level, so the whole identity stream is
    // the exact reverse of the ascending one — not just the outermost component. The
    // reader and the writer share one committed catalog so their member catalog ids
    // address the same store cells.
    let program = checked_program(&format!(
        "{ENROLLMENT_PRIMARY}\npub fn revStatuses()\n    for id, enrollment in reversed(^enrollments)\n        print(enrollment.status ?? \"\")\n"
    ));
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
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
    };
    enroll("s1", "c1", "active");
    enroll("s1", "c2", "dropped");
    enroll("s2", "c1", "active");

    // Ascending identity order is (s1,c1),(s1,c2),(s2,c1); reverse is the mirror.
    let outcome = run_entry(&store, checked_entry!(&program, "test::revStatuses")).expect("run");
    assert_eq!(outcome.output, "active\ndropped\nactive\n");
}

/// Iterating a sequence/keyed child layer yields positions. Two-name loops pair
/// each position with its value.
fn book_tags() -> String {
    format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()
    ^books(1).title = \"Mort\"
    const a: int = append(^books(1).tags, \"fiction\")
    const b: int = append(^books(1).tags, \"funny\")

pub fn positions()
    for pos in ^books(1).tags
        print($\"{{pos}}\")

pub fn tagValues()
    for pos, tag in ^books(1).tags
        print(tag)

pub fn tagEntries()
    for pos, tag in ^books(1).tags
        print($\"{{pos}}={{tag}}\")

pub fn tagValuesDescending()
    for pos, tag in reversed(^books(1).tags)
        print(tag)

pub fn positionsDescending()
    for pos in reversed(keys(^books(1).tags))
        print($\"{{pos}}\")

pub fn keysOf()
    for pos in keys(^books(1).tags)
        print($\"{{pos}}\")

pub fn positionsBetween(lo: int, hi: int)
    for pos in ^books(1).tags(lo..hi)
        print($\"{{pos}}\")

pub fn positionsBetweenKeys(lo: int, hi: int)
    for pos in keys(^books(1).tags(lo..hi))
        print($\"{{pos}}\")

pub fn entriesBetween(lo: int, hi: int)
    for pos, tag in entries(^books(1).tags(lo..hi))
        print($\"{{pos}}={{tag}}\")

pub fn positionsBetweenDescending(lo: int, hi: int)
    for pos in reversed(^books(1).tags(lo..hi))
        print($\"{{pos}}\")
"
    )
}

#[test]
fn iterates_a_sequence_child_layer() {
    let program = checked_program(&book_tags());
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // Direct layer iteration yields 1-based positions in key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::positions")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");

    // `keys(^books(1).tags)` yields the same positions.
    let outcome = run_entry(&store, checked_entry!(&program, "test::keysOf")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::positionsBetween",
            Value::Int(1),
            Value::Int(2)
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "1\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::positionsBetweenKeys",
            Value::Int(1),
            Value::Int(2)
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "1\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::entriesBetween",
            Value::Int(1),
            Value::Int(3)
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "1=fiction\n2=funny\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::positionsBetweenDescending",
            Value::Int(1),
            Value::Int(3)
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "2\n1\n");
}

#[test]
fn sequence_child_layer_two_name_loop_yields_element_values() {
    let program = checked_program(&book_tags());
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::tagValues")).expect("run");
    assert_eq!(outcome.output, "fiction\nfunny\n");
}

#[test]
fn two_name_sequence_child_layer_loop_yields_key_and_value() {
    let program = checked_program(&book_tags());
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::tagEntries")).expect("run");
    assert_eq!(outcome.output, "1=fiction\n2=funny\n");
}

#[test]
fn reversed_sequence_child_layer_loop_yields_values_descending() {
    let program = checked_program(&book_tags());
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::tagValuesDescending"),
    )
    .expect("run");
    assert_eq!(outcome.output, "funny\nfiction\n");
}

#[test]
fn reversed_sequence_child_layer_keys_loop_yields_positions_descending() {
    let program = checked_program(&book_tags());
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::positionsDescending"),
    )
    .expect("run");
    assert_eq!(outcome.output, "2\n1\n");
}

/// A keyed (non-sequence) child tree iterates keys; two-name loops pair keys
/// with values. Seeded through the store directly to keep the focus on order.
const PLAYER_SCORES: &str = "\
resource Game
    scores(playerId: string): int
store ^games(id: int): Game

pub fn players()
    for p in keys(^games(1).scores)
        print(p)

pub fn scores()
    for player, score in ^games(1).scores
        print($\"{score}\")
";

#[test]
fn iterates_a_keyed_child_tree() {
    let program = checked_program(PLAYER_SCORES);
    let store = TreeStore::memory();
    let score = |player: &str, n: i64| {
        write_data_value(
            &program,
            &store,
            "games",
            &[SavedKey::Int(1)],
            &keyed_data_path(
                &program,
                "games",
                &[("scores", vec![SavedKey::Str(player.into())])],
                &[],
            ),
            SavedValue::Int(n),
        );
    };
    score("bob", 7);
    score("alice", 10);

    // Keys iterate in sorted key order (alice before bob).
    let outcome = run_entry(&store, checked_entry!(&program, "test::players")).expect("run");
    assert_eq!(outcome.output, "alice\nbob\n");

    // Two-name child-layer iteration yields values in key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::scores")).expect("run");
    assert_eq!(outcome.output, "10\n7\n");
}

/// A keyed group layer iterates keys, with two-name loops preserving the group
/// address alongside the entry value.
const BOOK_VERSION_LOOPS: &str = "\
resource Book
    required title: string

    versions(v: int)
        required title: string
store ^books(id: int): Book

pub fn seed()
    ^books(1).title = \"Mort\"
    ^books(1).versions(2).title = \"second\"
    ^books(1).versions(1).title = \"first\"

pub fn versionTitles()
    for v, version in ^books(1).versions
        print(version.title)

pub fn versionEntries()
    for v, version in ^books(1).versions
        print($\"{v}: {version.title}\")
";

#[test]
fn keyed_group_layer_loop_yields_materialized_entries() {
    let program = checked_program(BOOK_VERSION_LOOPS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::versionTitles")).expect("run");
    assert_eq!(outcome.output, "first\nsecond\n");
}

#[test]
fn two_name_keyed_group_layer_loop_yields_key_and_entry() {
    let program = checked_program(BOOK_VERSION_LOOPS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::versionEntries")).expect("run");
    assert_eq!(outcome.output, "1: first\n2: second\n");
}

/// A composite-key leaf layer (`cells(row, col): T`) is a chain of single-key
/// sub-layers. The outer loop binds the outer key, and `cells(outer)` descends
/// to the inner sub-layer of `col -> value`.
const GRID_CELLS: &str = "\
resource Grid
    cells(row: int, col: int): string
store ^grids(id: int): Grid

pub fn seed()
    ^grids(1).cells(1, 1) = \"a\"
    ^grids(1).cells(1, 2) = \"b\"
    ^grids(1).cells(2, 1) = \"c\"

pub fn outerRows()
    for row in ^grids(1).cells
        print($\"{row}\")

pub fn innerCols(row: int)
    for col in ^grids(1).cells(row)
        print($\"{col}\")

pub fn innerEntries(row: int)
    for col, v in ^grids(1).cells(row)
        print($\"{col}={v}\")

pub fn descendValue(row: int, col: int)
    print(^grids(1).cells(row, col) ?? \"absent\")

pub fn innerCount(row: int)
    print($\"{count(^grids(1).cells(row))}\")

pub fn innerColsReversed(row: int)
    for col in reversed(^grids(1).cells(row))
        print($\"{col}\")
";

#[test]
fn outer_loop_over_a_composite_leaf_layer_binds_the_outer_key() {
    let program = checked_program(GRID_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // The outer key set is the distinct row keys in key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::outerRows")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

#[test]
fn partial_key_descent_iterates_the_inner_sub_layer() {
    let program = checked_program(GRID_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // `cells(1)` descends to the inner `col` sub-layer with two entries.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::innerCols", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.output, "1\n2\n");

    // `cells(2)` has a single inner column.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::innerCols", Value::Int(2)),
    )
    .expect("run");
    assert_eq!(outcome.output, "1\n");
}

#[test]
fn partial_key_descent_two_name_loop_yields_inner_key_and_value() {
    let program = checked_program(GRID_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::innerEntries", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.output, "1=a\n2=b\n");
}

#[test]
fn full_key_descent_reads_the_leaf_value() {
    let program = checked_program(GRID_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::descendValue", Value::Int(1), Value::Int(2)),
    )
    .expect("run");
    assert_eq!(outcome.output, "b\n");
}

#[test]
fn count_over_a_partial_key_descent_counts_inner_entries() {
    let program = checked_program(GRID_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::innerCount", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.output, "2\n");
}

#[test]
fn reversed_partial_key_descent_walks_inner_keys_descending() {
    let program = checked_program(GRID_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::innerColsReversed", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.output, "2\n1\n");
}

#[test]
fn partial_key_descent_over_an_empty_inner_layer_resolves_cleanly() {
    // An outer key with no inner entries iterates zero times and counts zero —
    // never a `run.absent_element` fault from a check-clean program.
    let program = checked_program(&format!(
        "{GRID_CELLS}\npub fn emptyInner()\n    for col in ^grids(1).cells(99)\n        print($\"{{col}}\")\n    print($\"{{count(^grids(1).cells(99))}}\")\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::emptyInner")).expect("run");
    assert_eq!(outcome.output, "0\n");
}

/// A three-key cube layer descends one column at a time: `cells(x)` and
/// `cells(x, y)` each peel a column until the final `z -> string` sub-layer, whose
/// keys, values, and entries iterate as a leaf collection.
const CUBE_CELLS: &str = "\
resource Cube
    cells(x: int, y: int, z: int): string
store ^cubes(id: int): Cube

pub fn seed()
    ^cubes(1).cells(1, 2, 3) = \"a\"
    ^cubes(1).cells(1, 2, 4) = \"b\"

pub fn descendToLeaf()
    for x in ^cubes(1).cells
        for y in ^cubes(1).cells(x)
            for z, v in ^cubes(1).cells(x, y)
                print($\"{x},{y},{z}={v}\")

pub fn leafValues(x: int, y: int)
    for v in values(^cubes(1).cells(x, y))
        print($\"{v}\")
";

#[test]
fn three_key_descent_chain_iterates_the_leaf_without_faulting() {
    // The full descent the checker permits runs end to end: peeling each column in
    // turn reaches the leaf sub-layer, whose two-name and value loops yield the
    // stored values — never a `run.absent_element` or `run.unsupported` fault.
    let program = checked_program(CUBE_CELLS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::descendToLeaf")).expect("run");
    assert_eq!(outcome.output, "1,2,3=a\n1,2,4=b\n");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::leafValues", Value::Int(1), Value::Int(2)),
    )
    .expect("run");
    assert_eq!(outcome.output, "a\nb\n");
}

/// A keyless singleton store whose record carries a keyed leaf layer. The layer
/// streams its own key column independent of the root's empty identity.
const SETTINGS_COUNTS: &str = "\
resource Settings
    counts(name: string): int
store ^settings: Settings

pub fn seed()
    ^settings.counts(\"a\") = 1
    ^settings.counts(\"b\") = 2

pub fn names()
    for name in ^settings.counts
        print($\"{name}\")

pub fn pairs()
    for name, c in ^settings.counts
        print($\"{name}={c}\")
";

#[test]
fn a_layer_on_a_singleton_root_iterates_its_key_column() {
    // The layer on a keyless singleton is iterable just as one on a keyed root is:
    // the single-name loop binds the layer key, the two-name loop binds key and
    // value — neither is mistaken for a single non-iterable value.
    let program = checked_program(SETTINGS_COUNTS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::names")).expect("run");
    assert_eq!(outcome.output, "a\nb\n");

    let outcome = run_entry(&store, checked_entry!(&program, "test::pairs")).expect("run");
    assert_eq!(outcome.output, "a=1\nb=2\n");
}
