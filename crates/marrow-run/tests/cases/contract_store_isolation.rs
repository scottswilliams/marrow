//! An in-memory store does not leak saved state across runs. A fresh
//! `TreeStore::memory()` starts empty, and writes committed against one store are
//! invisible to a separately-constructed store. This pins the ephemeral-store
//! boundary: each store is its own world, so a run against a new store never sees
//! another store's records — the contract a test, a sandbox, or a per-request
//! store relies on.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::tree::TreeStore;

const COUNTER: &str = "\
resource Note
    required body: string
store ^notes(id: int): Note

pub fn write_note(id: int, body: string)
    ^notes(id).body = body

pub fn body_of(id: int): string
    return ^notes(id).body ?? \"\"

pub fn note_count(): int
    return count(^notes)
";

#[test]
fn a_fresh_in_memory_store_starts_empty() {
    // A newly constructed store has no records: the root count is zero and a
    // resolved read of a never-written field takes its explicit fallback.
    let program = checked_program(COUNTER);
    let store = TreeStore::memory();

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::note_count"))
            .expect("count empty store")
            .value,
        Some(Value::Int(0)),
        "a fresh store has no records",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::body_of", Value::Int(1)),
        )
        .expect("read absent note with fallback")
        .value,
        Some(Value::Str("".into())),
    );
}

#[test]
fn a_write_in_one_store_is_invisible_to_a_separate_store() {
    // Two independently-constructed stores are isolated. A note committed to the
    // first store is readable there, and the second store — built fresh — neither
    // counts it nor reads it back. The write did not leak through shared process
    // state into a sibling store.
    let program = checked_program(COUNTER);

    let first = TreeStore::memory();
    run_entry(
        &first,
        checked_entry!(
            &program,
            "test::write_note",
            Value::Int(1),
            Value::Str("kept".into())
        ),
    )
    .expect("write into the first store");
    assert_eq!(
        run_entry(
            &first,
            checked_entry!(&program, "test::body_of", Value::Int(1))
        )
        .expect("read from the first store")
        .value,
        Some(Value::Str("kept".into())),
    );

    // A separately-built store is unaffected by the first store's commit.
    let second = TreeStore::memory();
    assert_eq!(
        run_entry(&second, checked_entry!(&program, "test::note_count"))
            .expect("count the second store")
            .value,
        Some(Value::Int(0)),
        "the second store is empty despite the first store's write",
    );
    assert_eq!(
        run_entry(
            &second,
            checked_entry!(&program, "test::body_of", Value::Int(1)),
        )
        .expect("read absent note from second store with fallback")
        .value,
        Some(Value::Str("".into())),
    );

    // The first store still holds its write: reading the second did not disturb it.
    assert_eq!(
        run_entry(&first, checked_entry!(&program, "test::note_count"))
            .expect("re-count the first store")
            .value,
        Some(Value::Int(1)),
    );
}

#[test]
fn each_default_run_uses_its_own_empty_store() {
    // The shared `run` helper builds a fresh store per call. A write committed by
    // one such run is invisible to the next: consecutive default-store runs do not
    // accumulate state, which is what keeps store-backed tests independent.
    let program = checked_program(COUNTER);

    run(checked_entry!(
        &program,
        "test::write_note",
        Value::Int(1),
        Value::Str("ephemeral".into())
    ))
    .expect("write against a default store");

    // A following default-store run sees an empty store, not the previous write.
    assert_eq!(
        run(checked_entry!(&program, "test::note_count")).expect("count a fresh default store"),
        Some(Value::Int(0)),
        "a new default-store run does not see the prior run's write",
    );
}
