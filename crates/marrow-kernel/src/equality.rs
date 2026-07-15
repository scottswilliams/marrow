//! The value-equality and collection-key-order owner.
//!
//! Marrow's value domain is closed. Two relations over it are load-bearing across
//! the language: **value equality** (the `==`/`!=` relation and the identity of a
//! stored value) and **collection key order** (the total order traversal and keyed
//! collections observe). This module is the single owner of both relations over the
//! current domain, reusing the scalar equality of [`RuntimeScalar`] and the
//! order-preserving key order of [`KeyScalar`] rather than restating either.
//!
//! **Value equality** covers the C02 aggregate value domain: `unit`, the admitted
//! scalars, dense/sparse products (records and structs), and closed sums (user
//! `enum`s and the `Option`/`Result` instantiations). A nominal scalar is `int`
//! valued, so it enters as [`ValueDomain::Scalar`] and its equality is base-`int`
//! equality — it needs no distinct case. Products compare field-wise in canonical
//! leaf order, with a sparse field's presence part of the comparison; sums compare
//! by exact `(variant, payload)`. The VM's `Eq*` opcodes compute the scalar and
//! enum cases; this relation is their specification, agreed by a conformance test
//! rather than re-derived (the VM's `Value` equality is structural, so routing it
//! through a per-comparison domain conversion would only add cost). C03 extends the
//! domain with sequences and keyed collections and consumes this relation for
//! element equality.
//!
//! **Collection key order** admits only the closed orderable durable-key scalar
//! set — `int`, `string`, `bool`, and `bytes` (a nominal key is `int` valued, so it
//! too enters as a scalar). A product or a sum is not an orderable key: durable keys
//! are single ordered scalar columns, so [`KeyDomain`] gains no aggregate case here.
//! C03 extends it lexicographically to composite key tuples.

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
}

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
/// equal payload; values of different shapes are never equal. This is the one owner
/// of value equality; the VM's `Eq*` opcodes compute the scalar and enum cases, and
/// C03 extends it to sequences and keyed collections.
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
        _ => false,
    }
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
    use super::{KeyDomain, ValueDomain, collection_key_order, value_equality};
    use crate::codec::key::KeyScalar;
    use crate::codec::value::RuntimeScalar;
    use std::cmp::Ordering;

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
}
