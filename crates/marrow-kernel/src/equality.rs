//! The value-equality and collection-key-order owner.
//!
//! Marrow's value domain is closed. Two relations over it are load-bearing across
//! the language: **value equality** (the `==`/`!=` relation and the identity of a
//! stored value) and **collection key order** (the total order traversal and keyed
//! collections observe). This module is the single owner of both relations over the
//! current domain, reusing the scalar equality of [`RuntimeScalar`] and the
//! order-preserving key order of [`KeyScalar`] rather than restating either.
//!
//! **Value equality** covers the aggregate value domain: `unit`, the admitted
//! scalars, dense/sparse products (records and structs), closed sums (user `enum`s
//! and the `Option`/`Result` instantiations), and the finite collections (`List` and
//! `Map`). A nominal scalar is `int` valued, so it enters as [`ValueDomain::Scalar`]
//! and its equality is base-`int` equality — it needs no distinct case. Products
//! compare field-wise in canonical leaf order, with a sparse field's presence part
//! of the comparison; sums compare by exact `(variant, payload)`; a list compares
//! element-wise in order; a map compares its `(key, value)` pairs in key order. The
//! VM's `Eq*`/`EqEnum` opcodes compute equality as a structural `Value == Value`
//! derive; this relation is their specification, agreed by a conformance test rather
//! than re-derived (routing every comparison through a domain conversion would only
//! add cost). No top-level `collection == collection` operator exists — the checker
//! rejects it — but a collection reached inside a compared product or sum (a struct
//! field or an enum/`Option` payload) participates in that aggregate's equality, so
//! this relation must recurse through collections to stay a faithful specification.
//!
//! **Collection key order** admits only the closed orderable durable-key scalar
//! set — `int`, `string`, `bool`, and `bytes` (a nominal key is `int` valued, so it
//! too enters as a scalar). A product, a sum, or a collection is never an orderable
//! key: durable keys are single ordered scalar columns, so [`KeyDomain`] gains no
//! aggregate case, and a map's key is a [`KeyScalar`] here, not a nested
//! [`ValueDomain`]. C03 extends it lexicographically to composite key tuples.

use std::cmp::Ordering;

use crate::codec::key::KeyScalar;
use crate::codec::value::RuntimeScalar;

/// A value in the equality domain: `unit`, an admitted scalar (a nominal enters
/// here as its base `int`), a product (record/struct) of per-field values in
/// canonical leaf order, or a sum (`enum`/`Option`/`Result`) as its selected
/// variant and dense payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueDomain {
    Unit,
    Scalar(RuntimeScalar),
    /// A product value: its type index and one slot per field in canonical leaf
    /// order. A required field's slot is always `Some`; a sparse field's slot is
    /// `None` when absent, so presence is part of the value.
    Product {
        ty: u16,
        fields: Vec<Option<ValueDomain>>,
    },
    /// A sum value: its type index, the selected variant, and that variant's dense
    /// payload in declaration order (empty for a payloadless member).
    Sum {
        ty: u16,
        variant: u16,
        payload: Vec<ValueDomain>,
    },
    /// A finite list value: its COLLTYPES instantiation index and its elements in
    /// order. The index discriminates distinct instantiations exactly as a product's
    /// `ty` does, matching the VM's structural `Value::List` comparison.
    List {
        idx: u16,
        items: Vec<ValueDomain>,
    },
    /// An ordered map value: its COLLTYPES instantiation index and its entries in
    /// ascending key order (the order the VM maintains). A key is a [`KeyScalar`] —
    /// a collection is never a map key — and a value is an arbitrary domain value.
    Map {
        idx: u16,
        entries: Vec<(KeyScalar, ValueDomain)>,
    },
    /// An entry identity: its store root and the key tuple that addresses one entry.
    /// A nominal leaf — not an aggregate — so it adds no fifth recursive-payload
    /// family: its equality is root identity plus key-tuple equality, each column a
    /// [`KeyScalar`], reusing scalar-key equality. [`RootId`] is a distinct newtype
    /// from a product's/sum's `ty` type-table index, so an identity can never alias a
    /// record or enum domain point. This case specifies value EQUALITY only; an entry
    /// identity carries no codec, order, or index meaning here (a durably stored
    /// identity is a separately reserved decision).
    Identity {
        root: RootId,
        keys: Vec<KeyScalar>,
    },
}

/// A store-root discriminator in the value domain, a distinct newtype from the
/// `ty: u16` type-table index a [`ValueDomain::Product`] or [`ValueDomain::Sum`]
/// carries. Keeping the namespaces separate makes an identity-vs-record domain
/// collision unrepresentable rather than merely untested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootId(pub u16);

/// A value in the collection-key-order domain: `unit`, or one admitted key scalar.
/// Products and sums are not orderable durable keys, so they have no case here; C03
/// extends this with composite key tuples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDomain {
    Unit,
    Scalar(KeyScalar),
}

/// The value-equality relation. `unit` equals `unit`; two scalars are equal exactly
/// when they are the same scalar type and value; two products are equal when they
/// share a type and every field agrees (a vacant field equals only a vacant field);
/// two sums are equal when they share a type, select the same variant, and carry an
/// equal payload; two lists are equal when they share an instantiation and are
/// element-wise equal in order; two maps are equal when they share an instantiation
/// and every `(key, value)` pair agrees in ascending key order; values of different
/// shapes are never equal. This is the one owner of value equality; the VM computes
/// it as a structural `Value` derive that this relation specifies.
pub fn value_equality(a: &ValueDomain, b: &ValueDomain) -> bool {
    match (a, b) {
        (ValueDomain::Unit, ValueDomain::Unit) => true,
        (ValueDomain::Scalar(x), ValueDomain::Scalar(y)) => x == y,
        (
            ValueDomain::Product { ty: ta, fields: fa },
            ValueDomain::Product { ty: tb, fields: fb },
        ) => ta == tb && fields_equal(fa, fb),
        (
            ValueDomain::Sum {
                ty: ta,
                variant: va,
                payload: pa,
            },
            ValueDomain::Sum {
                ty: tb,
                variant: vb,
                payload: pb,
            },
        ) => ta == tb && va == vb && payload_equal(pa, pb),
        (ValueDomain::List { idx: ia, items: xa }, ValueDomain::List { idx: ib, items: xb }) => {
            ia == ib && payload_equal(xa, xb)
        }
        (
            ValueDomain::Map {
                idx: ia,
                entries: ea,
            },
            ValueDomain::Map {
                idx: ib,
                entries: eb,
            },
        ) => ia == ib && entries_equal(ea, eb),
        (
            ValueDomain::Identity {
                root: ra,
                keys: ka,
            },
            ValueDomain::Identity {
                root: rb,
                keys: kb,
            },
        ) => ra == rb && ka == kb,
        _ => false,
    }
}

/// Map equality: equal instantiation, equal length, and each `(key, value)` pair
/// agrees in the shared ascending key order — keys by exact scalar equality, values
/// by recursion.
fn entries_equal(a: &[(KeyScalar, ValueDomain)], b: &[(KeyScalar, ValueDomain)]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|((ka, va), (kb, vb))| ka == kb && value_equality(va, vb))
}

/// Field-wise product equality in canonical leaf order: equal length, and each slot
/// agrees — a vacant slot equals only a vacant slot, and two present slots by
/// recursion.
fn fields_equal(a: &[Option<ValueDomain>], b: &[Option<ValueDomain>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (None, None) => true,
            (Some(x), Some(y)) => value_equality(x, y),
            _ => false,
        })
}

/// Payload equality in declaration order: equal length and each leaf equal.
fn payload_equal(a: &[ValueDomain], b: &[ValueDomain]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| value_equality(x, y))
}

/// The collection-key-order relation: a total order over the key domain. Scalars
/// order by the order-preserving [`KeyScalar`] order (the same order the durable
/// store observes); `unit` is a single value that sorts before every scalar. This
/// is the one owner of key order; C03 extends it lexicographically to composite
/// keys.
pub fn collection_key_order(a: &KeyDomain, b: &KeyDomain) -> Ordering {
    match (a, b) {
        (KeyDomain::Unit, KeyDomain::Unit) => Ordering::Equal,
        (KeyDomain::Unit, KeyDomain::Scalar(_)) => Ordering::Less,
        (KeyDomain::Scalar(_), KeyDomain::Unit) => Ordering::Greater,
        (KeyDomain::Scalar(x), KeyDomain::Scalar(y)) => x.cmp(y),
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyDomain, RootId, ValueDomain, collection_key_order, value_equality};
    use crate::codec::key::KeyScalar;
    use crate::codec::value::RuntimeScalar;
    use std::cmp::Ordering;

    #[test]
    fn identity_equality_is_root_and_key_tuple() {
        let id = |root: u16, keys: Vec<KeyScalar>| ValueDomain::Identity {
            root: RootId(root),
            keys,
        };
        // Same root and equal key tuple are equal; a differing key breaks equality.
        assert!(value_equality(
            &id(0, vec![KeyScalar::Int(5)]),
            &id(0, vec![KeyScalar::Int(5)]),
        ));
        assert!(!value_equality(
            &id(0, vec![KeyScalar::Int(5)]),
            &id(0, vec![KeyScalar::Int(6)]),
        ));
        // A differing key-tuple length is not equal.
        assert!(!value_equality(
            &id(0, vec![KeyScalar::Int(5)]),
            &id(0, vec![KeyScalar::Int(5), KeyScalar::Str("x".into())]),
        ));
        // Distinct roots are never equal even with an equal key tuple — defense in depth
        // (the checker forbids the comparison, but the spec must still separate roots).
        assert!(!value_equality(
            &id(0, vec![KeyScalar::Int(5)]),
            &id(1, vec![KeyScalar::Int(5)]),
        ));
        // An identity never equals a non-identity domain point: the injectivity probe
        // against a same-`ty`, same-fields product that Option A would have aliased.
        let record_like = ValueDomain::Product {
            ty: 0,
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(5)))],
        };
        assert!(!value_equality(&id(0, vec![KeyScalar::Int(5)]), &record_like));
        assert!(!value_equality(
            &id(0, vec![KeyScalar::Int(5)]),
            &ValueDomain::Scalar(RuntimeScalar::Int(5)),
        ));
    }

    #[test]
    fn value_equality_covers_unit_and_every_admitted_scalar() {
        assert!(value_equality(&ValueDomain::Unit, &ValueDomain::Unit));
        let samples = [
            RuntimeScalar::Bool(true),
            RuntimeScalar::Int(-7),
            RuntimeScalar::Str("hi".into()),
            RuntimeScalar::Bytes(vec![0x00, 0xff]),
        ];
        for value in &samples {
            let a = ValueDomain::Scalar(value.clone());
            assert!(value_equality(&a, &a), "{value:?} equals itself");
            // A scalar never equals unit.
            assert!(!value_equality(&a, &ValueDomain::Unit));
        }
        // Distinct values of the same type differ.
        assert!(!value_equality(
            &ValueDomain::Scalar(RuntimeScalar::Int(1)),
            &ValueDomain::Scalar(RuntimeScalar::Int(2)),
        ));
        // Same shape, different scalar type: not equal.
        assert!(!value_equality(
            &ValueDomain::Scalar(RuntimeScalar::Int(1)),
            &ValueDomain::Scalar(RuntimeScalar::Bool(true)),
        ));
    }

    fn int(v: i64) -> ValueDomain {
        ValueDomain::Scalar(RuntimeScalar::Int(v))
    }

    #[test]
    fn product_equality_is_field_wise_with_presence() {
        // Two records of the same type agree iff every field agrees.
        let a = ValueDomain::Product {
            ty: 0,
            fields: vec![
                Some(int(1)),
                Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into()))),
            ],
        };
        assert!(value_equality(&a, &a.clone()));
        // A differing field breaks equality.
        let b = ValueDomain::Product {
            ty: 0,
            fields: vec![
                Some(int(2)),
                Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into()))),
            ],
        };
        assert!(!value_equality(&a, &b));
        // A vacant sparse field equals only a vacant field, never a present one.
        let absent = ValueDomain::Product {
            ty: 0,
            fields: vec![Some(int(1)), None],
        };
        let present = ValueDomain::Product {
            ty: 0,
            fields: vec![
                Some(int(1)),
                Some(ValueDomain::Scalar(RuntimeScalar::Str("".into()))),
            ],
        };
        assert!(value_equality(&absent, &absent.clone()));
        assert!(!value_equality(&absent, &present));
        // A different product type is never equal, even with equal fields.
        let other_ty = ValueDomain::Product {
            ty: 1,
            fields: vec![
                Some(int(1)),
                Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into()))),
            ],
        };
        assert!(!value_equality(&a, &other_ty));
    }

    #[test]
    fn nominal_scalar_equality_is_base_int() {
        // A nominal value enters the domain as its base int, so equality is int
        // equality — no distinct nominal case.
        assert!(value_equality(&int(42), &int(42)));
        assert!(!value_equality(&int(42), &int(7)));
    }

    #[test]
    fn sum_equality_is_variant_and_payload_exact() {
        // Option[int]-shaped: none is variant 0 (empty payload), some(v) is variant 1.
        let none = ValueDomain::Sum {
            ty: 3,
            variant: 0,
            payload: Vec::new(),
        };
        let some1 = ValueDomain::Sum {
            ty: 3,
            variant: 1,
            payload: vec![int(1)],
        };
        let some2 = ValueDomain::Sum {
            ty: 3,
            variant: 1,
            payload: vec![int(2)],
        };
        assert!(value_equality(&none, &none.clone()));
        assert!(value_equality(&some1, &some1.clone()));
        // Different variant, different payload, and a none-vs-some are all unequal.
        assert!(!value_equality(&none, &some1));
        assert!(!value_equality(&some1, &some2));
        // A different sum type never equals, even at the same variant and payload.
        let other_ty = ValueDomain::Sum {
            ty: 4,
            variant: 1,
            payload: vec![int(1)],
        };
        assert!(!value_equality(&some1, &other_ty));
    }

    #[test]
    fn list_equality_is_element_wise_in_order() {
        let list = |idx, items: Vec<ValueDomain>| ValueDomain::List { idx, items };
        let a = list(0, vec![int(1), int(2), int(3)]);
        assert!(value_equality(&a, &a.clone()));
        // A differing element breaks equality.
        assert!(!value_equality(&a, &list(0, vec![int(1), int(9), int(3)])));
        // Order matters: the same multiset in a different order is not equal.
        assert!(!value_equality(&a, &list(0, vec![int(3), int(2), int(1)])));
        // A differing length is not equal.
        assert!(!value_equality(&a, &list(0, vec![int(1), int(2)])));
        // A different instantiation index is never equal, even with equal elements.
        assert!(!value_equality(&a, &list(1, vec![int(1), int(2), int(3)])));
        // Two empty lists of the same instantiation are equal.
        assert!(value_equality(&list(0, vec![]), &list(0, vec![])));
    }

    #[test]
    fn map_equality_is_pairwise_in_key_order() {
        let map = |idx, entries: Vec<(KeyScalar, ValueDomain)>| ValueDomain::Map { idx, entries };
        let a = map(
            0,
            vec![(KeyScalar::Int(1), int(10)), (KeyScalar::Int(2), int(20))],
        );
        assert!(value_equality(&a, &a.clone()));
        // A differing value breaks equality.
        assert!(!value_equality(
            &a,
            &map(
                0,
                vec![(KeyScalar::Int(1), int(10)), (KeyScalar::Int(2), int(99))]
            ),
        ));
        // A differing key breaks equality.
        assert!(!value_equality(
            &a,
            &map(
                0,
                vec![(KeyScalar::Int(1), int(10)), (KeyScalar::Int(3), int(20))]
            ),
        ));
        // A different instantiation index is never equal.
        assert!(!value_equality(
            &a,
            &map(
                1,
                vec![(KeyScalar::Int(1), int(10)), (KeyScalar::Int(2), int(20))]
            ),
        ));
    }

    #[test]
    fn nested_collection_equality_recurses() {
        // A list of Option[int]-shaped sums: recursion must reach the sum payload.
        let some = |v| ValueDomain::Sum {
            ty: 3,
            variant: 1,
            payload: vec![int(v)],
        };
        let none = ValueDomain::Sum {
            ty: 3,
            variant: 0,
            payload: Vec::new(),
        };
        let a = ValueDomain::List {
            idx: 2,
            items: vec![some(1), none.clone()],
        };
        let b = ValueDomain::List {
            idx: 2,
            items: vec![some(1), some(1)],
        };
        assert!(value_equality(&a, &a.clone()));
        assert!(!value_equality(&a, &b));
        // A map whose value is a list recurses through both.
        let inner = |items| ValueDomain::List { idx: 0, items };
        let m = |v0: Vec<ValueDomain>| ValueDomain::Map {
            idx: 4,
            entries: vec![(KeyScalar::Str("k".into()), inner(v0))],
        };
        assert!(value_equality(&m(vec![int(1)]), &m(vec![int(1)])));
        assert!(!value_equality(&m(vec![int(1)]), &m(vec![int(2)])));
    }

    #[test]
    fn a_collection_never_equals_a_non_collection() {
        let list = ValueDomain::List {
            idx: 0,
            items: vec![int(1)],
        };
        let map = ValueDomain::Map {
            idx: 0,
            entries: vec![(KeyScalar::Int(1), int(1))],
        };
        assert!(!value_equality(&list, &map));
        assert!(!value_equality(&list, &int(1)));
        assert!(!value_equality(&map, &ValueDomain::Unit));
    }

    #[test]
    fn nested_sum_equality_recurses() {
        // some(some(1)) vs some(none): Option[Option[int]] distinguishes the nesting.
        let inner_some = ValueDomain::Sum {
            ty: 3,
            variant: 1,
            payload: vec![int(1)],
        };
        let inner_none = ValueDomain::Sum {
            ty: 3,
            variant: 0,
            payload: Vec::new(),
        };
        let outer_some_some = ValueDomain::Sum {
            ty: 5,
            variant: 1,
            payload: vec![inner_some.clone()],
        };
        let outer_some_none = ValueDomain::Sum {
            ty: 5,
            variant: 1,
            payload: vec![inner_none],
        };
        assert!(value_equality(&outer_some_some, &outer_some_some.clone()));
        assert!(!value_equality(&outer_some_some, &outer_some_none));
    }

    #[test]
    fn cross_shape_values_are_never_equal() {
        let scalar = int(1);
        let product = ValueDomain::Product {
            ty: 0,
            fields: vec![Some(int(1))],
        };
        let sum = ValueDomain::Sum {
            ty: 0,
            variant: 0,
            payload: Vec::new(),
        };
        assert!(!value_equality(&scalar, &product));
        assert!(!value_equality(&product, &sum));
        assert!(!value_equality(&sum, &ValueDomain::Unit));
    }

    #[test]
    fn key_order_is_total_with_unit_least() {
        assert_eq!(
            collection_key_order(&KeyDomain::Unit, &KeyDomain::Unit),
            Ordering::Equal
        );
        let scalar = KeyDomain::Scalar(KeyScalar::Int(0));
        assert_eq!(
            collection_key_order(&KeyDomain::Unit, &scalar),
            Ordering::Less
        );
        assert_eq!(
            collection_key_order(&scalar, &KeyDomain::Unit),
            Ordering::Greater
        );
        // Scalars use the order-preserving KeyScalar order.
        assert_eq!(
            collection_key_order(
                &KeyDomain::Scalar(KeyScalar::Int(1)),
                &KeyDomain::Scalar(KeyScalar::Int(2)),
            ),
            Ordering::Less
        );
        assert_eq!(
            collection_key_order(
                &KeyDomain::Scalar(KeyScalar::Str("a".into())),
                &KeyDomain::Scalar(KeyScalar::Str("b".into())),
            ),
            Ordering::Less
        );
    }

    /// The key-order domain is exactly `{unit, scalar}` — a collection is never a
    /// key. This exhaustive match is the enforcement artifact: adding an aggregate
    /// `KeyDomain` variant (a collection key) would fail to compile here, forcing a
    /// deliberate revisit of the durable-key contract. The value domain admits `List`
    /// and `Map`, but the key domain admits neither, and a map's key is a
    /// [`KeyScalar`], not a nested `ValueDomain`.
    #[test]
    fn key_domain_admits_only_unit_and_scalar() {
        let key = KeyDomain::Scalar(KeyScalar::Int(0));
        match key {
            KeyDomain::Unit | KeyDomain::Scalar(_) => {}
        }
    }
}
