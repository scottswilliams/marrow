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

/// A complete `counters` entry, the entry a field set updates.
fn seed_entry() -> EntryValue {
    EntryValue {
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))],
        groups: Vec::new(),
    }
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
    let entry = txn.site(0);
    let field = txn.site(1);
    txn.create_entry(&entry, &[KeyScalar::Int(1)], seed_entry())
        .expect("create the entry the set updates");
    let writes_before = counters.writes();
    // A date beyond the year-9999 canonical bound cannot encode; the op must reject
    // it before any engine write.
    let rejected = txn.set_field(
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
    let entry = txn.site(0);
    let field = txn.site(1);
    txn.create_entry(&entry, &[KeyScalar::Int(1)], seed_entry())
        .expect("create the entry the set updates");
    let writes_before = counters.writes();
    txn.set_field(
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
// These witnesses specify the deferred presence-seek bounds. The current marker walk
// seeks past each descendant-only sibling, so acquisition costs O(limit + 1 + d)
// seeks and family presence costs O(1 + d), where d counts skipped descendant-only
// siblings. A bound on frozen keys does not establish the stronger seek bound.

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

/// The deferred presence-seek bound: `at most 1` over
/// `[k1 present, k2 and k3 descendant-only, k4 present]` freezes `k1` and flags
/// `more` in two seeks. The current walk also seeks past `k2` and `k3` (four seeks),
/// so the count grows with the descendant-only population the bound never names.
#[test]
#[ignore = "deferred presence-seek slice: acquisition still seeks past descendant-only siblings"]
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

/// The deferred presence-seek bound: a family presence probe over
/// `[a1, a2, a3 descendant-only, a4 present]` answers `Present` in one seek. The current
/// walk seeks past each descendant-only sibling first (four seeks).
#[test]
#[ignore = "deferred presence-seek slice: family presence still seeks past descendant-only siblings"]
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

// --- The required-completeness class: an incomplete payload never reaches the engine. ---

/// A `books` root with a required `title` and a `details` group whose `pages` leaf is
/// required. Site 0 is the entry, site 1 the group.
fn grouped_schema() -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Int]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_group("details");
    builder.scalar_field("pages", ScalarKind::Int, true);
    builder.close_group();
    builder.finish().expect("the grouped schema builds")
}

fn grouped_sites() -> Vec<SiteTarget> {
    vec![SiteTarget::whole_payload(), SiteTarget::group_entry(0)]
}

fn pages(value: Option<i64>) -> EntryValue {
    EntryValue {
        fields: vec![value.map(|n| ValueDomain::Scalar(RuntimeScalar::Int(n)))],
        groups: Vec::new(),
    }
}

fn book(title: Option<&str>, group: EntryValue) -> EntryValue {
    EntryValue {
        fields: vec![title.map(|t| ValueDomain::Scalar(RuntimeScalar::Str(t.into())))],
        groups: vec![group],
    }
}

/// The typed fault of an incomplete write, reported as `run.corruption`.
fn assert_incomplete<T: std::fmt::Debug>(result: Result<T, KernelFault>, what: &str) {
    match result {
        Err(KernelFault::Incomplete) => {}
        other => panic!("{what}: expected KernelFault::Incomplete, got {other:?}"),
    }
}

/// Entry and group writes check exact record widths, required fields, group count,
/// and the absence of nested groups before engine access. Both create and replace
/// refuse malformed values as `KernelFault::Incomplete`, including create over a
/// present slot, with no reads or writes.
#[test]
fn a_write_missing_a_required_field_is_a_typed_kernel_fault_before_any_engine_write() {
    let counters = Counters::new();
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(&grouped_schema(), grouped_sites()),
        writing_demand(),
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), writing_demand())
        .expect("txn session");
    let entry = txn.site(0);
    let group = txn.site(1);
    let key = [KeyScalar::Int(1)];

    txn.create_entry(&entry, &key, book(Some("Small Gods"), pages(Some(381))))
        .expect("a complete entry writes");
    let staged = counters.writes();
    let probed = counters.reads();

    assert_incomplete(
        txn.create_entry(&entry, &[KeyScalar::Int(2)], book(None, pages(Some(1)))),
        "an entry write missing its required `title`",
    );
    assert_incomplete(
        txn.create_entry(&entry, &[KeyScalar::Int(3)], book(Some("t"), pages(None))),
        "an entry write whose group misses the required `pages`",
    );
    assert_incomplete(
        txn.create_entry(
            &entry,
            &[KeyScalar::Int(4)],
            EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("t".into())))],
                groups: Vec::new(),
            },
        ),
        "an entry write whose `groups` vector is shorter than the schema",
    );
    assert_incomplete(
        txn.create_entry(&entry, &key, book(None, pages(Some(1)))),
        "an incomplete create over a present slot is refused before the slot probe",
    );
    assert_incomplete(
        txn.replace_entry(&entry, &key, book(None, pages(Some(1)))),
        "a replace of a present entry missing its required `title`",
    );
    assert_incomplete(
        txn.replace_group(&group, &key, pages(None)),
        "a group write missing the required `pages`",
    );

    let mut short_record = book(Some("t"), pages(Some(1)));
    short_record.fields.clear();
    let mut long_record = book(Some("t"), pages(Some(1)));
    long_record.fields.push(None);
    let mut extra_groups = book(Some("t"), pages(Some(1)));
    extra_groups.groups.push(pages(Some(2)));
    for (value, what) in [
        (short_record, "a short top-level record"),
        (long_record, "a long top-level record"),
        (extra_groups, "more groups than the schema declares"),
    ] {
        assert_incomplete(
            txn.create_entry(&entry, &[KeyScalar::Int(5)], value.clone()),
            what,
        );
        assert_incomplete(txn.replace_entry(&entry, &key, value), what);
    }

    let mut short_group = pages(Some(1));
    short_group.fields.clear();
    let mut long_group = pages(Some(1));
    long_group.fields.push(None);
    let mut nested_group = pages(Some(1));
    nested_group.groups.push(pages(Some(2)));
    for (value, what) in [
        (short_group, "a short group record"),
        (long_group, "a long group record"),
        (nested_group, "a group containing a nested group"),
    ] {
        let whole = book(Some("t"), value.clone());
        assert_incomplete(
            txn.create_entry(&entry, &[KeyScalar::Int(5)], whole.clone()),
            what,
        );
        assert_incomplete(txn.replace_entry(&entry, &key, whole), what);
        assert_incomplete(txn.replace_group(&group, &key, value), what);
    }
    assert_eq!(
        counters.writes(),
        staged,
        "every refused write stages zero engine writes",
    );
    assert_eq!(
        counters.reads(),
        probed,
        "the completeness check precedes the slot probe, so a refused write reads nothing",
    );
}

/// Erasing what a present entry must hold is refused the same way: `erase_field` on a
/// required field site and `erase_group` on a group with a required leaf return
/// `KernelFault::Incomplete` with zero engine reads or writes.
#[test]
fn an_erase_of_a_required_field_or_group_is_a_typed_kernel_fault() {
    let counters = Counters::new();
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        project(
            &grouped_schema(),
            vec![
                SiteTarget::whole_payload(),
                SiteTarget::group_entry(0),
                SiteTarget::field_leaf(0),
            ],
        ),
        writing_demand(),
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), writing_demand())
        .expect("txn session");
    let entry = txn.site(0);
    let group = txn.site(1);
    let title = txn.site(2);
    let key = [KeyScalar::Int(1)];
    txn.create_entry(&entry, &key, book(Some("Small Gods"), pages(Some(381))))
        .expect("a complete entry writes");
    let staged = counters.writes();
    let probed = counters.reads();

    assert_incomplete(
        txn.erase_field(&title, &key),
        "an erase of the required `title`",
    );
    assert_incomplete(
        txn.erase_group(&group, &key),
        "an erase of the group holding the required `pages`",
    );
    assert_eq!(
        counters.writes(),
        staged,
        "every refused erase stages zero engine writes",
    );
    assert_eq!(
        counters.reads(),
        probed,
        "every refused erase reads nothing",
    );
}
