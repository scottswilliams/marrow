//! Index-key iteration: counting and printing in key order, bare first-level key
//! iteration, and the fault raised when an indexed field is mutated mid-traversal.

use crate::support;
use support::*;

use marrow_run::{RUN_TRAVERSAL, RUN_TYPE, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

/// A program that indexes books by shelf and traverses the index with `keys`.
/// The resource/store/index shape is owned by the shared `BOOK_SHELF_INDEX_SCHEMA`
/// fixture; only the traversal functions are declared here.
fn book_shelf() -> String {
    format!(
        "{BOOK_SHELF_INDEX_SCHEMA}pub fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

pub fn count_on(shelf: string): int
    var c = 0
    for id in ^books.byShelf(shelf)
        c = c + 1
    return c

pub fn count_via_bare_index(): int
    var c = 0
    for id in ^books.byShelf
        c = c + 1
    return c

pub fn reshelve_while_iterating()
    for id in ^books.byShelf(\"fiction\")
        ^books(id).shelf = \"history\"

pub fn reshelve_while_iterating_direct()
    for id in ^books.byShelf(\"fiction\")
        ^books(id).shelf = \"history\"

pub fn titles_on(shelf: string)
    for id, book in ^books.byShelf(shelf)
        print(book.title)
"
    )
}

const POST_DATES: &str = "\
resource Post
    required title: string
    published: int
store ^posts(id: int): Post

    index byDate(published, id)

pub fn add(id: int, title: string, published: int)
    ^posts(id).title = title
    ^posts(id).published = published

pub fn titlesBetween(start: int, end: int)
    for id in ^posts.byDate(start..end)
        print(^posts(id).title ?? \"\")

pub fn titlePairsBetween(start: int, end: int)
    for id, post in ^posts.byDate(start..end)
        print($\"{id}: {post.title}\")

pub fn valuesBetween(start: int, end: int)
    for k, post in ^posts.byDate(start..end)
        print(post.title)

pub fn entriesBetween(start: int, end: int)
    for id, post in ^posts.byDate(start..end)
        print($\"{id}: {post.title}\")

pub fn countBetween(start: int, end: int): int
    return count(^posts.byDate(start..end))

pub fn existsBetween(start: int, end: int): bool
    return exists(^posts.byDate(start..end))

pub fn titlesFrom(start: int)
    for id in ^posts.byDate(start..)
        print(^posts(id).title ?? \"\")

pub fn titlesBefore(end: int)
    for id in ^posts.byDate(..end)
        print(^posts(id).title ?? \"\")

pub fn titlesThrough(end: int)
    for id in ^posts.byDate(..=end)
        print(^posts(id).title ?? \"\")

pub fn titlesAfter(lastSeen: int, end: int)
    for id in ^posts.byDate(lastSeen..end)
        if (^posts(id).published ?? 0) != lastSeen
            print(^posts(id).title ?? \"\")

pub fn titlesBetweenInverted(start: int, end: int)
    for id in ^posts.byDate(start..end)
        print(^posts(id).title ?? \"\")
";

#[test]
fn iterates_index_keys() {
    let program = checked_program(&book_shelf());
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

    let count = |shelf: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str(shelf.into())),
        )
        .expect("count")
        .value
    };
    assert_eq!(count("fiction"), Some(Value::Int(2)));
    assert_eq!(count("history"), Some(Value::Int(1)));
    assert_eq!(count("romance"), Some(Value::Int(0)));
}

#[test]
fn bare_index_iteration_streams_every_store_identity() {
    let program = checked_program(&book_shelf());
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

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::count_via_bare_index"),
    )
    .expect("run");
    assert_eq!(outcome.value, Some(Value::Int(3)));
}

#[test]
fn updating_an_indexed_field_while_iterating_that_index_faults() {
    let program = checked_program(&book_shelf());
    let store = TreeStore::memory();
    for (id, title) in [(1, "Mort"), (2, "Sourcery")] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str("fiction".into()),
            ),
        )
        .expect("add");
    }

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::reshelve_while_iterating"),
        ),
        RUN_TRAVERSAL,
    );
    let remaining = run_entry(
        &store,
        checked_entry!(&program, "test::count_on", Value::Str("fiction".into())),
    )
    .expect("count")
    .value;
    assert_eq!(remaining, Some(Value::Int(2)));
}

#[test]
fn updating_an_indexed_field_while_directly_iterating_that_index_faults() {
    let program = checked_program(&book_shelf());
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ),
    )
    .expect("add");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::reshelve_while_iterating_direct"),
        ),
        RUN_TRAVERSAL,
    );
}

#[test]
fn prints_titles_in_index_key_order() {
    let program = checked_program(&book_shelf());
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
    add(2, "Sourcery", "fiction");
    add(1, "Mort", "fiction");

    // The index yields ids in key order (1 then 2), regardless of insert order.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::titles_on", Value::Str("fiction".into())),
    )
    .expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

#[test]
fn bounded_index_range_streams_matching_records() {
    let program = checked_program(POST_DATES);
    let store = TreeStore::memory();
    for (id, title, published) in [(1, "A", 10), (2, "B", 20), (3, "C", 30), (4, "D", 40)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Int(published),
            ),
        )
        .expect("add");
    }

    let between = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlesBetween",
            Value::Int(20),
            Value::Int(40)
        ),
    )
    .expect("between");
    assert_eq!(between.output, "B\nC\n");

    let pairs = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlePairsBetween",
            Value::Int(20),
            Value::Int(40)
        ),
    )
    .expect("pairs");
    assert_eq!(pairs.output, "2: B\n3: C\n");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::countBetween",
                Value::Int(100),
                Value::Int(200)
            )
        )
        .expect("count empty range")
        .value,
        Some(Value::Int(0))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::existsBetween",
                Value::Int(100),
                Value::Int(200)
            )
        )
        .expect("exists empty range")
        .value,
        Some(Value::Bool(false))
    );

    let from = run_entry(
        &store,
        checked_entry!(&program, "test::titlesFrom", Value::Int(30)),
    )
    .expect("from");
    assert_eq!(from.output, "C\nD\n");

    let before = run_entry(
        &store,
        checked_entry!(&program, "test::titlesBefore", Value::Int(30)),
    )
    .expect("before");
    assert_eq!(before.output, "A\nB\n");

    let through = run_entry(
        &store,
        checked_entry!(&program, "test::titlesThrough", Value::Int(30)),
    )
    .expect("through");
    assert_eq!(through.output, "A\nB\nC\n");

    let page = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlesAfter",
            Value::Int(20),
            Value::Int(50)
        ),
    )
    .expect("page after");
    assert_eq!(page.output, "C\nD\n");

    let inverted = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlesBetweenInverted",
            Value::Int(40),
            Value::Int(20)
        ),
    )
    .expect("inverted");
    assert_eq!(inverted.output, "");
}

/// `values(...)` and `entries(...)` over a non-unique index branch materialize
/// the whole record at each streamed identity, exactly as the bare two-name loop
/// (`titlePairsBetween`) does. The single-name `values` form reads the record
/// value; the two-name `entries` form pairs the identity with that record.
#[test]
fn value_materialization_wrappers_stream_records_over_a_non_unique_index_branch() {
    let program = checked_program(POST_DATES);
    let store = TreeStore::memory();
    for (id, title, published) in [(1, "A", 10), (2, "B", 20), (3, "C", 30), (4, "D", 40)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Int(published),
            ),
        )
        .expect("add");
    }

    let values = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::valuesBetween",
            Value::Int(20),
            Value::Int(40)
        ),
    )
    .expect("values");
    assert_eq!(values.output, "B\nC\n");

    let entries = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::entriesBetween",
            Value::Int(20),
            Value::Int(40)
        ),
    )
    .expect("entries");
    assert_eq!(entries.output, "2: B\n3: C\n");
}

#[test]
fn bounded_enum_range_rejects_corrupt_ranged_component() {
    let program = checked_program(
        "enum Status\n\
         \x20   draft\n\
         \x20   published\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required status: Status\n\
         store ^books(id: int): Book\n\
         \x20   index byStatus(status, id)\n\
         \n\
         pub fn countDraftOrLater(): int\n\
         \x20   return count(^books.byStatus(Status::draft..))\n\
         \n\
         pub fn hasDraftOrLater(): bool\n\
         \x20   return exists(^books.byStatus(Status::draft..))\n\
         \n\
         pub fn printDraftOrLater()\n\
         \x20   for id in ^books.byStatus(Status::draft..)\n\
         \x20       print(id)\n",
    );
    let store = TreeStore::memory();
    // The entry sits under the valid `draft` member but carries a non-int id
    // key, so it lands inside the ranged member walk and fails closed when the
    // walked tuple cannot decode to a store identity.
    let draft = enum_member_catalog_id(&program, "Status", "draft")
        .as_str()
        .to_string();
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byStatus"),
            &[SavedKey::Str(draft), SavedKey::Str("not-an-int".into())],
            &[SavedKey::Int(1)],
            Vec::new(),
        )
        .expect("corrupt enum range entry");

    for entry in [
        "test::countDraftOrLater",
        "test::hasDraftOrLater",
        "test::printDraftOrLater",
    ] {
        assert_run_error(run_entry(&store, checked_entry!(&program, entry)), RUN_TYPE);
    }
}

/// An ordered range over an enum-typed index component must yield exactly the
/// identities whose component falls in the declared ordinal range. Enum index
/// keys are stored as content-independent member ids, which do not sort by
/// ordinal, so the range cannot be a raw key-byte span; it must walk the
/// in-range members in declaration order.
const PRIORITY_TASKS: &str = "\
enum Priority
    low
    medium
    high
resource Task
    required prio: Priority
store ^tasks(id: int): Task

    index byPrio(prio, id)

pub fn seed()
    ^tasks(1).prio = Priority::low
    ^tasks(2).prio = Priority::high
    ^tasks(3).prio = Priority::medium

pub fn idsInRange(): string
    var out = \"\"
    for id in ^tasks.byPrio(Priority::low..=Priority::high)
        out = out + $\"{id} \"
    return out

pub fn idsFromLow(): string
    var out = \"\"
    for id in ^tasks.byPrio(Priority::low..)
        out = out + $\"{id} \"
    return out

pub fn idsLowToMedium(): string
    var out = \"\"
    for id in ^tasks.byPrio(Priority::low..=Priority::medium)
        out = out + $\"{id} \"
    return out

pub fn idsMediumExact(): string
    var out = \"\"
    for id in ^tasks.byPrio(Priority::medium)
        out = out + $\"{id} \"
    return out

pub fn countInRange(): int
    return count(^tasks.byPrio(Priority::low..=Priority::high))

pub fn existsMediumOrHigh(): bool
    return exists(^tasks.byPrio(Priority::medium..=Priority::high))

pub fn breakAfterFirstBare(): string
    var out = \"\"
    var n = 0
    for id in ^tasks.byPrio
        out = out + $\"{id} \"
        n = n + 1
        if n == 1
            break
    return out

pub fn breakAfterFirstRange(): string
    var out = \"\"
    var n = 0
    for id in ^tasks.byPrio(Priority::low..=Priority::high)
        out = out + $\"{id} \"
        n = n + 1
        if n == 1
            break
    return out

pub fn breakAfterFirstPairs(): string
    var out = \"\"
    var n = 0
    for id, task in ^tasks.byPrio
        out = out + $\"{id} \"
        n = n + 1
        if n == 1
            break
    return out

pub fn breakAfterFirstReversed(): string
    var out = \"\"
    var n = 0
    for id in reversed ^tasks.byPrio
        out = out + $\"{id} \"
        n = n + 1
        if n == 1
            break
    return out
";

#[test]
fn ranged_enum_index_component_yields_ordinal_range_identities() {
    let program = checked_program(PRIORITY_TASKS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let call_str = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry))
            .expect(entry)
            .value
    };
    // Identities sort by (prio ordinal, id), so low(1), medium(3), high(2).
    assert_eq!(
        call_str("test::idsInRange"),
        Some(Value::Str("1 3 2 ".into()))
    );
    assert_eq!(
        call_str("test::idsFromLow"),
        Some(Value::Str("1 3 2 ".into()))
    );
    assert_eq!(
        call_str("test::idsLowToMedium"),
        Some(Value::Str("1 3 ".into()))
    );
    assert_eq!(
        call_str("test::idsMediumExact"),
        Some(Value::Str("3 ".into()))
    );
    assert_eq!(call_str("test::countInRange"), Some(Value::Int(3)));
    assert_eq!(
        call_str("test::existsMediumOrHigh"),
        Some(Value::Bool(true))
    );
}

/// A `break` in the loop body must stop the whole index walk even when the break
/// lands at an enum-member boundary. Each in-range enum member is walked as its
/// own keyed sub-scan, so the member loop must observe the body break rather than
/// reading the first sub-scan's natural completion as permission to advance to the
/// next member. The break here fires after the first identity, which sits exactly
/// on the boundary between the first and second populated member.
#[test]
fn break_stops_enum_led_index_walk_at_member_boundary() {
    let program = checked_program(PRIORITY_TASKS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let call_str = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry))
            .expect(entry)
            .value
    };
    // Ascending identity order is low(1), medium(3), high(2); reversed is the
    // reverse. A break after the first identity must yield exactly one id.
    assert_eq!(
        call_str("test::breakAfterFirstBare"),
        Some(Value::Str("1 ".into()))
    );
    assert_eq!(
        call_str("test::breakAfterFirstRange"),
        Some(Value::Str("1 ".into()))
    );
    assert_eq!(
        call_str("test::breakAfterFirstPairs"),
        Some(Value::Str("1 ".into()))
    );
    assert_eq!(
        call_str("test::breakAfterFirstReversed"),
        Some(Value::Str("2 ".into()))
    );
}

/// A `bool`-keyed index. `bool` sorts `false` (0x00) before `true` (0x01) through
/// the order-preserving key encoding, so a `bool` index component ranges like any
/// other ordered key.
const TASK_DONE_FLAG: &str = "\
resource Task
    required title: string
    required done: bool
store ^tasks(id: int): Task

    index byDone(done, id)

pub fn add(id: int, title: string, done: bool)
    ^tasks(id) = Task(title: title, done: done)

pub fn titlesInRange()
    for id in ^tasks.byDone(false..=true)
        print(^tasks(id).title ?? \"\")

pub fn titlesExclusive()
    for id in ^tasks.byDone(false..true)
        print(^tasks(id).title ?? \"\")

pub fn titlesDone(done: bool)
    for id in ^tasks.byDone(done)
        print(^tasks(id).title ?? \"\")
";

#[test]
fn bool_index_component_ranges_in_key_order() {
    let program = checked_program(TASK_DONE_FLAG);
    let store = TreeStore::memory();
    for (id, title, done) in [(1, "a", false), (2, "b", true), (3, "c", false)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Bool(done),
            ),
        )
        .expect("add");
    }

    // The inclusive range covers both flag values, false-keyed (1, 3) before
    // true-keyed (2).
    let inclusive = run_entry(&store, checked_entry!(&program, "test::titlesInRange"))
        .expect("inclusive range");
    assert_eq!(inclusive.output, "a\nc\nb\n");

    // The exclusive upper bound drops the true-keyed entry, leaving only false.
    let exclusive = run_entry(&store, checked_entry!(&program, "test::titlesExclusive"))
        .expect("exclusive range");
    assert_eq!(exclusive.output, "a\nc\n");

    // Exact lookups still partition by flag.
    let done = run_entry(
        &store,
        checked_entry!(&program, "test::titlesDone", Value::Bool(true)),
    )
    .expect("done");
    assert_eq!(done.output, "b\n");
    let not_done = run_entry(
        &store,
        checked_entry!(&program, "test::titlesDone", Value::Bool(false)),
    )
    .expect("not done");
    assert_eq!(not_done.output, "a\nc\n");
}
