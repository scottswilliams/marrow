//! A transaction that aborts on a unique conflict rolls back every staged write,
//! and a retry against the same root and index then commits cleanly. This pins
//! the recovery path a real application takes on a unique collision: the failed
//! attempt leaves no partial record and no stale index entry, so the index stays
//! usable and the retried value persists.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;
use marrow_store::tree::TreeStore;

/// Two books share a root with a unique `isbn` index. `tryRegister` runs the
/// whole registration in one transaction: it writes a sibling field (`shelf`)
/// first, then claims the isbn. When the isbn collides the conflict escapes the
/// transaction; an outer `try` catches it, the transaction rolls back, and the
/// caller retries with a free isbn. The entry returns the first attempt's error
/// code so the conflict is observable as a typed code, not just a final state.
const REGISTER_RETRY: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
    isbn: string

    index byIsbn(isbn) unique

pub fn seed(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

pub fn registerWithRetry(id: int, t: string, taken: string, free: string): string
    var firstCode = \"\"
    try
        transaction
            ^books(id).title = t
            ^books(id).shelf = \"staged\"
            ^books(id).isbn = taken
    catch err: Error
        firstCode = err.code
        transaction
            ^books(id).title = t
            ^books(id).isbn = free
    return firstCode

pub fn shelfOf(id: int): string
    return ^books(id).shelf ?? \"<absent>\"

pub fn isbnOf(id: int): string
    return ^books(id).isbn ?? \"<absent>\"

pub fn titleOf(id: int): string
    return ^books(id).title ?? \"<absent>\"

pub fn ownerOf(isbn: string): Id(^books)
    for id in ^books.byIsbn(isbn)
        return id
    throw Error(code: \"test.no_owner\", message: \"no owner\")

pub fn hasIsbn(isbn: string): bool
    return exists(^books.byIsbn(isbn))
";

fn seeded_store(program: &marrow_check::CheckedRuntimeProgram) -> TreeStore {
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("taken-isbn".into())
        ),
    )
    .expect("seed the existing owner of the taken isbn");
    store
}

#[test]
fn a_rolled_back_transaction_retries_and_persists_the_non_conflicting_value() {
    // Book 2's first transaction stages `shelf` and `title`, then collides on the
    // taken isbn. The conflict escapes the transaction (rollback), the outer catch
    // binds the typed `write.unique_conflict` code, and the retry commits a free
    // isbn. The first-attempt code proves the abort was the unique conflict, not a
    // generic fault.
    let program = checked_program(REGISTER_RETRY);
    let store = seeded_store(&program);

    let first_code = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::registerWithRetry",
            Value::Int(2),
            Value::Str("Reaper".into()),
            Value::Str("taken-isbn".into()),
            Value::Str("free-isbn".into()),
        ),
    )
    .expect("registration recovers after the conflict")
    .value;
    assert_eq!(
        first_code,
        Some(Value::Str("write.unique_conflict".into())),
        "the first transaction aborted on the unique conflict"
    );

    // The retry's value is durable: book 2 owns the free isbn and the title it set.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::isbnOf", Value::Int(2))
        )
        .expect("read isbn")
        .value,
        Some(Value::Str("free-isbn".into())),
        "the retried isbn committed"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(2))
        )
        .expect("read title")
        .value,
        Some(Value::Str("Reaper".into())),
        "the retried title committed"
    );
}

#[test]
fn the_rolled_back_transaction_leaves_no_partial_sibling_write() {
    // The aborted transaction wrote `shelf` before it hit the conflict on `isbn`.
    // Rollback must rewind that sibling too: the retry never sets `shelf`, so a
    // `shelf` that survived would be debris from the rolled-back attempt.
    let program = checked_program(REGISTER_RETRY);
    let store = seeded_store(&program);

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::registerWithRetry",
            Value::Int(2),
            Value::Str("Reaper".into()),
            Value::Str("taken-isbn".into()),
            Value::Str("free-isbn".into()),
        ),
    )
    .expect("registration recovers after the conflict");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::shelfOf", Value::Int(2))
        )
        .expect("read shelf")
        .value,
        Some(Value::Str("<absent>".into())),
        "the staged shelf rolled back with the aborted transaction"
    );
}

#[test]
fn the_index_stays_usable_after_a_rolled_back_conflict() {
    // After the abort and retry, the unique index resolves the original owner and
    // the retried owner to their own records, and the rolled-back `taken-isbn`
    // claim never created a second mapping. The aborted attempt left no stale or
    // half-applied index entry.
    let program = checked_program(REGISTER_RETRY);
    let store = seeded_store(&program);

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::registerWithRetry",
            Value::Int(2),
            Value::Str("Reaper".into()),
            Value::Str("taken-isbn".into()),
            Value::Str("free-isbn".into()),
        ),
    )
    .expect("registration recovers after the conflict");

    // The taken isbn still maps to its original owner, book 1 — the aborted claim
    // did not steal or duplicate it.
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::ownerOf", Value::Str("taken-isbn".into())),
        )
        .expect("taken isbn owner")
        .value,
        "books",
        &[marrow_store::key::SavedKey::Int(1)],
    );
    // The free isbn maps to the retried record, book 2.
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::ownerOf", Value::Str("free-isbn".into())),
        )
        .expect("free isbn owner")
        .value,
        "books",
        &[marrow_store::key::SavedKey::Int(2)],
    );
    // Both live records resolve through the index; the aborted claim left no third
    // mapping behind.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::hasIsbn", Value::Str("taken-isbn".into())),
        )
        .expect("present lookup")
        .value,
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::hasIsbn", Value::Str("free-isbn".into())),
        )
        .expect("present lookup")
        .value,
        Some(Value::Bool(true)),
    );
}
