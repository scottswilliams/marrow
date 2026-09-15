//! Field tokens: resolution, authority, and the engine calls a token costs.

use super::super::super::{AuthTarget, AuthorizedSite, ResolvedField};
use super::engine_call_support::{Counters, CountingEngine};
use super::*;
use crate::codec::value::{ValueShape, ValueShapeBuilder, ValueShapeRef};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
enum SessionKind {
    Read,
    Transaction,
}

fn nested_shape() -> ValueShape {
    let mut shape = ValueShapeBuilder::new();
    shape
        .open_product(7)
        .scalar(ScalarKind::Int)
        .open_sum(9)
        .open_variant()
        .close()
        .open_variant()
        .open_product(8)
        .scalar(ScalarKind::Str)
        .scalar(ScalarKind::Bool)
        .close()
        .close()
        .close()
        .close();
    shape.finish().expect("bounded nested field shape")
}

fn schema_with_width(width: usize) -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("items", vec![ScalarKind::Str]);
    for branch in [false, true] {
        if branch {
            builder.open_branch("children", vec![ScalarKind::Int]);
        }
        builder.scalar_field("scalar", ScalarKind::Int, true);
        builder.field("nested", nested_shape(), true);
        for field in 2..width {
            let shape = if field % 2 == 0 {
                ValueShape::scalar(ScalarKind::Int)
            } else {
                nested_shape()
            };
            builder.field(format!("spare{field}"), shape, false);
        }
        if branch {
            builder.close_branch();
        }
    }
    builder.finish().expect("root and branch schema")
}

fn scalar(value: i64) -> ValueDomain {
    ValueDomain::Scalar(RuntimeScalar::Int(value))
}

fn nested(value: i64, label: &str) -> ValueDomain {
    ValueDomain::Product {
        ty: 7,
        fields: vec![
            Some(scalar(value)),
            Some(ValueDomain::Sum {
                ty: 9,
                variant: 1,
                payload: vec![ValueDomain::Product {
                    ty: 8,
                    fields: vec![
                        Some(ValueDomain::Scalar(RuntimeScalar::Str(label.into()))),
                        Some(ValueDomain::Scalar(RuntimeScalar::Bool(true))),
                    ],
                }],
            }),
        ],
    }
}

fn entry(width: usize, value: i64, label: &str) -> EntryValue {
    let mut fields = vec![None; width];
    fields[0] = Some(scalar(value));
    fields[1] = Some(nested(value, label));
    EntryValue {
        fields,
        groups: Vec::new(),
    }
}

fn field_parts(
    site: &AuthorizedSite,
) -> (physical::NodeNumber, bool, &ValueShape, &[ResolvedField]) {
    let AuthTarget::Field { payload } = &site.target else {
        panic!("the fixture selects a field site")
    };
    (
        payload.number,
        payload.required,
        &payload.shape,
        &payload.record,
    )
}

fn shape_units(shape: &ValueShape) -> usize {
    match shape.view() {
        ValueShapeRef::Scalar(_) => 1,
        ValueShapeRef::Product { fields, .. } => 1 + fields.iter().map(shape_units).sum::<usize>(),
        ValueShapeRef::Sum { variants, .. } => {
            1 + variants.len()
                + (0..variants.len())
                    .map(|variant| {
                        variants
                            .get(variant)
                            .expect("declared variant")
                            .iter()
                            .map(shape_units)
                            .sum::<usize>()
                    })
                    .sum::<usize>()
        }
    }
}

fn check_session(
    session: &mut dyn Durable,
    counters: &Counters,
    kind: SessionKind,
    width: usize,
    mismatches: &mut Vec<(SessionKind, usize, u16, usize, usize)>,
) {
    let root_keys = [KeyScalar::Str("parent".into())];
    let child_keys = [KeyScalar::Str("parent".into()), KeyScalar::Int(11)];
    for (site_id, keys, expected) in [
        (1, root_keys.as_slice(), scalar(21)),
        (2, root_keys.as_slice(), nested(21, "root")),
        (4, child_keys.as_slice(), scalar(42)),
        (5, child_keys.as_slice(), nested(42, "child")),
    ] {
        let before = (counters.gets(), counters.scans(), counters.writes());
        let tokens: [AuthorizedSite; 4] = std::array::from_fn(|_| session.site(site_id));
        assert_eq!(
            (counters.gets(), counters.scans(), counters.writes()),
            before,
            "copying a prepared field token performs no engine work",
        );
        let first = &tokens[0];
        let (number, required, shape, record) = field_parts(first);
        assert!(required);
        assert_eq!(record.len(), width);
        assert_eq!(first.key_arity(), keys.len());
        let record_owners = tokens
            .iter()
            .map(|token| field_parts(token).3.as_ptr())
            .collect::<BTreeSet<_>>()
            .len();
        let shape_owners = tokens
            .iter()
            .map(|token| std::ptr::from_ref(field_parts(token).2))
            .collect::<BTreeSet<_>>()
            .len();
        let units = record.len()
            + record
                .iter()
                .map(|field| shape_units(&field.shape))
                .sum::<usize>();
        eprintln!(
            "field token work: session={kind:?} width={width} site={site_id} \
             tokens={} record_shape_units={units} selected_shape_units={} \
             record_owners={record_owners} selected_shape_owners={shape_owners}",
            tokens.len(),
            shape_units(shape),
        );
        if record_owners != 1 || shape_owners != 1 {
            mismatches.push((kind, width, site_id, record_owners, shape_owners));
        }
        for token in &tokens {
            assert_eq!(field_parts(token), (number, required, shape, record));
            assert_eq!(token.root_number, first.root_number);
            assert_eq!(token.root_index, first.root_index);
            assert_eq!(token.key, first.key);
            assert_eq!(token.branch.len(), first.branch.len());
            for (actual, expected) in token.branch.iter().zip(&first.branch) {
                assert_eq!(actual.number, expected.number);
                assert_eq!(actual.key, expected.key);
            }
            let before = (
                counters.gets(),
                counters.scans(),
                counters.writes(),
                counters.reads(),
            );
            assert_eq!(session.read_field(token, keys), Ok(Some(expected.clone())));
            assert_eq!(
                (
                    counters.gets() - before.0,
                    counters.scans() - before.1,
                    counters.writes() - before.2,
                    counters.reads() - before.3,
                ),
                (1, 0, 0, 1),
                "one field read performs one get and no scans or writes",
            );
        }
    }
}

#[test]
fn field_site_tokens_share_schema_owners_and_read_one_cell() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<AuthorizedSite>();

    let mut mismatches = Vec::new();
    for width in [2, 65, 257] {
        let schema = schema_with_width(width);
        let sites = vec![
            SiteTarget::whole_payload(),
            SiteTarget::field_leaf(0),
            SiteTarget::field_leaf(1),
            branch_entry(&[0]),
            branch_field(&[0], 0),
            branch_field(&[0], 1),
        ];
        let counters = Counters::new();
        let mut store = DurableStore::from_engine(
            CountingEngine::new(counters.clone()),
            project(&schema, sites),
        );
        let root_keys = [KeyScalar::Str("parent".into())];
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("seed transaction");
            txn.create_entry(&txn.site(0), &root_keys, entry(width, 21, "root"))
                .expect("complete root");
            txn.create_entry(
                &txn.site(3),
                &[KeyScalar::Str("parent".into()), KeyScalar::Int(11)],
                entry(width, 42, "child"),
            )
            .expect("complete child");
            assert!(matches!(txn.commit(), CommitResult::Committed));
        }
        assert!(counters.opens() > 0);
        assert!(counters.writes() > 0);
        {
            let mut read = store
                .read_session(InvocationGrant::full_store(), read_demand())
                .expect("read session");
            let scans = counters.scans();
            assert_eq!(
                read.read_entry(&read.site(0), &root_keys),
                Ok(Some(entry(width, 21, "root"))),
            );
            assert!(
                counters.scans() > scans,
                "the scan counter observes a real scan"
            );
            check_session(
                &mut read,
                &counters,
                SessionKind::Read,
                width,
                &mut mismatches,
            );
        }
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("transaction session");
            check_session(
                &mut txn,
                &counters,
                SessionKind::Transaction,
                width,
                &mut mismatches,
            );
        }
    }
    assert!(
        mismatches.is_empty(),
        "field token schema owners were copied: {mismatches:?}",
    );
}
