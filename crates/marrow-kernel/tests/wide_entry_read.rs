//! Whole-entry read engine work is proportional to the *populated* field count, not
//! the *declared* width (WR01 obligation ii).
//!
//! A sparse entry stores one cell per present field plus its marker, so materializing
//! the whole entry is a structural-tag-bounded range scan over the entry's own
//! contiguous field-leaf cells — the same cells, no format change. The scan visits
//! only present leaves, so its engine work (counting-engine reads) is flat across
//! declared widths at a fixed populated count. This measures it through the counting
//! engine: reading a whole entry with the same present fields stages the same number
//! of engine reads on a resource declaring 100 fields and on one declaring 2000.

mod common;

use common::{Counters, CountingEngine};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CommitResult, CreateOutcome, DemandCoverage, Durable, DurableStore, EntryValue, FieldSchema,
    InvocationGrant, SiteSpec, SiteTarget, StoreSchema,
};
use marrow_kernel::equality::ValueDomain;

/// A schema whose first field is a required `Int` and whose remaining `declared - 1`
/// fields are optional `Int`s — the declared width grows while the populated set a
/// caller reads stays the same.
fn schema(declared: usize) -> StoreSchema {
    let mut fields = vec![FieldSchema::scalar("value", ScalarKind::Int, true)];
    for i in 0..declared.saturating_sub(1) {
        fields.push(FieldSchema::scalar(format!("f{i}"), ScalarKind::Int, false));
    }
    StoreSchema {
        root_name: "wide".into(),
        key: vec![ScalarKind::Int],
        fields,
        branches: Vec::new(),
        groups: Vec::new(),
        indexes: Vec::new(),
    }
}

fn sites() -> Vec<SiteSpec> {
    vec![SiteSpec {
        target: SiteTarget::WholePayload,
    }]
}

fn read() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: false,
    }
}

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

/// One whole-entry read measured two ways at the "exact-field work vs declaration
/// width and present count" tier: `reads` is the deterministic counting-engine read
/// count (engine scan calls); `value_len` is the materialized value size (the length
/// of the dense schema-aligned `EntryValue.fields`).
struct Measured {
    reads: usize,
    value_len: usize,
}

/// A whole-entry read against a resource declaring `declared` fields whose entry has
/// the leading `populated` fields present.
fn measure_whole_entry(declared: usize, populated: usize) -> Measured {
    assert!(populated <= declared);
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(declared),
        sites(),
        write(),
    );

    // Stage the entry with exactly `populated` leading fields present.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn session");
        let entry = txn.site(0);
        let mut fields: Vec<Option<ValueDomain>> = (0..populated)
            .map(|i| Some(ValueDomain::Scalar(RuntimeScalar::Int(i as i64))))
            .collect();
        fields.extend(std::iter::repeat_n(None, declared - populated));
        assert_eq!(
            txn.create_entry(
                &entry,
                &[KeyScalar::Int(1)],
                EntryValue {
                    groups: Vec::new(),
                    fields,
                },
            )
            .expect("create"),
            CreateOutcome::Created,
        );
        assert_eq!(txn.commit(), CommitResult::Committed);
    }

    // Measure only the whole-entry read. The counting engine tallies reads through a
    // transaction view (`get`/`scan_after`), and `read_entry` runs the same
    // materialization owner in a read or a transaction session, so a fresh
    // transaction session measures the whole-entry read's engine work directly.
    let mut session = store
        .txn_session(InvocationGrant::full_store(), read())
        .expect("read txn session");
    let entry = session.site(0);
    let before = counters.reads();
    let value = session
        .read_entry(&entry, &[KeyScalar::Int(1)])
        .expect("read")
        .expect("entry present");
    let reads = counters.reads() - before;
    let present = value.fields.iter().filter(|slot| slot.is_some()).count();
    assert_eq!(
        present, populated,
        "the read materialized every present field"
    );
    Measured {
        reads,
        value_len: value.fields.len(),
    }
}

#[test]
fn whole_entry_read_engine_work_is_flat_across_declared_widths() {
    let narrow = measure_whole_entry(100, 20).reads;
    let wide = measure_whole_entry(2000, 20).reads;
    assert_eq!(
        narrow, wide,
        "declared width must not change the engine work of reading a fixed present set \
         (narrow={narrow}, wide={wide})",
    );
    // O(populated + 1): the whole-entry read stages at most one range-scan read per
    // present leaf plus the boundary read, far below the declared width — a
    // per-declared-field probe would stage one read per declared field (101 and 2001).
    assert!(
        narrow <= 20 + 1,
        "whole-entry read is O(populated + 1), got {narrow} reads for 20 present fields",
    );
}

/// The engine work tracks the *populated* count, not a constant: a denser entry (same
/// declared width) stages strictly more range-scan reads. This distinguishes an
/// O(populated) scan from one that ignores the data entirely.
#[test]
fn whole_entry_read_engine_work_grows_with_the_populated_count() {
    let sparse = measure_whole_entry(2000, 20).reads;
    let dense = measure_whole_entry(2000, 200).reads;
    assert!(
        dense > sparse,
        "more present fields must stage more range-scan reads (sparse={sparse}, dense={dense})",
    );
}

/// The materialized value size is O(declared): the whole-entry read yields a dense
/// schema-aligned `EntryValue.fields` with one slot per *declared* field, so its
/// length tracks the declared width and is independent of the present count. This is
/// the accepted, measured O(declared) value-size seam WR01 records and defers: the
/// engine work is already O(populated+1) (above), but the value shape stays dense.
/// The named seam a later lane can take to make value size O(populated) is sparse
/// sorted (field-index, value) slots (which the field-leaf scan already yields in
/// order) versus an Rc-COW record backing. This law fails if a future change alters
/// the value shape, so the seam is flipped deliberately, not by accident.
#[test]
fn whole_entry_read_value_size_is_the_declared_width() {
    // Value size tracks the declared width, not the present count.
    assert_eq!(measure_whole_entry(100, 20).value_len, 100);
    assert_eq!(measure_whole_entry(2000, 20).value_len, 2000);
    // Fixed declared width, different present counts: identical value size (O(declared),
    // independent of populated) — the complement of the O(populated) engine-work law.
    assert_eq!(
        measure_whole_entry(2000, 20).value_len,
        measure_whole_entry(2000, 200).value_len,
        "value size is set by the declared width, not the present count",
    );
}
