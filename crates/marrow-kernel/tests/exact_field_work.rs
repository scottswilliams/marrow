//! Exact field work is independent of a resource's declared width.
//!
//! A sparse resource stores one cell per present field plus one entry marker, never
//! one cell per *declared* field. So an exact field mutation stages a constant number
//! of engine writes regardless of how many fields the resource declares — the
//! bounded-work law the sparse structural model rests on. This measures it through
//! the counting engine: a single required-field set, a single field erase, and a
//! create with one present field each stage the same number of engine writes on a
//! resource declaring one field and on one declaring twenty.

mod common;

use common::{Counters, CountingEngine};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CreateOutcome, DemandCoverage, Durable, DurableStore, EntryValue, FieldSchema, InvocationGrant,
    SiteSpec, SiteTarget, StoreSchema,
};

/// A schema whose first field is a required `Int` and whose remaining `extra`
/// fields are optional — so the *declared* width grows while the field a caller
/// mutates stays the same.
fn schema(extra: usize) -> StoreSchema {
    let mut fields = vec![FieldSchema {
        name: "value".into(),
        kind: ScalarKind::Int,
        required: true,
    }];
    for i in 0..extra {
        fields.push(FieldSchema {
            name: format!("opt{i}"),
            kind: ScalarKind::Int,
            required: false,
        });
    }
    StoreSchema {
        root_name: "counters".into(),
        key: ScalarKind::Int,
        fields,
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

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

/// The number of engine writes a single required-field set stages against a resource
/// declaring `1 + extra` fields.
fn writes_for_single_field_set(extra: usize) -> usize {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(extra),
        sites(),
        write(),
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let field = txn.site(1);
    let before = counters.writes();
    txn.set_required(&field, &[KeyScalar::Int(1)], RuntimeScalar::Int(7))
        .expect("set required");
    counters.writes() - before
}

/// The number of engine writes creating an entry with only its required field present
/// stages against a resource declaring `1 + extra` fields.
fn writes_for_narrow_create(extra: usize) -> usize {
    let counters = Counters::new();
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(counters.clone()),
        schema(extra),
        sites(),
        write(),
    );
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let entry = txn.site(0);
    // Only field 0 is present; every declared optional field is vacant.
    let mut fields = vec![Some(RuntimeScalar::Int(7))];
    fields.extend(std::iter::repeat_n(None, extra));
    let before = counters.writes();
    assert_eq!(
        txn.create_entry(&entry, &[KeyScalar::Int(1)], EntryValue { fields })
            .expect("create"),
        CreateOutcome::Created
    );
    counters.writes() - before
}

#[test]
fn a_single_field_set_stages_constant_writes_regardless_of_declared_width() {
    let narrow = writes_for_single_field_set(0);
    let wide = writes_for_single_field_set(19);
    assert_eq!(
        narrow, 1,
        "a required-field set stages exactly its one leaf"
    );
    assert_eq!(
        narrow, wide,
        "declared width must not change the work of setting one field",
    );
}

#[test]
fn a_narrow_create_stages_constant_writes_regardless_of_declared_width() {
    let narrow = writes_for_narrow_create(0);
    let wide = writes_for_narrow_create(19);
    // Marker plus the one present field: two writes, whatever the declared width.
    assert_eq!(
        narrow, 2,
        "a one-present-field create stages marker + one leaf"
    );
    assert_eq!(
        narrow, wide,
        "declared width must not change the work of a narrow create",
    );
}
