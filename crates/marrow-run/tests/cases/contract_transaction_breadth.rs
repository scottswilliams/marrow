//! Transaction breadth fails closed. A single `transaction` buffers its whole
//! pending write set in memory; without a ceiling a large but legitimate atomic
//! seed grows the buffer until the process is OOM-killed (an uncatchable
//! SIGKILL). A typed, catchable cap stops the transaction with
//! `write.transaction_too_large` before memory exhausts, and the aborted
//! transaction commits nothing.

use crate::support;
use support::*;

use marrow_run::{TRANSACTION_WRITE_BYTE_BUDGET, Value, WRITE_TRANSACTION_TOO_LARGE};
use marrow_store::tree::TreeStore;

/// One saved root plus entries that build oversized transactions. `mib(n)`
/// doubles a one-character string into a field value of about `2^n` bytes, so a
/// small record count crosses the byte budget without an enormous loop. The
/// loop-built writer scales by record count; the bare writer unrolls three very
/// large records so the cap must trip on the accumulated write set, not on any
/// loop-specific path. `tryBulk` wraps the loop-built writer in a handler so the
/// abort is observed as a bound `Error`, proving the fault is catchable rather
/// than a process abort.
const BULK_SEED: &str = "\
resource Doc
    required body: string
store ^docs(id: int): Doc

pub fn mib(n: int): string
    var s = \"x\"
    for i in 1..=n
        s = s + s
    return s

pub fn bulkLoop(n: int)
    const body = mib(20)
    transaction
        for id in 1..=n
            ^docs(id).body = body

pub fn bulkBare()
    const body = mib(25)
    transaction
        ^docs(1).body = body
        ^docs(2).body = body
        ^docs(3).body = body

pub fn smallBulk()
    const body = mib(10)
    transaction
        ^docs(1).body = body
        ^docs(2).body = body
        ^docs(3).body = body

pub fn tryBulk(n: int): string
    try
        bulkLoop(n)
    catch err: Error
        return err.code
    return \"committed\"

pub fn docCount(): int
    var c = 0
    for doc in ^docs
        c = c + 1
    return c

pub fn hasDoc(id: int): bool
    return exists(^docs(id))
";

/// Loop-built records of about one mebibyte each. The byte budget is generous
/// enough that a normal seed never approaches it, so a loop large enough to cross
/// it stays well under the multi-gigabyte point where the process would be
/// OOM-killed.
const RECORDS_OVER_BUDGET: i64 = 256;

#[test]
fn the_transaction_breadth_budget_is_fixed_at_sixty_four_mebibytes() {
    // The breadth cap is fixed in v0.1, not configurable. Pinning the value here
    // makes it a checked contract rather than a comment, mirroring the call-depth
    // budget, and anchors the public export so its only consumer is a real test.
    assert_eq!(TRANSACTION_WRITE_BYTE_BUDGET, 64 * 1024 * 1024);
}

#[test]
fn a_loop_built_oversized_transaction_aborts_with_the_typed_cap() {
    let program = checked_program(BULK_SEED);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::bulkLoop", Value::Int(RECORDS_OVER_BUDGET)),
    );
    assert_run_error(result, WRITE_TRANSACTION_TOO_LARGE);
}

#[test]
fn a_bare_oversized_transaction_aborts_with_the_typed_cap() {
    // The bare form unrolls three ~32 MiB records rather than looping, so the cap
    // must trip on the accumulated write set itself, not on a loop-specific path.
    let program = checked_program(BULK_SEED);
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::bulkBare"));
    assert_run_error(result, WRITE_TRANSACTION_TOO_LARGE);
}

#[test]
fn an_oversized_transaction_abort_is_catchable_and_commits_nothing() {
    let program = checked_program(BULK_SEED);
    let store = TreeStore::memory();

    // The cap surfaces as a bound `Error`, so a surrounding handler resumes; it is
    // not a process abort.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tryBulk", Value::Int(RECORDS_OVER_BUDGET))
        )
        .expect("the cap is catchable")
        .value,
        Some(Value::Str(WRITE_TRANSACTION_TOO_LARGE.into())),
    );

    // Atomicity: the aborted transaction left no record behind.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::docCount"))
            .expect("count survivors")
            .value,
        Some(Value::Int(0)),
        "a capped transaction commits nothing",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::hasDoc", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the first staged record rolled back with the rest",
    );
}

#[test]
fn a_transaction_just_under_the_cap_commits() {
    // Three ~1 MiB records stay well under the budget and commit normally, proving
    // the cap does not disturb ordinary transactions.
    let program = checked_program(BULK_SEED);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::smallBulk")).expect("small bulk commits");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::docCount"))
            .expect("count records")
            .value,
        Some(Value::Int(3)),
        "every record in an under-cap transaction persists",
    );
}
