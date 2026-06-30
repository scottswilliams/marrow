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

/// Single-int records whose serialized payload is a handful of bytes but whose
/// real buffered footprint (record presence cell, field cell, redb pending-tree
/// page overhead, allocator slack) is kilobytes apiece. The breadth cap must
/// reflect that real footprint, not the logical payload, or a tiny-payload flood
/// buffers gigabytes while staying nominally "under" the byte budget.
const TINY_RECORD_SEED: &str = "\
resource Num
    val: int
store ^nums(id: int): Num

pub fn seedTx(n: int)
    transaction
        for id in 1..=n
            ^nums(id).val = id

pub fn seedBatched(n: int, batch: int)
    var start = 1
    while start <= n
        var stop = start + batch - 1
        if stop > n
            stop = n
        transaction
            for id in start..=stop
                ^nums(id).val = id
        start = stop + 1

pub fn numCount(): int
    var c = 0
    for x in ^nums
        c = c + 1
    return c
";

/// Records whose field value is a single int but whose identity carries a
/// multi-kilobyte string key. The serialized value is a handful of bytes, yet the
/// key bytes ride every staged step and the pending-tree key, so the real buffered
/// footprint scales with the key. Metering only the value plus a flat per-cell
/// constant would let a large composite key buffer gigabytes while staying
/// nominally "under" the byte budget; the cap must charge the key bytes too.
const KEYED_SEED: &str = "\
resource Tag
    weight: int
store ^tags(label: string, id: int): Tag

pub fn bigStr(doublings: int): string
    var s = \"k\"
    for i in 1..=doublings
        s = s + s
    return s

pub fn seedKeyed(n: int, doublings: int)
    const label = bigStr(doublings)
    transaction
        for id in 1..=n
            ^tags(label, id).weight = id

pub fn tagCount(): int
    var c = 0
    for t in ^tags
        c = c + 1
    return c
";

/// A single transaction of this many records, each keyed by a ~32 KiB string,
/// buffers far past 64 MiB of real memory while its logical value payload is a few
/// kilobytes. Absent the key-byte charge the flat per-cell overhead alone meters
/// only about ten mebibytes for this count, so the cap would not trip; charging
/// the key bytes trips it before real memory crosses the budget.
const KEYED_RECORDS_OVER_BUDGET: i64 = 2_000;

/// About 32 KiB per string key (`2^15` bytes).
const LARGE_KEY_DOUBLINGS: i64 = 15;

/// A few thousand records keyed by a ~256-byte string is an ordinary keyed seed
/// whose real footprint is a small fraction of the budget; it must commit.
const KEYED_RECORDS_UNDER_BUDGET: i64 = 3_000;

/// About 256 bytes per string key (`2^8` bytes).
const MODERATE_KEY_DOUBLINGS: i64 = 8;

/// A single transaction of this many single-int records buffers far past 64 MiB
/// of real memory while its logical payload is only a few megabytes. The cap must
/// trip on the real footprint.
const TINY_RECORDS_OVER_BUDGET: i64 = 30_000;

/// The same total, committed in sub-transactions whose real footprint each stays
/// well under the budget, must all succeed.
const TINY_BATCH_SIZE: i64 = 5_000;

#[test]
fn a_tiny_payload_flood_aborts_on_real_buffered_footprint() {
    // Single-int records carry only a few payload bytes each, yet their real
    // buffered footprint is kilobytes apiece. Metering the logical payload would
    // let this transaction buffer gigabytes before the cap noticed; metering the
    // real footprint stops it while the buffer is still bounded.
    let program = checked_program(TINY_RECORD_SEED);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seedTx",
            Value::Int(TINY_RECORDS_OVER_BUDGET)
        ),
    );
    assert_run_error(result, WRITE_TRANSACTION_TOO_LARGE);

    // The aborted transaction committed nothing.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::numCount"))
            .expect("count survivors")
            .value,
        Some(Value::Int(0)),
        "a capped tiny-payload transaction commits nothing",
    );
}

#[test]
fn the_same_total_in_batched_sub_transactions_commits() {
    // Splitting the identical record count into sub-transactions, each bounded
    // well under the budget, succeeds: the cap bounds a single transaction's
    // buffer, not the total work a program may do across commits.
    let program = checked_program(TINY_RECORD_SEED);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seedBatched",
            Value::Int(TINY_RECORDS_OVER_BUDGET),
            Value::Int(TINY_BATCH_SIZE),
        ),
    )
    .expect("batched sub-transactions commit");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::numCount"))
            .expect("count records")
            .value,
        Some(Value::Int(TINY_RECORDS_OVER_BUDGET)),
        "every batched record persists",
    );
}

#[test]
fn a_moderate_tiny_payload_transaction_is_unaffected() {
    // A few thousand single-int records is an ordinary atomic seed whose real
    // footprint is a small fraction of the budget; it must commit untouched.
    let program = checked_program(TINY_RECORD_SEED);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seedTx", Value::Int(2_000)),
    )
    .expect("a moderate transaction commits");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::numCount"))
            .expect("count records")
            .value,
        Some(Value::Int(2_000)),
        "every record in a moderate transaction persists",
    );
}

#[test]
fn a_large_composite_key_transaction_aborts_on_real_buffered_footprint() {
    // Each record's field value is one int, but its identity carries a ~32 KiB
    // string key that rides every staged step and the pending-tree key. Metering
    // only the value plus a flat per-cell constant meters about ten mebibytes for
    // this record count, far under the cap; charging the real key bytes trips the
    // cap while the buffer is still bounded, before real memory crosses 64 MiB.
    let program = checked_program(KEYED_SEED);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seedKeyed",
            Value::Int(KEYED_RECORDS_OVER_BUDGET),
            Value::Int(LARGE_KEY_DOUBLINGS),
        ),
    );
    assert_run_error(result, WRITE_TRANSACTION_TOO_LARGE);

    // The aborted transaction committed nothing.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tagCount"))
            .expect("count survivors")
            .value,
        Some(Value::Int(0)),
        "a capped large-key transaction commits nothing",
    );
}

#[test]
fn a_moderate_composite_key_transaction_is_unaffected() {
    // A few thousand records keyed by a ~256-byte string is an ordinary keyed seed
    // whose real footprint is a small fraction of the budget; it must commit
    // untouched, so the key-byte charge does not penalize moderate keyed writes.
    let program = checked_program(KEYED_SEED);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seedKeyed",
            Value::Int(KEYED_RECORDS_UNDER_BUDGET),
            Value::Int(MODERATE_KEY_DOUBLINGS),
        ),
    )
    .expect("a moderate keyed transaction commits");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tagCount"))
            .expect("count records")
            .value,
        Some(Value::Int(KEYED_RECORDS_UNDER_BUDGET)),
        "every record in a moderate keyed transaction persists",
    );
}

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
