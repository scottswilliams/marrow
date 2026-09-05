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
//! The boundary: a permitted session opens the engine (a read view or a write
//! transaction) as its *first* engine access. The zero-call property covers exactly the
//! rejection classes ordered before that first access — authority denial and the poison
//! latch, both decided in memory. Once the session is open, ordinary engine access begins.
//! (Schema-binding consistency is no longer an in-store profile read: the id-keyed layout
//! makes a rename zero-cell, so the lifecycle head owns binding, not a profile cell.)

mod common;

use common::{Counters, CountingEngine};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    BoundedLimit, CommitResult, DemandCoverage, Durable, DurableStore, EntryValue, InvocationGrant,
    KernelFault, Presence, SessionError, SiteTarget, StoreProjection, StoreSchema,
    StoreSchemaBuilder,
};
use marrow_kernel::equality::ValueDomain;

/// The single-root projection a case opens under: the root, plus its sites resolved against
/// it. Every site here names root 0 — the store's only root.
fn project(schema: &StoreSchema, sites: Vec<SiteTarget>) -> StoreProjection {
    let mut projection = StoreProjection::builder();
    projection.root(schema.clone());
    for target in sites {
        projection.site(0, target);
    }
    projection
        .finish()
        .expect("every site names the one declared root")
}

fn schema() -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Int]);
    builder.scalar_field("value", ScalarKind::Int, true);
    builder.finish().expect("a bounded schema builds")
}

/// A whole-payload entry site (index 0) and the required `value` field site (index 1).
fn sites() -> Vec<SiteTarget> {
    vec![SiteTarget::whole_payload(), SiteTarget::field_leaf(0)]
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
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&schema(), sites()),
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
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&schema(), sites()),
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

/// The witness is real: a permitted session opens the engine (a read view), so it makes a
/// nonzero number of engine opens. Without this a broken counter would pass the zero-call
/// assertions vacuously. This also pins the boundary: the view open is the session's first
/// engine access, after the in-memory authority and poison-latch checks.
#[test]
fn a_permitted_open_makes_a_nonzero_number_of_engine_calls() {
    let counters = Counters::new();
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&schema(), sites()),
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
        "a permitted read opens the engine view, so the counter must observe it",
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
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&schema(), sites()),
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
    let rejected = txn.set_required(
        &field,
        &[KeyScalar::Int(1)],
        ValueDomain::Scalar(RuntimeScalar::Date(i32::MAX)),
    );
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
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&schema(), sites()),
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
    txn.set_required(
        &field,
        &[KeyScalar::Int(1)],
        ValueDomain::Scalar(RuntimeScalar::Int(7)),
    )
    .expect("in-range set");
    assert!(
        counters.writes() > writes_before,
        "an in-range write must advance the write counter",
    );
}

// --- The layer-walk bound class: a family walk costs its bound, not its population. ---
//
// Both witnesses are red until the B2 complete-entries vertical relocates entry presence
// into an ordered family namespace. Today `layer_step` meets every descendant-only
// sibling in the marker walk and seeks past each one, so a bound on frozen keys is not a
// bound on engine seeks.

/// A `books` root keyed by string with a required `title`, and a `notes` branch keyed by
/// int with a required `text`. Site 0 is the root entry, site 1 the branch entry.
fn layered_schema() -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_branch("notes", vec![ScalarKind::Int]);
    builder.scalar_field("text", ScalarKind::Str, true);
    builder.close_branch();
    builder.finish().expect("the layered schema builds")
}

fn layered_sites() -> Vec<SiteTarget> {
    vec![
        SiteTarget::whole_payload(),
        SiteTarget::branch_entry(vec![0u16]),
    ]
}

fn text_entry(text: &str) -> EntryValue {
    EntryValue {
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str(text.into())))],
        groups: Vec::new(),
    }
}

/// A counting store whose root family holds `present` books with their own payload and
/// `descendant_only` books that carry one note each and no payload of their own.
fn layered_store(
    present: &[&str],
    descendant_only: &[&str],
) -> (DurableStore<CountingEngine>, Counters) {
    let counters = Counters::new();
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&layered_schema(), layered_sites()),
        writing_demand(),
    );
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), writing_demand())
            .expect("txn session");
        let root = txn.site(0);
        let note = txn.site(1);
        for key in present {
            txn.create_entry(&root, &[KeyScalar::Str((*key).into())], text_entry("T"))
                .expect("create a present book");
        }
        for key in descendant_only {
            txn.create_entry(
                &note,
                &[KeyScalar::Str((*key).into()), KeyScalar::Int(7)],
                text_entry("hi"),
            )
            .expect("create a note under an absent book");
        }
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    (store, counters)
}

fn bound(n: u32) -> BoundedLimit {
    BoundedLimit::new(n).expect("positive bound")
}

/// `at most 1` over `[k1 present, k2 and k3 descendant-only, k4 present]` freezes `k1`
/// and flags `more` in exactly two seeks: one per frozen key plus the one that finds the
/// `(N + 1)`th present key. Today the walk also seeks past `k2` and `k3` (four seeks),
/// so the count grows with the descendant-only population the bound never names.
#[test]
#[ignore = "B2 complete entries"]
fn a_bounded_layer_walk_costs_the_bound_plus_one_seek_regardless_of_descendant_only_siblings() {
    let (mut store, counters) = layered_store(&["k1", "k4"], &["k2", "k3"]);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), writing_demand())
        .expect("txn session");
    let root = txn.site(0);
    let before = counters.reads();
    let frozen = txn
        .iterate_bounded(&root, &[], None, bound(1))
        .expect("iterate");
    let seeks = counters.reads() - before;
    assert_eq!(frozen.keys, vec![KeyScalar::Str("k1".into())]);
    assert!(frozen.more, "k4 lies beyond the bound");
    assert_eq!(
        seeks, 2,
        "one seek per frozen key plus the one that flags `on more`, whatever sits between",
    );
}

/// A family presence probe over `[a1, a2, a3 descendant-only, a4 present]` answers
/// `Present` in exactly one seek. Today it seeks past each descendant-only sibling first
/// (four seeks), so `exists(^books)` costs the population, not the probe.
#[test]
#[ignore = "B2 complete entries"]
fn a_family_presence_probe_costs_one_seek_regardless_of_descendant_only_siblings() {
    let (mut store, counters) = layered_store(&["a4"], &["a1", "a2", "a3"]);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), writing_demand())
        .expect("txn session");
    let root = txn.site(0);
    let before = counters.reads();
    let populated = txn.family_populated(&root, &[]).expect("probe");
    let seeks = counters.reads() - before;
    assert_eq!(populated, Presence::Present);
    assert_eq!(
        seeks, 1,
        "a family probe is one seek into the presence namespace"
    );
}
