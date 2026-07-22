//! Whole-entry read engine work is proportional to the *populated* field count, not
//! the *declared* width.
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
    GroupSchema, InvocationGrant, SiteSpec, SiteTarget, StoreSchema,
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
        root: 0,
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
    let mut store = DurableStore::from_schemas_with_ceiling(
        CountingEngine::new(counters.clone()),
        vec![schema(declared)],
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
        assert!(matches!(txn.commit(), CommitResult::Committed));
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
    // Exact page math (SCAN_MAX_RECORDS = 64): probe(1) + one scan call for the 20
    // present leaves (< one page) + one boundary call = 3, independent of declared
    // width. A per-declared-field probe would stage one read per declared field (101
    // and 2001). The counted unit is engine scan calls, O(populated / page + 1) — one
    // scan call per SCAN_MAX_RECORDS page plus a boundary call, not one call per leaf.
    assert_eq!(
        narrow, 3,
        "whole-entry read is a probe plus one page scan plus a boundary call",
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
/// the accepted, measured O(declared) value-size seam recorded and deferred: the
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

// --- Group-bearing whole-entry read ---------------------------------------------

/// A schema with a wide top-level plus two sibling unkeyed groups `a` and `b`, each
/// declaring `group_declared` optional `Int` fields. A whole-entry read materializes
/// the top-level record and every group, each through its own field-leaf range scan.
fn group_schema(top_declared: usize, group_declared: usize) -> StoreSchema {
    let group_fields = |prefix: &str| {
        (0..group_declared)
            .map(|i| FieldSchema::scalar(format!("{prefix}{i}"), ScalarKind::Int, false))
            .collect()
    };
    StoreSchema {
        root_name: "wide".into(),
        key: vec![ScalarKind::Int],
        fields: (0..top_declared)
            .map(|i| FieldSchema::scalar(format!("t{i}"), ScalarKind::Int, false))
            .collect(),
        branches: Vec::new(),
        groups: vec![
            GroupSchema {
                name: "a".into(),
                fields: group_fields("af"),
            },
            GroupSchema {
                name: "b".into(),
                fields: group_fields("bf"),
            },
        ],
        indexes: Vec::new(),
    }
}

/// A dense schema-aligned column of `declared` slots with the leading `present` slots
/// set to `base + i`, so each group's present values occupy a disjoint numeric range.
fn present_column(declared: usize, present: usize, base: i64) -> Vec<Option<ValueDomain>> {
    (0..declared)
        .map(|i| (i < present).then(|| ValueDomain::Scalar(RuntimeScalar::Int(base + i as i64))))
        .collect()
}

/// The present values of a materialized column, in slot order.
fn present_values(fields: &[Option<ValueDomain>]) -> Vec<i64> {
    fields
        .iter()
        .filter_map(|slot| match slot {
            Some(ValueDomain::Scalar(RuntimeScalar::Int(v))) => Some(*v),
            _ => None,
        })
        .collect()
}

struct GroupMeasured {
    reads: usize,
    entry: EntryValue,
}

/// Create a group-bearing entry (top-level `1000+`, group `a` `10000+`, group `b`
/// `20000+`), commit it, then read it back through BOTH a read session (value
/// correctness) and a transaction session (engine-read count). The two sessions must
/// materialize an identical value; the transaction read supplies the counted work.
fn measure_group_entry(
    top_declared: usize,
    group_declared: usize,
    top_present: usize,
    a_present: usize,
    b_present: usize,
) -> GroupMeasured {
    let counters = Counters::new();
    let mut store = DurableStore::from_schemas_with_ceiling(
        CountingEngine::new(counters.clone()),
        vec![group_schema(top_declared, group_declared)],
        sites(),
        write(),
    );
    let entry_value = || EntryValue {
        fields: present_column(top_declared, top_present, 1000),
        groups: vec![
            EntryValue {
                fields: present_column(group_declared, a_present, 10000),
                groups: Vec::new(),
            },
            EntryValue {
                fields: present_column(group_declared, b_present, 20000),
                groups: Vec::new(),
            },
        ],
    };
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn session");
        let entry = txn.site(0);
        assert_eq!(
            txn.create_entry(&entry, &[KeyScalar::Int(1)], entry_value())
                .expect("create"),
            CreateOutcome::Created,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    // Read session: value correctness (its engine reads are uncounted).
    let read_value = {
        let mut session = store
            .read_session(InvocationGrant::full_store(), read())
            .expect("read session");
        let entry = session.site(0);
        session
            .read_entry(&entry, &[KeyScalar::Int(1)])
            .expect("read")
            .expect("entry present")
    };

    // Transaction session: the counted whole-entry read.
    let mut session = store
        .txn_session(InvocationGrant::full_store(), read())
        .expect("read txn session");
    let entry = session.site(0);
    let before = counters.reads();
    let txn_value = session
        .read_entry(&entry, &[KeyScalar::Int(1)])
        .expect("read")
        .expect("entry present");
    let reads = counters.reads() - before;

    assert_eq!(
        read_value.fields, txn_value.fields,
        "the read and transaction sessions materialize the same top-level record",
    );
    assert_eq!(
        read_value.groups.len(),
        txn_value.groups.len(),
        "both sessions materialize the same group count",
    );
    for (r, t) in read_value.groups.iter().zip(&txn_value.groups) {
        assert_eq!(
            r.fields, t.fields,
            "the read and transaction sessions materialize the same group record",
        );
    }
    GroupMeasured {
        reads,
        entry: txn_value,
    }
}

/// A whole-entry read of a group-bearing sparse entry keeps every group's leaves in
/// its own group (no sibling bleed), materializes a group whose present count crosses
/// the scan page size (paging), and stages engine work flat across declared widths at
/// a fixed present set — the group loop in `op_read_entry` obeys the same
/// O(populated/page + 1) law as the top-level record.
#[test]
fn group_bearing_whole_entry_read_is_bounded_and_isolated() {
    // group `a`: 100 present (crosses the 64-record page size); group `b`: 20 present.
    let narrow = measure_group_entry(100, 300, 20, 100, 20);
    let wide = measure_group_entry(3000, 3000, 20, 100, 20);

    // Sibling-group isolation: each group materializes only its own values, in its own
    // disjoint numeric range — no value bleeds from `a` (10000+) into `b` (20000+).
    let group_a = present_values(&narrow.entry.groups[0].fields);
    let group_b = present_values(&narrow.entry.groups[1].fields);
    assert_eq!(
        group_a,
        (10000..10100).collect::<Vec<_>>(),
        "group a is intact"
    );
    assert_eq!(
        group_b,
        (20000..20020).collect::<Vec<_>>(),
        "group b is intact"
    );
    assert!(
        group_a.iter().all(|v| (10000..20000).contains(v)),
        "no group-b value bled into group a",
    );

    // Paging: group `a` materialized all 100 present leaves even though the count
    // crosses the SCAN_MAX_RECORDS page size, so the range scan pages correctly.
    assert_eq!(
        group_a.len(),
        100,
        "group a materialized every present leaf across the page boundary",
    );

    // Flat across declared widths: the same present set stages the same engine work on
    // a 300-declared and a 3000-declared group (and top-level).
    assert_eq!(
        narrow.reads, wide.reads,
        "group-bearing whole-entry read work is flat across declared widths \
         (narrow={}, wide={})",
        narrow.reads, wide.reads,
    );

    // Counted engine calls, by page math (SCAN_MAX_RECORDS = 64, one boundary call per
    // scanned range): probe(1) + top-level 20-present(2) + group a 100-present(3) +
    // group b 20-present(2) = 8.
    assert_eq!(
        narrow.reads, 8,
        "one scan call per SCAN_MAX_RECORDS page plus a boundary call, per scanned node",
    );
}
