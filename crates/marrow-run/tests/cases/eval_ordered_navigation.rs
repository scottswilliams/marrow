//! Ordered saved navigation: reversed iteration and stored next / prev neighbor
//! seeks over record identities, child-layer keys, and index branches.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::tree::TreeStore;

/// A primary keyed root with a keyed child layer, used to exercise reverse
/// iteration and stored-neighbor seeks over both record identities and layer keys.
/// The resource/store header lives in the shared fixture; only the navigation
/// functions are spelled here.
fn nav_books() -> String {
    format!(
        "{BOOK_TAGS_SCHEMA}pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn delId(id: int)
    delete ^books(id)

pub fn tag(id: int, t: string)
    const p: int = append(^books(id).tags, t)

pub fn idsDescending()
    for id in reversed(keys(^books))
        print($\"{{id}}\")

pub fn titlesDescending()
    for id, book in reversed(^books)
        print(book.title)

pub fn nextOfKey(id: int, fallback: string): string
    const wanted: string = ^books(id).title ?? \"\"
    for current in keys(^books)
        if (^books(current).title ?? \"\") == wanted
            const neighbor: Id(^books) = next(^books(current)) ?? current
            if neighbor == current
                return fallback
            return ^books(neighbor).title ?? fallback
    return fallback

pub fn prevOfKey(id: int, fallback: string): string
    const wanted: string = ^books(id).title ?? \"\"
    for current in keys(^books)
        if (^books(current).title ?? \"\") == wanted
            const neighbor: Id(^books) = prev(^books(current)) ?? current
            if neighbor == current
                return fallback
            return ^books(neighbor).title ?? fallback
    return fallback

pub fn firstIdKey(fallback: string): string
    for current in keys(^books)
        const first: Id(^books) = next(^books) ?? current
        return ^books(first).title ?? fallback
    return fallback

pub fn lastIdKey(fallback: string): string
    for current in keys(^books)
        const last: Id(^books) = prev(^books) ?? current
        return ^books(last).title ?? fallback
    return fallback

pub fn breakAfterFirst(): int
    var seen = 0
    for id in reversed(^books)
        seen = seen + 1
        break
    return seen
"
    )
}

#[test]
fn reversed_layer_iterates_descending_and_skips_a_hole() {
    let program = checked_program(&nav_books());
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
    add(1, "Mort");
    add(2, "Sourcery");
    add(3, "Reaper");

    // `reversed(keys(^books))` yields ids in descending key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::idsDescending")).expect("run");
    assert_eq!(outcome.output, "3\n2\n1\n");

    // Bare reversed root iteration yields records in descending key order.
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::titlesDescending")).expect("run");
    assert_eq!(outcome.output, "Reaper\nSourcery\nMort\n");

    // Deleting the middle record leaves a hole; reverse iteration skips it,
    // visiting only stored entries.
    run_entry(
        &store,
        checked_entry!(&program, "test::delId", Value::Int(2)),
    )
    .expect("del");
    let outcome = run_entry(&store, checked_entry!(&program, "test::idsDescending")).expect("run");
    assert_eq!(outcome.output, "3\n1\n");
}

#[test]
fn reversed_keys_as_a_value_is_rejected_at_check() {
    // Binding `reversed(keys(^books))` to a local materializes a saved collection — an
    // in-place reversed key stream the runtime refuses to materialize. It is a check
    // error, not a runtime fault.
    checker_rejects(
        &format!(
            "{BOOK_TAGS_SCHEMA}pub fn keysReversedValue()\n    const r = reversed(keys(^books))\n    for id in r\n        print($\"{{id}}\")\n"
        ),
        "check.collection_unsupported",
    );
}

#[test]
fn next_and_prev_skip_a_deleted_hole() {
    let program = checked_program(&nav_books());
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);
    add(5);
    // Delete the middle stored key, leaving a gap between 1 and 5.
    run_entry(
        &store,
        checked_entry!(&program, "test::delId", Value::Int(2)),
    )
    .expect("del");

    // `next(^books(1))` is the nearest *stored* successor, skipping the gap at 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::nextOfKey",
                Value::Int(1),
                Value::Str("missing".into())
            )
        )
        .expect("next")
        .value,
        Some(Value::Str("5".into()))
    );
    // `prev(^books(5))` mirrors it: the nearest stored predecessor is 1.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::prevOfKey",
                Value::Int(5),
                Value::Str("missing".into())
            )
        )
        .expect("prev")
        .value,
        Some(Value::Str("1".into()))
    );
}

#[test]
fn next_of_bare_layer_is_first_and_prev_is_last() {
    let program = checked_program(&nav_books());
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(4);
    add(2);
    add(9);

    // `next(^books)` (a bare layer) is the first stored entry; `prev(^books)` the
    // last — in key order, regardless of insertion order.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::firstIdKey", Value::Str("missing".into()))
        )
        .expect("first")
        .value,
        Some(Value::Str("2".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::lastIdKey", Value::Str("missing".into()))
        )
        .expect("last")
        .value,
        Some(Value::Str("9".into()))
    );
}

#[test]
fn maybe_return_propagates_empty_neighbor_absence() {
    // A `maybe`-returning function whose neighbor seek is empty propagates absence
    // through a `??` fallback. The fallback identity is bound to a name before it
    // keys a saved read, because a user-function call may not ride into a guard
    // key — pure or not, its body is opaque before per-function effect closures
    // exist, so the result is bound first and the guard keyed off the bound name.
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\n\
         fn maybeNext(): Id(^books)?\n\
         \x20   return next(^books)\n\n\
         fn maybePrev(): Id(^books)?\n\
         \x20   return prev(^books)\n\n\
         pub fn nextFallback(): bool\n\
         \x20   const id: Id(^books) = maybeNext() ?? Id(^books, 1)\n\
         \x20   return exists(^books(id))\n\n\
         pub fn prevFallback(): bool\n\
         \x20   const id: Id(^books) = maybePrev() ?? Id(^books, 1)\n\
         \x20   return exists(^books(id))\n",
    );
    let store = TreeStore::memory();

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::nextFallback"))
            .expect("next fallback")
            .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::prevFallback"))
            .expect("prev fallback")
            .value,
        Some(Value::Bool(false))
    );
}

#[test]
fn prev_of_first_is_absent_and_composes_with_coalesce() {
    let program = checked_program(&nav_books());
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::prevOfKey",
                Value::Int(1),
                Value::Str("-1".into())
            )
        )
        .expect("prev default")
        .value,
        Some(Value::Str("-1".into()))
    );

    // `next` of the last stored key is likewise absent, and `?? -1` recovers it —
    // the edge fault composes with `??`.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::nextOfKey",
                Value::Int(2),
                Value::Str("-1".into())
            )
        )
        .expect("coalesce")
        .value,
        Some(Value::Str("-1".into()))
    );
}

#[test]
fn next_neighbor_identity_reads_a_field() {
    let program = checked_program(&nav_books());
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

    // `^books(next(^books(1))).title` reads the neighbor record's field through its
    // returned identity.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::nextOfKey",
                Value::Int(1),
                Value::Str("missing".into())
            )
        )
        .expect("nextTitle")
        .value,
        Some(Value::Str("Sourcery".into()))
    );
}

#[test]
fn reversed_iteration_supports_early_break() {
    let program = checked_program(&nav_books());
    let store = TreeStore::memory();
    for id in 1..=3 {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str("t".into())
            ),
        )
        .expect("add");
    }
    // A `break` on the first reversed element stops the loop after one iteration.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::breakAfterFirst"))
            .expect("break")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn neighbors_on_a_keyed_child_layer_position() {
    // `next(^books(1).tags(1))` seeks among the layer's integer positions.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    const x: int = append(^books(1).tags, \"p\")\n    const y: int = append(^books(1).tags, \"q\")\n    const z: int = append(^books(1).tags, \"r\")\n\npub fn nextPos(p: int): int\n    return next(^books(1).tags(p)) ?? 0\n\npub fn prevPos(p: int): int\n    return prev(^books(1).tags(p)) ?? 0\n\npub fn firstPos(): int\n    return next(^books(1).tags) ?? 0\n\npub fn lastPos(): int\n    return prev(^books(1).tags) ?? 0\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // Positions are 1, 2, 3; the successor of 1 is 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextPos", Value::Int(1))
        )
        .expect("nextPos")
        .value,
        Some(Value::Int(2))
    );
    // The predecessor of 3 is 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::prevPos", Value::Int(3))
        )
        .expect("prevPos")
        .value,
        Some(Value::Int(2))
    );
    // `next(^books(1).tags)` and `prev(^books(1).tags)` seek from bare layer edges.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstPos"))
            .expect("firstPos")
            .value,
        Some(Value::Int(1))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::lastPos"))
            .expect("lastPos")
            .value,
        Some(Value::Int(3))
    );
}

#[test]
fn if_const_binds_a_keyed_layer_neighbor_as_a_usable_key() {
    // Over a keyed child layer the neighbor types to the layer's key, so an
    // `if const` binding is directly usable as that key — addressing the sibling
    // entry's value without a `??` default to recover the type.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    const x: int = append(^books(1).tags, \"p\")\n    const y: int = append(^books(1).tags, \"q\")\n\npub fn neighborTag(p: int): string\n    if const n = next(^books(1).tags(p))\n        return ^books(1).tags(n) ?? \"absent\"\n    return \"edge\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // The successor of position 1 is position 2, whose value is "q".
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::neighborTag", Value::Int(1))
        )
        .expect("neighborTag")
        .value,
        Some(Value::Str("q".to_string()))
    );
    // Position 2 is the last, so its neighbor is absent and the `if const` is skipped.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::neighborTag", Value::Int(2))
        )
        .expect("neighborTag")
        .value,
        Some(Value::Str("edge".to_string()))
    );
}

/// A composite keyed layer is a chain of single-key sub-layers. A partial prefix
/// names an inner sub-layer, so its edge neighbor is the first/last entry of that
/// inner column under the prefix — the same descent `count` and iteration take. A
/// fully-keyed leaf is a position within the final column, so its neighbor is the
/// stored sibling in that column under the same outer prefix.
const GRID_NEIGHBORS: &str = "\
resource Grid
    cells(row: int, col: int): string
store ^grids(id: int): Grid

pub fn seed()
    ^grids(1).cells(0, 2) = \"a\"
    ^grids(1).cells(0, 7) = \"b\"
    ^grids(1).cells(5, 1) = \"c\"

pub fn firstInner(row: int): int
    return next(^grids(1).cells(row)) ?? -1

pub fn lastInner(row: int): int
    return prev(^grids(1).cells(row)) ?? -1

pub fn nextLeaf(row: int, col: int): int
    return next(^grids(1).cells(row, col)) ?? -1

pub fn prevLeaf(row: int, col: int): int
    return prev(^grids(1).cells(row, col)) ?? -1
";

#[test]
fn neighbor_of_a_partial_composite_prefix_seeks_the_inner_column_edge() {
    // `cells(0)` descends to the inner `col` sub-layer under row 0, whose stored
    // columns are 2 and 7. `next` is the first (2), `prev` the last (7) — the same
    // entries the descending loop and `count` see, never the outer `row` column.
    let program = checked_program(GRID_NEIGHBORS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::firstInner", Value::Int(0))
        )
        .expect("firstInner")
        .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::lastInner", Value::Int(0))
        )
        .expect("lastInner")
        .value,
        Some(Value::Int(7))
    );
}

#[test]
fn neighbor_of_a_fully_keyed_composite_leaf_seeks_the_final_column() {
    // `cells(0, 2)` is a position in the inner `col` column under row 0; its
    // neighbor is the stored sibling in that column. The successor of col 2 is 7,
    // the predecessor of col 7 is 2. Stepping off an edge is a catchable absent.
    let program = checked_program(GRID_NEIGHBORS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextLeaf", Value::Int(0), Value::Int(2))
        )
        .expect("nextLeaf")
        .value,
        Some(Value::Int(7))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::prevLeaf", Value::Int(0), Value::Int(7))
        )
        .expect("prevLeaf")
        .value,
        Some(Value::Int(2))
    );
    // The successor of the last column under row 0 steps off the edge: a catchable
    // absent that `?? -1` recovers, never an uncatchable `run.unsupported`.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextLeaf", Value::Int(0), Value::Int(7))
        )
        .expect("nextLeaf edge")
        .value,
        Some(Value::Int(-1))
    );
}

/// A neighbor fixture spanning every key arity over the same composite layer: a
/// fully-keyed leaf, a partial outer prefix, the bare layer with no column filled,
/// and an absent key. Each probe coalesces an edge/absent result to a sentinel, so a
/// regression that faulted `run.unsupported` or surfaced an uncatchable
/// `run.absent_element` would fail the run rather than return the sentinel cleanly.
const GRID_NEIGHBOR_ARITIES: &str = "\
resource Grid
    cells(row: int, col: int): string
store ^grids(id: int): Grid

pub fn seed()
    ^grids(1).cells(0, 2) = \"a\"
    ^grids(1).cells(0, 7) = \"b\"
    ^grids(1).cells(5, 1) = \"c\"

pub fn nextLeaf(row: int, col: int): int
    return next(^grids(1).cells(row, col)) ?? -1

pub fn prevLeaf(row: int, col: int): int
    return prev(^grids(1).cells(row, col)) ?? -1

pub fn nextPrefix(row: int): int
    return next(^grids(1).cells(row)) ?? -1

pub fn prevPrefix(row: int): int
    return prev(^grids(1).cells(row)) ?? -1

pub fn nextBare(): int
    return next(^grids(1).cells) ?? -1

pub fn prevBare(): int
    return prev(^grids(1).cells) ?? -1
";

#[test]
fn neighbor_over_every_composite_key_arity_returns_cleanly() {
    // The runtime already navigates a composite layer one column at a time. This pins
    // the observed neighbor value at each key arity — the full leaf seeks a sibling in
    // the final column, the partial outer prefix and the bare layer seek the edge of
    // the first unfilled column, and an absent key falls off cleanly — and proves none
    // of them faults `run.unsupported` or escapes an uncatchable `run.absent_element`.
    let program = checked_program(GRID_NEIGHBOR_ARITIES);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let probe = |func: &str, args: Vec<Value>| {
        run_entry(
            &store,
            checked_entry_call(&program, &format!("test::{func}"), args),
        )
        .unwrap_or_else(|error| panic!("{func} faulted: {error:?}"))
        .value
    };

    // A fully-keyed leaf seeks the stored sibling in the inner `col` column under the
    // same `row`: after col 2 is col 7, before col 7 is col 2.
    assert_eq!(
        probe("nextLeaf", vec![Value::Int(0), Value::Int(2)]),
        Some(Value::Int(7))
    );
    assert_eq!(
        probe("prevLeaf", vec![Value::Int(0), Value::Int(7)]),
        Some(Value::Int(2))
    );

    // A partial outer prefix descends to the inner `col` column under `row` 0, whose
    // stored values are 2 and 7: `next` is the first (2), `prev` the last (7).
    assert_eq!(
        probe("nextPrefix", vec![Value::Int(0)]),
        Some(Value::Int(2))
    );
    assert_eq!(
        probe("prevPrefix", vec![Value::Int(0)]),
        Some(Value::Int(7))
    );

    // The bare layer fills no column, so the seek navigates the outer `row` column,
    // whose stored values are 0 and 5: `next` is the first (0), `prev` the last (5).
    assert_eq!(probe("nextBare", vec![]), Some(Value::Int(0)));
    assert_eq!(probe("prevBare", vec![]), Some(Value::Int(5)));

    // An absent outer key (`row` 99 has no stored inner column) and an absent inner
    // key (`col` 99 under `row` 0) both step off the edge: a catchable absent the `??`
    // recovers to the sentinel, never an uncatchable fault.
    assert_eq!(
        probe("nextPrefix", vec![Value::Int(99)]),
        Some(Value::Int(-1))
    );
    assert_eq!(
        probe("nextLeaf", vec![Value::Int(0), Value::Int(99)]),
        Some(Value::Int(-1))
    );
}

/// A composite-leaf grid for proving `delete` is surgical: deleting a fully-keyed
/// leaf removes exactly that entry, never the inner sub-tree under a partial prefix.
/// A partial-key `delete` is rejected at check (see the checker tests), so the only
/// delete that reaches the runtime is the fully-keyed one this fixture exercises.
const GRID_DELETE: &str = "\
resource Grid
    cells(row: int, col: int): string
store ^grids(id: int): Grid

pub fn seed()
    ^grids(1).cells(1, 2) = \"a\"
    ^grids(1).cells(1, 3) = \"b\"
    ^grids(1).cells(2, 2) = \"c\"

pub fn delLeaf(row: int, col: int)
    delete ^grids(1).cells(row, col)

pub fn leaf(row: int, col: int): string
    return ^grids(1).cells(row, col) ?? \"gone\"
";

#[test]
fn deleting_a_fully_keyed_composite_leaf_removes_only_that_entry() {
    // `delete ^grids(1).cells(1, 2)` drops exactly the (1,2) leaf. Its sibling in the
    // same inner column (1,3) and the entry under a different row (2,2) both survive,
    // so the fully-keyed delete is a single-entry delete, never a prefix cascade.
    let program = checked_program(GRID_DELETE);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    run_entry(
        &store,
        checked_entry!(&program, "test::delLeaf", Value::Int(1), Value::Int(2)),
    )
    .expect("delete the fully-keyed leaf");

    let leaf = |row: i64, col: i64| {
        run_entry(
            &store,
            checked_entry!(&program, "test::leaf", Value::Int(row), Value::Int(col)),
        )
        .expect("read leaf")
        .value
    };
    assert_eq!(leaf(1, 2), Some(Value::Str("gone".into())));
    assert_eq!(leaf(1, 3), Some(Value::Str("b".into())));
    assert_eq!(leaf(2, 2), Some(Value::Str("c".into())));
}

/// A three-key cube layer peels one column per supplied key, so a partial prefix of
/// any length descends to the first unfilled column. The edge neighbor of that
/// prefix is the first/last entry of that column under the pinned prefix.
const CUBE_NEIGHBORS: &str = "\
resource Cube
    cells(x: int, y: int, z: int): string
store ^cubes(id: int): Cube

pub fn seed()
    ^cubes(1).cells(0, 0, 3) = \"p\"
    ^cubes(1).cells(0, 5, 0) = \"q\"
    ^cubes(1).cells(4, 0, 0) = \"r\"

pub fn firstAfterX(x: int): int
    return next(^cubes(1).cells(x)) ?? -1

pub fn firstAfterXY(x: int, y: int): int
    return next(^cubes(1).cells(x, y)) ?? -1
";

#[test]
fn neighbor_of_a_multi_column_partial_prefix_descends_to_the_first_unfilled_column() {
    // `cells(0)` leaves two columns (`y`, `z`); its edge neighbor is the first `y`
    // under x 0 — among y values 0 and 5, the first is 0, not the next `x`. A
    // two-column prefix `cells(0, 0)` descends to the `z` column, first value 3.
    let program = checked_program(CUBE_NEIGHBORS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::firstAfterX", Value::Int(0))
        )
        .expect("firstAfterX")
        .value,
        Some(Value::Int(0))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::firstAfterXY", Value::Int(0), Value::Int(0))
        )
        .expect("firstAfterXY")
        .value,
        Some(Value::Int(3))
    );
}

#[test]
fn reversed_over_an_in_memory_sequence_reverses_directly() {
    // `reversed(values(std::text::split(...)))` reverses the in-memory element
    // values — no store involved — so a `for` over it yields them back-to-front.
    let program = checked_program(
        "pub fn rev()\n    for word in reversed(values(std::text::split(\"a,b,c\", \",\")))\n        print(word)\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "test::rev")).expect("run");
    assert_eq!(outcome.output, "c\nb\na\n");
}

#[test]
fn reversed_respects_the_traversed_layer_write_guard() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn clear()\n    for id in reversed(keys(^books))\n        delete ^books(id)\n"
        ),
        "check.loop_mutates_traversed_layer",
    );
}

/// A non-unique index branch, iterated forward and reversed: the entries enumerate
/// in identity-key order, and `reversed(...)` walks the same branch backward. The
/// resource/store/index header lives in the shared fixture.
fn book_shelf_nav() -> String {
    format!(
        "{BOOK_SHELF_INDEX_SCHEMA}pub fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

pub fn onShelfReversed(shelf: string)
    for id in reversed(^books.byShelf(shelf))
        print($\"{{id}}\")
"
    )
}

#[test]
fn reversed_over_an_index_branch_descends() {
    // `reversed(^books.byShelf(\"x\"))` walks a declared index branch backward,
    // yielding the matching identities in descending id order.
    let program = checked_program(&book_shelf_nav());
    let store = TreeStore::memory();
    let add = |id: i64, s: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str("t".into()),
                Value::Str(s.into())
            ),
        )
        .expect("add");
    };
    add(1, "x");
    add(2, "x");
    add(3, "y");
    add(4, "x");

    // Only shelf-"x" ids (1, 2, 4) match, enumerated in descending key order.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::onShelfReversed", Value::Str("x".into())),
    )
    .expect("run");
    assert_eq!(outcome.output, "4\n2\n1\n");
}

/// `next`/`prev` over a keyed root that *also* declares an index. The index is
/// stored as a named child of the root, sorting after the record-key children, so
/// the edge seek must skip it: stepping off the last record raises the catchable
/// `run.absent_element`, never an uncatchable `run.unsupported`, and `prev(^books)`
/// returns the last *record*, not the index.
fn book_shelf_neighbor() -> String {
    format!(
        "{BOOK_SHELF_INDEX_SCHEMA}pub fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

pub fn nextOrDefaultKey(id: int, fallback: string): string
    const wanted: string = ^books(id).title ?? \"\"
    for current in keys(^books)
        if (^books(current).title ?? \"\") == wanted
            const neighbor: Id(^books) = next(^books(current)) ?? current
            if neighbor == current
                return fallback
            return ^books(neighbor).title ?? fallback
    return fallback

pub fn nextOrSelfKey(id: int): string
    return nextOrDefaultKey(id, ^books(id).title ?? \"\")

pub fn lastIdKey(fallback: string): string
    for current in keys(^books)
        const last: Id(^books) = prev(^books) ?? current
        return ^books(last).title ?? fallback
    return fallback
"
    )
}

#[test]
fn neighbor_at_an_indexed_root_edge_skips_the_index_on_both_backends() {
    // The declared `index byShelf` is a named child of `^books`, stored after the
    // record-key children. The edge seek must skip it: `next` past the last record
    // is a catchable absent-element (so `??` recovers), and `prev(^books)` lands on
    // the last record, not the index name. Both must hold in memory and redb.
    let program = checked_program(&book_shelf_neighbor());
    let dir = TempDir::new("marrow-run-test").expect("temp dir");
    let mem = TreeStore::memory();
    let redb = TreeStore::open(&dir.path().join("nav.redb")).expect("open redb");
    let stores: [&TreeStore; 2] = [&mem, &redb];

    for store in stores {
        let add = |id: i64, s: &str| {
            run_entry(
                store,
                checked_entry!(
                    &program,
                    "test::add",
                    Value::Int(id),
                    Value::Str(id.to_string()),
                    Value::Str(s.into())
                ),
            )
            .expect("add");
        };
        add(1, "x");
        add(2, "x");
        add(3, "y");
        add(4, "x");

        // `next(^books(4)) ?? -1`: 4 is the last record, so `next` steps off the
        // edge with a catchable absent-element that `?? -1` recovers — it must not
        // abort with `run.unsupported` by landing on the `byShelf` index name.
        assert_eq!(
            run_entry(
                store,
                checked_entry!(
                    &program,
                    "test::nextOrDefaultKey",
                    Value::Int(4),
                    Value::Str("-1".into())
                )
            )
            .expect("next ?? -1")
            .value,
            Some(Value::Str("-1".into()))
        );
        // The same edge with a different default proves `??` is reached, not bypassed.
        assert_eq!(
            run_entry(
                store,
                checked_entry!(&program, "test::nextOrSelfKey", Value::Int(4))
            )
            .expect("next ?? id")
            .value,
            Some(Value::Str("4".into()))
        );
        // `prev(^books)` is the last *record* (4), not the trailing index name.
        assert_eq!(
            run_entry(
                store,
                checked_entry!(&program, "test::lastIdKey", Value::Str("missing".into()))
            )
            .expect("prev")
            .value,
            Some(Value::Str("4".into()))
        );
    }
}

/// A primary keyed root with `next`/`prev` neighbor reads resolved by every
/// maybe-present guard: `??`, `if const`, and `exists`. A neighbor result is
/// maybe-present and resolves at the read site like any maybe-present value, so
/// each guard form must accept it.
fn neighbor_guards() -> String {
    format!(
        "{BOOK_TAGS_SCHEMA}pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn nextWithCoalesce(id: int, fallback: int): string
    const nb: Id(^books) = next(^books(id)) ?? Id(^books, fallback)
    return ^books(nb).title ?? \"missing\"

pub fn nextWithIfConst(id: int): string
    if const nb = next(^books(id))
        return ^books(nb).title ?? \"missing\"
    return \"none\"

pub fn nextExists(id: int): bool
    return exists(next(^books(id)))

pub fn prevExists(id: int): bool
    return exists(prev(^books(id)))
"
    )
}

#[test]
fn next_neighbor_result_resolves_under_if_const() {
    let program = checked_program(&neighbor_guards());
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);

    // `if const nb = next(^books(1))` binds the present neighbor (id 2, title "2").
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextWithIfConst", Value::Int(1))
        )
        .expect("if const present")
        .value,
        Some(Value::Str("2".into()))
    );
    // `next` off the last record is absent, so the `if const` else branch runs.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextWithIfConst", Value::Int(2))
        )
        .expect("if const absent")
        .value,
        Some(Value::Str("none".into()))
    );
}

#[test]
fn next_and_prev_neighbor_results_resolve_under_exists() {
    let program = checked_program(&neighbor_guards());
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);

    // `exists(next(^books(1)))` is true: a successor record exists.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextExists", Value::Int(1))
        )
        .expect("next exists")
        .value,
        Some(Value::Bool(true))
    );
    // `exists(next(^books(2)))` is false: 2 is the last record.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextExists", Value::Int(2))
        )
        .expect("next missing")
        .value,
        Some(Value::Bool(false))
    );
    // `exists(prev(^books(1)))` is false: 1 is the first record.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::prevExists", Value::Int(1))
        )
        .expect("prev missing")
        .value,
        Some(Value::Bool(false))
    );
}

#[test]
fn next_neighbor_result_still_resolves_under_coalesce() {
    let program = checked_program(&neighbor_guards());
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);

    // `next(^books(2)) ?? Id(^books, 1)` recovers the edge with the fallback id,
    // whose title is "1".
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::nextWithCoalesce",
                Value::Int(2),
                Value::Int(1)
            )
        )
        .expect("coalesce")
        .value,
        Some(Value::Str("1".into()))
    );
}

#[test]
fn an_effectful_neighbor_base_stays_rejected_under_a_guard() {
    // Widening the guards to accept a `next`/`prev` result must not admit an
    // effectful expression. `next` of a function-call base (which could write,
    // open a transaction, call a host capability, or throw) is not a saved place,
    // so it stays rejected under `if const` exactly as under `??`.
    checker_rejects(
        &format!(
            "{BOOK_TAGS_SCHEMA}fn allocate(): Id(^books)\n    ^books(99).title = \"x\"\n    return Id(^books, 99)\n\npub fn smuggle()\n    if const nb = next(allocate())\n        print(\"reached\")\n"
        ),
        "check.condition_type",
    );
}

#[test]
fn an_effectful_neighbor_base_stays_rejected_under_exists() {
    checker_rejects(
        &format!(
            "{BOOK_TAGS_SCHEMA}fn allocate(): Id(^books)\n    ^books(99).title = \"x\"\n    return Id(^books, 99)\n\npub fn smuggle(): bool\n    return exists(next(allocate()))\n"
        ),
        "check.call_argument",
    );
}
