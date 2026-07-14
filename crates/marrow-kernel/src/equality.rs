//! The value-equality and collection-key-order seed.
//!
//! Marrow's value domain is closed. Two relations over it are load-bearing across
//! the language: **value equality** (the `==`/`!=` relation and the identity of a
//! stored value) and **collection key order** (the total order traversal and keyed
//! collections observe). This module is the single owner of both relations over the
//! current domain — `unit` and the admitted scalars — reusing the scalar equality of
//! [`RuntimeScalar`] and the order-preserving key order of [`KeyScalar`] rather than
//! restating either.
//!
//! It is a seed: C03 (collections and rank-1 generics) extends [`ValueDomain`] and
//! [`KeyDomain`] with the aggregate cases (sequences, keyed collections, and the
//! nominal products/sums C02 introduces) and consumes these two relations for
//! element equality and key ordering, rather than minting a parallel classifier.

use std::cmp::Ordering;

use crate::codec::key::KeyScalar;
use crate::codec::value::RuntimeScalar;

/// A value in the equality domain: `unit`, or one admitted scalar. C03 extends this
/// with the aggregate value cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueDomain {
    Unit,
    Scalar(RuntimeScalar),
}

/// A value in the collection-key-order domain: `unit`, or one admitted key scalar.
/// C03 extends this with composite key tuples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDomain {
    Unit,
    Scalar(KeyScalar),
}

/// The value-equality relation. `unit` equals `unit`; two scalars are equal exactly
/// when they are the same scalar type and value; a `unit` and a scalar are never
/// equal. This is the one owner of value equality; the VM's `Eq*` opcodes compute
/// the scalar case, and C03 extends it structurally to aggregates.
pub fn value_equality(a: &ValueDomain, b: &ValueDomain) -> bool {
    match (a, b) {
        (ValueDomain::Unit, ValueDomain::Unit) => true,
        (ValueDomain::Scalar(x), ValueDomain::Scalar(y)) => x == y,
        _ => false,
    }
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
