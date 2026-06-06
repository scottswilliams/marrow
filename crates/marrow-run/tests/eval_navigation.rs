//! Saved-layer enumeration and ordered navigation: primary and composite roots,
//! sequence and keyed child layers, traversal-write guards, and reversed / next
//! / prev neighbor reads.

#[macro_use]
mod support;

use support::*;

use marrow_run::{RUN_TRAVERSAL, RUN_TYPE, RUN_UNSUPPORTED, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

// --- Unified saved-layer enumeration ---

const BOOK_PRIMARY: &str = "\
resource Book at ^books(id: int)
    required title: string

pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn titles()
    for id in ^books
        print(^books(id).title)

pub fn directIds()
    for id in ^books
        print($\"{id}\")

pub fn idsAndElementTitles()
    for id, book in ^books
        print($\"{id}: {book.title}\")

pub fn reversedElementTitles()
    for id, book in reversed(^books)
        print(book.title)

pub fn reversedIdsAsValue()
    const ids = reversed(^books)
    for id in ids
        print($\"{id}\")

pub fn ids()
    const all = keys(^books)
    for id in all
        print($\"{id}\")
";

#[test]
fn iterates_a_primary_keyed_root() {
    let program = checked_program(BOOK_PRIMARY);
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
    let program = checked_program(BOOK_PRIMARY);
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
    let program = checked_program(BOOK_PRIMARY);
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
    let program = checked_program(BOOK_PRIMARY);
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
    let program = checked_program(BOOK_PRIMARY);
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
    let program = checked_program(BOOK_PRIMARY);
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

#[test]
fn iterating_a_singleton_root_is_a_type_error() {
    // A keyless singleton has no identities to enumerate; iterating it is a
    // type error, not a silent empty loop.
    let program = checked_program(
        "resource Settings at ^settings\n    theme: string\n\npub fn each()\n    for s in ^settings\n        print(\"x\")\n",
    );
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::each")),
        RUN_TYPE,
    );
}

/// Iterating a composite primary root yields materialized records in identity order.
const ENROLLMENT_PRIMARY: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

pub fn enroll(s: string, c: string, st: string)
    ^enrollments(s, c).status = st

pub fn statuses()
    for id, enrollment in ^enrollments
        print(enrollment.status)
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

/// Iterating a sequence/keyed child layer yields positions. Two-name loops pair
/// each position with its value.
const BOOK_TAGS: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

pub fn seed()
    ^books(1).title = \"Mort\"
    const a: int = append(^books(1).tags, \"fiction\")
    const b: int = append(^books(1).tags, \"funny\")

pub fn positions()
    for pos in ^books(1).tags
        print($\"{pos}\")

pub fn tagValues()
    for pos, tag in ^books(1).tags
        print(tag)

pub fn tagEntries()
    for pos, tag in ^books(1).tags
        print($\"{pos}={tag}\")

pub fn tagValuesDescending()
    for pos, tag in reversed(^books(1).tags)
        print(tag)

pub fn positionsDescending()
    for pos in reversed(keys(^books(1).tags))
        print($\"{pos}\")

pub fn keysOf()
    for pos in keys(^books(1).tags)
        print($\"{pos}\")
";

#[test]
fn iterates_a_sequence_child_layer() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // Direct layer iteration yields 1-based positions in key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::positions")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");

    // `keys(^books(1).tags)` yields the same positions.
    let outcome = run_entry(&store, checked_entry!(&program, "test::keysOf")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

#[test]
fn sequence_child_layer_two_name_loop_yields_element_values() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::tagValues")).expect("run");
    assert_eq!(outcome.output, "fiction\nfunny\n");
}

#[test]
fn two_name_sequence_child_layer_loop_yields_key_and_value() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::tagEntries")).expect("run");
    assert_eq!(outcome.output, "1=fiction\n2=funny\n");
}

#[test]
fn reversed_sequence_child_layer_loop_yields_values_descending() {
    let program = checked_program(BOOK_TAGS);
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
    let program = checked_program(BOOK_TAGS);
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
resource Game at ^games(id: int)
    scores(playerId: string): int

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
resource Book at ^books(id: int)
    required title: string

    versions(v: int)
        required title: string

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

#[test]
fn deleting_a_record_while_traversing_the_root_is_a_traversal_fault() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn clear()\n    for id in keys(^books)\n        delete ^books(id)\n"
        ),
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn traversal_faults_are_not_catchable_errors() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn clear(): string\n    try\n        for id in keys(^books)\n            delete ^books(id)\n        return \"completed\"\n    catch error: Error\n        return error.code\n"
        ),
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    const p: int = append(^books(1).tags, \"x\")\n\npub fn grow()\n    for tag in ^books(1).tags\n        const p: int = append(^books(1).tags, \"y\")\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::grow"));
    assert_run_error(faulted, RUN_TRAVERSAL);
}

#[test]
fn helper_appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    append(^books(1).tags, \"x\")\n\npub fn grow()\n    append(^books(1).tags, \"y\")\n\npub fn walk()\n    for tag in ^books(1).tags\n        grow()\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::walk"));
    assert_run_error(faulted, RUN_TRAVERSAL);
}

#[test]
fn helper_deleting_from_the_root_being_traversed_is_a_traversal_fault() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn remove(id: int)\n    delete ^books(id)\n\npub fn walk()\n    for id in keys(^books)\n        remove(id)\n"
        ),
        "check.call_argument",
    );
}

#[test]
fn field_write_creating_a_record_in_the_traversed_root_is_a_traversal_fault() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn grow()\n    for id in ^books\n        ^books(99).title = \"new\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::grow"));
    assert_run_error(faulted, RUN_TRAVERSAL);
}

#[test]
fn collecting_saved_keys_as_a_local_snapshot_is_checker_rejected() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn clear()\n    const ids = keys(^books)\n    for id in ids\n        delete ^books(id)\n\npub fn remaining(): int\n    return count(^books)\n"
        ),
        "check.key_type",
    );
}

#[test]
fn mutating_a_different_record_layer_while_traversing_is_allowed() {
    // Traversing `^books(1).tags` and appending to `^books(2).tags` touches a
    // different record's layer, so it is not a traversal fault.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n    const p: int = append(^books(1).tags, \"x\")\n\npub fn copy()\n    for tag in ^books(1).tags\n        const p: int = append(^books(2).tags, \"y\")\n\npub fn tags2(): int\n    return count(^books(2).tags)\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::copy")).expect("copy");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tags2"))
            .expect("count")
            .value,
        Some(Value::Int(1))
    );
}

// --- Ordered navigation: reversed / next / prev ---

/// A primary keyed root with a keyed child layer, used to exercise reverse
/// iteration and stored-neighbor seeks over both record identities and layer keys.
const NAV_BOOKS: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags(pos: int): string

pub fn add(id: int, t: string)
    ^books(id).title = t

pub fn delId(id: int)
    delete ^books(id)

pub fn tag(id: int, t: string)
    const p: int = append(^books(id).tags, t)

pub fn idsDescending()
    for id in reversed(keys(^books))
        print($\"{id}\")

pub fn keysReversedValue()
    const r = reversed(keys(^books))
    for id in r
        print($\"{id}\")

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

pub fn nextOrDefaultKey(id: int, fallback: string): string
    return nextOfKey(id, fallback)

pub fn nextTitleKey(id: int): string
    return nextOfKey(id, \"\")

pub fn breakAfterFirst(): int
    var seen = 0
    for id in reversed(^books)
        seen = seen + 1
        break
    return seen
";

#[test]
fn reversed_layer_iterates_descending_and_skips_a_hole() {
    let program = checked_program(NAV_BOOKS);
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
    let program = checked_program(NAV_BOOKS);
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
    let program = checked_program(NAV_BOOKS);
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
fn prev_of_first_is_absent_and_composes_with_coalesce() {
    let program = checked_program(NAV_BOOKS);
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
                "test::nextOrDefaultKey",
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
    let program = checked_program(NAV_BOOKS);
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
            checked_entry!(&program, "test::nextTitleKey", Value::Int(1))
        )
        .expect("nextTitle")
        .value,
        Some(Value::Str("Sourcery".into()))
    );
}

#[test]
fn reversed_iteration_supports_early_break() {
    let program = checked_program(NAV_BOOKS);
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
fn next_on_a_keyed_child_layer_position() {
    // `next(^books(1).tags(1))` seeks among the layer's integer positions.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    const x: int = append(^books(1).tags, \"p\")\n    const y: int = append(^books(1).tags, \"q\")\n    const z: int = append(^books(1).tags, \"r\")\n\npub fn nextPos(p: int): int\n    return next(^books(1).tags(p)) ?? 0\n\npub fn firstPos(): int\n    return next(^books(1).tags) ?? 0\n"
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
    // `next(^books(1).tags)` (a bare layer) is the first stored position.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstPos"))
            .expect("firstPos")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn reversed_over_an_in_memory_sequence_reverses_directly() {
    // `reversed(std::text::split(...))` reverses the in-memory sequence — no store
    // involved — so a `for` over it yields the elements back-to-front.
    let program = checked_program(
        "pub fn rev()\n    for word in reversed(std::text::split(\"a,b,c\", \",\"))\n        print(word)\n",
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

#[test]
fn reversed_over_a_composite_root_is_a_true_reverse() {
    // A composite identity reverses at every level, so the whole identity stream is
    // the exact reverse of the ascending one — not just the outermost component. The
    // reader and the writer share one committed catalog so their member catalog ids
    // address the same store cells.
    let program = checked_program(&format!(
        "{ENROLLMENT_PRIMARY}\npub fn revStatuses()\n    for id, enrollment in reversed(^enrollments)\n        print(enrollment.status)\n"
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

/// A non-unique index branch, iterated forward and reversed: the entries enumerate
/// in identity-key order, and `reversed(...)` walks the same branch backward.
const BOOK_SHELF_NAV: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

pub fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

pub fn onShelfReversed(shelf: string)
    for id in reversed(^books.byShelf(shelf))
        print($\"{id}\")
";

#[test]
fn reversed_over_an_index_branch_descends() {
    // `reversed(^books.byShelf(\"x\"))` walks a declared index branch backward,
    // yielding the matching identities in descending id order.
    let program = checked_program(BOOK_SHELF_NAV);
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
const BOOK_SHELF_NEIGHBOR: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

pub fn add(id: int, t: string, s: string)
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
";

#[test]
fn neighbor_at_an_indexed_root_edge_skips_the_index_on_both_backends() {
    // The declared `index byShelf` is a named child of `^books`, stored after the
    // record-key children. The edge seek must skip it: `next` past the last record
    // is a catchable absent-element (so `??` recovers), and `prev(^books)` lands on
    // the last record, not the index name. Both must hold in memory and redb.
    let program = checked_program(BOOK_SHELF_NEIGHBOR);
    let dir = tempfile::tempdir().expect("temp dir");
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
