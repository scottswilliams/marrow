//! Engine-call witness for the path kernel's pre-engine check order.
//!
//! The kernel resolves and validates every rejectable pre-engine condition before
//! the store's byte engine is touched for that work, so a rejected operation performs
//! no spurious engine access. This harness proves the operationally-witnessable
//! classes rather than reading the source: a counting engine wraps the in-memory
//! backend and tallies both store *opens* (`read_view`, `begin`, `audit_integrity`)
//! and transaction *writes* (`put`, `remove`) through shared counters the test reads
//! independently of the store.
//!
//! The pre-engine check order (the invariant) is:
//!
//! ```text
//! verified site → active binding → view/invocation state → typed operands
//!   → derived address without I/O → demand/ceiling/grant/budgets
//! ```
//!
//! Each rejection class and how its zero-engine-call property is established:
//!
//! | Rejection class            | Where established                    | Zero-call proof |
//! |----------------------------|--------------------------------------|-----------------|
//! | verified site              | verifier (phase 3) rejects an opcode  | by construction: a verified image names only sealed sites; the kernel resolves them from an in-memory table built with no engine call |
//! | active binding             | type system + `VerifiedImage`         | by construction: an attachment is a live owned handle, and a forged image cannot be verified, so it can never mint one |
//! | view/invocation state      | verifier + typed session              | by construction: a read-only session's mutation ops are `unreachable!` (verifier-proven), and a committed transaction is consumed, so no op runs after it |
//! | typed operands             | VM (before the kernel op)             | operational (below): a value the codec cannot represent faults before any engine write |
//! | derived address without I/O | kernel (pure codec)                  | operational (below): the physical address and value are computed in memory; a rejected write stages zero engine writes |
//! | demand/ceiling/grant/budgets | kernel session open (pure)          | operational (below): a denied session returns before the store's first engine access, with the open tally at zero |
//!
//! The boundary: [`SessionError::ProfileMismatch`] is detected by reading the store's
//! recorded profile cell — the session's *first* engine access. It is therefore
//! deliberately outside the zero-engine-call set; the zero-call property covers
//! exactly the classes ordered before the first engine access. Once the profile
//! matches and the view or transaction is open, ordinary engine access begins.

mod common;

use common::{Counters, CountingEngine};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    DemandCoverage, Durable, DurableStore, FieldSchema, InvocationGrant, KernelFault, SessionError,
    SiteSpec, SiteTarget, StoreSchema,
};

fn schema() -> StoreSchema {
    StoreSchema {
        root_name: "counters".into(),
        key: ScalarKind::Int,
        fields: vec![FieldSchema {
            name: "value".into(),
            kind: ScalarKind::Int,
            required: true,
        }],
        branches: Vec::new(),
    }
}

/// A whole-payload entry site (index 0) and the required `value` field site (index 1).
fn sites() -> Vec<SiteSpec> {
    vec![
        SiteSpec {
            target: SiteTarget::WholePayload,
        },
        SiteSpec {
            target: SiteTarget::FieldLeaf(0),
        },
    ]
}

fn read_only_ceiling() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: false,
    }
}

fn writing_demand() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

// --- The demand/ceiling/grant class: denied before the store's first access. ---

/// A writing demand under a read-only ceiling is denied at the transaction-session
/// open, and the engine is never touched.
#[test]
fn a_denied_transaction_open_makes_zero_engine_calls() {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(),
        sites(),
        read_only_ceiling(),
    );
    let denied = store.txn_session(InvocationGrant::full_store(), writing_demand());
    assert!(matches!(denied, Err(SessionError::Denied)));
    assert_eq!(
        counters.opens(),
        0,
        "a denied authority check must open the engine zero times",
    );
    assert_eq!(counters.writes(), 0, "a denied open stages no writes");
}

/// A read demand denied by a no-read grant is refused before the read view opens.
#[test]
fn a_denied_read_open_makes_zero_engine_calls() {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(),
        sites(),
        read_only_ceiling(),
    );
    let no_read_grant = InvocationGrant {
        read: false,
        write: false,
    };
    let denied = store.read_session(
        no_read_grant,
        DemandCoverage {
            read: true,
            write: false,
        },
    );
    assert!(matches!(denied, Err(SessionError::Denied)));
    assert_eq!(
        counters.opens(),
        0,
        "a denied read must open the engine zero times"
    );
}

/// The witness is real: a permitted session reads the profile cell, so it opens the
/// engine a nonzero number of times. Without this a broken counter would pass the
/// zero-call assertions vacuously. This also pins the boundary: the profile read is
/// the session's first engine access, the point where `ProfileMismatch` is decided.
#[test]
fn a_permitted_open_makes_a_nonzero_number_of_engine_calls() {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(),
        sites(),
        read_only_ceiling(),
    );
    let read = store.read_session(
        InvocationGrant::full_store(),
        DemandCoverage {
            read: true,
            write: false,
        },
    );
    assert!(read.is_ok());
    drop(read);
    assert!(
        counters.opens() > 0,
        "a permitted read reads the profile cell, so the counter must observe it",
    );
}

// --- The typed-operand / derived-address class: validated in memory before I/O. ---

/// A mutating operation given a value the canonical codec cannot represent returns
/// the typed `ValueRange` fault and stages zero engine writes: the kernel derives the
/// physical address and encodes the value in memory before it would put, so a
/// rejected write never reaches the engine's write path.
#[test]
fn a_value_range_rejection_stages_zero_engine_writes() {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(),
        sites(),
        DemandCoverage {
            read: true,
            write: true,
        },
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), writing_demand())
        .expect("txn session");
    let field = txn.site(1);
    let writes_before = counters.writes();
    // A date beyond the year-9999 canonical bound cannot encode; the op must reject
    // it before any engine write.
    let rejected = txn.set_required(&field, &[KeyScalar::Int(1)], RuntimeScalar::Date(i32::MAX));
    assert_eq!(rejected, Err(KernelFault::ValueRange));
    assert_eq!(
        counters.writes(),
        writes_before,
        "a value the codec rejects must stage zero engine writes",
    );
}

/// The write counter is real: an in-range required set stages exactly one engine
/// write, so the zero-write assertion above is not vacuous.
#[test]
fn an_in_range_write_advances_the_write_counter() {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(),
        sites(),
        DemandCoverage {
            read: true,
            write: true,
        },
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), writing_demand())
        .expect("txn session");
    let field = txn.site(1);
    let writes_before = counters.writes();
    txn.set_required(&field, &[KeyScalar::Int(1)], RuntimeScalar::Int(7))
        .expect("in-range set");
    assert!(
        counters.writes() > writes_before,
        "an in-range write must advance the write counter",
    );
}
