//! Ordered saved navigation: reversed iteration and stored next / prev neighbor
//! seeks over record identities, child-layer keys, and index branches.

use crate::support;
use support::*;

use marrow_run::{RUN_UNSUPPORTED, Value};
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

pub fn keysReversedValue()
    const r = reversed(keys(^books))
    for id in r
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

    // Materializing the same durable reversed key collection as a value is rejected.
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::keysReversedValue")),
        RUN_UNSUPPORTED,
    );

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
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\n\
         fn maybeNext(): maybe Id(^books)\n\
         \x20   return next(^books)\n\n\
         fn maybePrev(): maybe Id(^books)\n\
         \x20   return prev(^books)\n\n\
         pub fn nextFallback(): bool\n\
         \x20   return exists(^books(maybeNext() ?? Id(^books, 1)))\n\n\
         pub fn prevFallback(): bool\n\
         \x20   return exists(^books(maybePrev() ?? Id(^books, 1)))\n",
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
