//! A managed-index scan's engine work is O(distinct + 1) seeks, independent of fan-out.
//!
//! A nonunique index scan enumerates the *distinct* values of the next projected
//! component. Rows sharing one component value are passed by a single prefix-successor
//! seek — the index traversal-skip law — so the engine work is one seek per distinct
//! value plus one boundary probe, regardless of how many rows share each value. This
//! measures it through the counting engine: scanning the same distinct labels costs the
//! same seeks whether each label carries two rows or fifty, a bounded `at most N` scan
//! costs `N + 1` seeks, and a unique lookup is one probe.

mod common;

use common::{Counters, CountingEngine};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    BoundedLimit, DemandCoverage, Durable, DurableStore, EntryValue, FieldSchema, IndexComponent,
    IndexSchema, InvocationGrant, SiteSpec, SiteTarget, StoreSchema,
};
use marrow_kernel::equality::ValueDomain;

const BY_SHELF: [u8; 16] = [0xA0; 16];
const BY_ISBN: [u8; 16] = [0xB0; 16];

/// A keyed `books` root with a nonunique `byShelf[shelf, id]` index and a unique
/// `byIsbn[isbn]` index.
fn schema() -> StoreSchema {
    StoreSchema {
        root_name: "books".into(),
        key: vec![ScalarKind::Int],
        fields: vec![
            FieldSchema::scalar("shelf", ScalarKind::Str, true),
            FieldSchema::scalar("isbn", ScalarKind::Str, true),
        ],
        branches: Vec::new(),
        groups: Vec::new(),
        indexes: vec![
            IndexSchema {
                id: BY_SHELF,
                unique: false,
                projection: vec![IndexComponent::Field(0), IndexComponent::Key(0)],
            },
            IndexSchema {
                id: BY_ISBN,
                unique: true,
                projection: vec![IndexComponent::Field(1)],
            },
        ],
    }
}

/// Site 0 the entry, site 1 the `byShelf` scan, site 2 the `byIsbn` lookup.
fn sites() -> Vec<SiteSpec> {
    vec![
        SiteSpec {
            target: SiteTarget::WholePayload,
        },
        SiteSpec {
            target: SiteTarget::IndexScan(0),
        },
        SiteSpec {
            target: SiteTarget::IndexLookup(1),
        },
    ]
}

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

fn bound(n: u32) -> BoundedLimit {
    BoundedLimit::new(n).expect("positive bound")
}

fn entry(shelf: &str, isbn: &str) -> EntryValue {
    EntryValue {
        fields: vec![
            Some(ValueDomain::Scalar(RuntimeScalar::Str(shelf.into()))),
            Some(ValueDomain::Scalar(RuntimeScalar::Str(isbn.into()))),
        ],
    }
}

/// A store seeded with `distinct` shelves, each carrying `fanout` books with distinct
/// ids and isbns, over the counting engine.
fn seeded(distinct: usize, fanout: usize) -> (DurableStore<CountingEngine>, Counters) {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(),
        sites(),
        write(),
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let entry_site = txn.site(0);
    let mut id: i64 = 0;
    for shelf in 0..distinct {
        for _ in 0..fanout {
            txn.create_entry(
                &entry_site,
                &[KeyScalar::Int(id)],
                entry(&format!("s{shelf:03}"), &format!("i{id:05}")),
            )
            .expect("create");
            id += 1;
        }
    }
    assert_eq!(
        txn.commit(),
        marrow_kernel::durable::CommitResult::Committed
    );
    (store, counters)
}

/// The engine seeks a from-less scan of every distinct shelf performs.
fn scan_all_shelves_seeks(distinct: usize, fanout: usize) -> usize {
    let (mut store, counters) = seeded(distinct, fanout);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let scan = txn.site(1);
    let before = counters.reads();
    let result = txn.index_scan(&scan, &[], None, bound(1000)).expect("scan");
    let seeks = counters.reads() - before;
    assert_eq!(result.keys.len(), distinct, "one key per distinct shelf");
    assert!(!result.more, "no shelf beyond the population");
    seeks
}

#[test]
fn a_scan_costs_one_seek_per_distinct_value_independent_of_fan_out() {
    // Five distinct shelves, whether each holds two books or fifty: the same distinct
    // count plus one boundary probe, never scaling with the rows behind each value.
    let narrow = scan_all_shelves_seeks(5, 2);
    let wide = scan_all_shelves_seeks(5, 50);
    assert_eq!(narrow, 6, "five distinct values plus one boundary probe");
    assert_eq!(
        narrow, wide,
        "row fan-out behind each value must not change the scan's engine work",
    );
}

#[test]
fn a_bounded_scan_costs_exactly_the_bound_plus_the_boundary_probe() {
    let (mut store, counters) = seeded(10, 3);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let scan = txn.site(1);
    let before = counters.reads();
    let result = txn.index_scan(&scan, &[], None, bound(3)).expect("scan");
    let seeks = counters.reads() - before;
    assert_eq!(result.keys.len(), 3, "the bound freezes three values");
    assert!(result.more, "further values remain");
    assert_eq!(
        seeks, 4,
        "three frozen values plus the one that flags `on more`"
    );
}

#[test]
fn a_unique_lookup_is_a_single_probe() {
    let (mut store, counters) = seeded(4, 1);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let lookup = txn.site(2);
    let before = counters.reads();
    // The book with id 2 was seeded with isbn "i00002".
    let hit = txn
        .index_lookup(&lookup, &[KeyScalar::Str("i00002".into())])
        .expect("lookup");
    let seeks = counters.reads() - before;
    assert_eq!(
        hit,
        Some(vec![KeyScalar::Int(2)]),
        "the one matching source key"
    );
    assert_eq!(seeks, 1, "a unique lookup is one exact probe");
    let miss = txn
        .index_lookup(&lookup, &[KeyScalar::Str("absent".into())])
        .expect("lookup");
    assert_eq!(miss, None, "no row matches an absent isbn");
}
