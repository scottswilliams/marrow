//! The logical audit's engine-call counts and largest returned scan page.
//!
//! The walk point-reads the empty key once, pages through the remaining key space, and
//! point-reads each index cell's source and each indexed entry's index cell. The counting engine
//! compares those calls across declared widths and entry populations. The page witness
//! measures the largest returned batch; it does not measure retained pages, peak
//! allocation, or native engine-cache residency.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::{Counters, CountingEngine};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CommitResult, ContentDigest, CreateOutcome, DemandCoverage, Durable, DurableStore, EntryValue,
    IndexComponent, InvocationGrant, SiteTarget, StoreProjection, StoreSchema, StoreSchemaBuilder,
};
use marrow_kernel::equality::ValueDomain;
use marrow_store::{ByteEngine, Cell as StoreCell, MemoryEngine, ReadView, StoreError};

/// `^wide[id: int]` with a required `value`, `declared - 1` sparse `Int` fields, and a
/// unique index on `value`.
fn schema(declared: usize) -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("wide", vec![ScalarKind::Int]);
    builder.scalar_field("value", ScalarKind::Int, true);
    for i in 0..declared.saturating_sub(1) {
        builder.scalar_field(format!("f{i}"), ScalarKind::Int, false);
    }
    builder.index([0x5A; 16], true, vec![IndexComponent::field(0)]);
    builder.finish().expect("a bounded schema builds")
}

fn projection(declared: usize) -> StoreProjection {
    let mut projection = StoreProjection::builder();
    projection.root(schema(declared));
    projection.site(0, SiteTarget::whole_payload());
    projection.finish().expect("the site names the root")
}

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

struct Discard;

impl ContentDigest for Discard {
    fn absorb(&mut self, _key: &[u8], _value: &[u8]) {}
}

/// Populate `entries` entries, each with only its required field, over a resource
/// declaring `declared` fields.
fn populate<E: ByteEngine>(store: &mut DurableStore<E>, entries: i64, declared: usize) {
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn");
    let site = txn.site(0);
    for id in 0..entries {
        let mut fields = vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(id)))];
        fields.extend(std::iter::repeat_n(None, declared - 1));
        assert_eq!(
            txn.create_entry(
                &site,
                &[KeyScalar::Int(id)],
                EntryValue {
                    fields,
                    groups: Vec::new(),
                },
            )
            .expect("create"),
            CreateOutcome::Created
        );
    }
    assert!(matches!(txn.commit(), CommitResult::Committed));
}

/// The engine reads (scans and point reads) one audit of `entries` entries costs on a
/// resource declaring `declared` fields.
fn reads_for_audit(entries: i64, declared: usize) -> usize {
    let counters = Counters::new();
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::new(counters.clone()),
        projection(declared),
        write(),
    );
    populate(&mut store, entries, declared);
    let before = counters.reads();
    let report = store.logical_audit(&mut Discard).expect("audit");
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(report.summary.entries, entries as u64);
    counters.reads() - before
}

#[test]
fn audit_reads_are_flat_across_declared_width() {
    let narrow = reads_for_audit(50, 2);
    let wide = reads_for_audit(50, 500);
    assert_eq!(
        narrow, wide,
        "the walk's engine reads depend on populated cells, not declared fields",
    );
}

#[test]
fn audit_reads_are_linear_in_populated_cells() {
    // Each entry is 2 entry cells (marker, value) and 1 index cell; the walk pays one
    // scan per 64-cell page (plus the final empty page), one point read per index cell
    // (the source marker) and one per projected field of it, one per indexed entry,
    // and one initial point read for the empty key.
    let small = reads_for_audit(64, 2);
    let large = reads_for_audit(640, 2);
    assert!(
        large <= 10 * small + 16,
        "reads grow at most linearly with entries: {small} at 64, {large} at 640",
    );
    assert!(
        large > small,
        "more cells cost more reads: {small} < {large}"
    );
}

/// A view that reports the largest page the walk ever received.
struct PageWitness<'a> {
    inner: <MemoryEngine as ByteEngine>::View<'a>,
    largest: Rc<Cell<usize>>,
}

impl ReadView for PageWitness<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        let page = self.inner.scan_after(prefix, cursor)?;
        self.largest.set(self.largest.get().max(page.len()));
        Ok(page)
    }
}

/// An engine whose every read view is a page witness over the memory engine.
struct WitnessEngine {
    inner: MemoryEngine,
    largest: Rc<Cell<usize>>,
}

impl ByteEngine for WitnessEngine {
    type View<'a> = PageWitness<'a>;
    type Txn<'a> = <MemoryEngine as ByteEngine>::Txn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        Ok(PageWitness {
            inner: self.inner.read_view()?,
            largest: Rc::clone(&self.largest),
        })
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        self.inner.begin()
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.inner.require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.inner.audit_integrity()
    }
}

/// On 10,000 entries (20,000 entry cells and 10,000 index cells), the largest page
/// returned to the walk respects the engine's record bound. This witness observes one
/// return at a time and makes no assertion about peak allocation or retained pages.
#[test]
fn the_largest_returned_audit_page_respects_the_engine_record_bound() {
    let largest = Rc::new(Cell::new(0));
    let mut store = DurableStore::from_projection_with_ceiling(
        WitnessEngine {
            inner: MemoryEngine::new(),
            largest: Rc::clone(&largest),
        },
        projection(2),
        write(),
    );
    populate(&mut store, 10_000, 2);
    let report = store.logical_audit(&mut Discard).expect("audit");
    assert!(report.is_clean());
    assert_eq!(report.summary.entries, 10_000);
    assert_eq!(report.summary.index_cells, 10_000);
    assert!(
        largest.get() <= 64,
        "a page holds at most the engine's scan batch, saw {}",
        largest.get()
    );
}
