//! Conformance: the VM's runtime `Value` equality agrees with the kernel's
//! `value_equality`, the one owner of the relation (C02 V6).
//!
//! The VM's `Eq*` opcodes compute equality as `Value == Value` (a structural
//! comparison over contents; a collection's cached size never participates). The
//! kernel `value_equality` over [`ValueDomain`] is the specification. Rather than
//! convert on every comparison (the crate DAG allows delegation, but it would only
//! add cost to a structural comparison), this test pins that the two agree
//! over the C02 value domain: for every pair of representative values, the kernel
//! relation and the runtime `==` return the same verdict.

use std::rc::Rc;

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::equality::{RootId, ValueDomain, value_equality};
use marrow_vm::Value;

/// Project a runtime value into the kernel equality domain. A nominal value is an
/// `Int`, so it projects as a scalar. A top-level optional is not part of the C02
/// equality domain (no `==` consumes a `T?`); it never reaches this projection.
fn to_domain(value: &Value) -> ValueDomain {
    match value {
        Value::Int(n) => ValueDomain::Scalar(RuntimeScalar::Int(*n)),
        Value::Bool(b) => ValueDomain::Scalar(RuntimeScalar::Bool(*b)),
        Value::Text(s) => ValueDomain::Scalar(RuntimeScalar::Str(s.to_string())),
        Value::Bytes(b) => ValueDomain::Scalar(RuntimeScalar::Bytes(b.to_vec())),
        Value::Date(v) => ValueDomain::Scalar(RuntimeScalar::Date(*v)),
        Value::Instant(v) => ValueDomain::Scalar(RuntimeScalar::Instant(*v)),
        Value::Duration(v) => ValueDomain::Scalar(RuntimeScalar::Duration(*v)),
        Value::Record(ty, slots) => ValueDomain::Product {
            ty: *ty,
            fields: slots
                .iter()
                .map(|slot| slot.as_ref().map(to_domain))
                .collect(),
        },
        Value::Enum(ty, variant, payload) => ValueDomain::Sum {
            ty: *ty,
            variant: *variant,
            payload: payload.iter().map(to_domain).collect(),
        },
        Value::Optional(_) => {
            unreachable!("a top-level optional is outside the equality domain")
        }
        // No top-level `collection == collection` operator exists, but a collection
        // reached inside a compared struct or enum payload participates in that
        // aggregate's structural equality, so the domain must project it.
        Value::List(idx, _, items) => ValueDomain::List {
            idx: *idx,
            items: items.iter().map(to_domain).collect(),
        },
        Value::Map(idx, _, entries) => ValueDomain::Map {
            idx: *idx,
            entries: entries
                .iter()
                .map(|(key, value)| (key.clone(), to_domain(value)))
                .collect(),
        },
        // An entry identity projects to the nominal identity domain point: its root and
        // key tuple. It is not a durable value (`value_to_domain` refuses it at the
        // store boundary), but it participates in `==`, so the specification must cover
        // it, and this projection is the contract the runtime `Value::Id` equality meets.
        Value::Id(root, keys) => ValueDomain::Identity {
            root: RootId(*root),
            keys: keys.to_vec(),
        },
    }
}

/// Representative values across every admitted C02 shape: scalars (int carries the
/// nominal case), products with present and vacant sparse fields, and sums with
/// payloadless, payload-bearing, and nested-sum variants.
fn corpus() -> Vec<Value> {
    let text = |s: &str| Value::Text(Rc::from(s));
    vec![
        // Scalars (int also stands in for a nominal value).
        Value::Int(1),
        Value::Int(2),
        Value::Bool(true),
        Value::Bool(false),
        text("x"),
        text("y"),
        Value::Bytes(Rc::from([0x00u8, 0xff].as_slice())),
        // Temporal scalars: dates, instants, and durations differing by value.
        Value::Date(0),
        Value::Date(20_650),
        Value::Instant(0),
        Value::Instant(1_500_000_000),
        Value::Duration(-1),
        Value::Duration(90_000_000_000),
        // Products: same type, differing by a field and by sparse presence.
        Value::Record(0, Box::new([Some(Value::Int(1)), Some(text("a"))])),
        Value::Record(0, Box::new([Some(Value::Int(2)), Some(text("a"))])),
        Value::Record(0, Box::new([Some(Value::Int(1)), None])),
        // A different product type with otherwise-equal fields.
        Value::Record(1, Box::new([Some(Value::Int(1)), Some(text("a"))])),
        // Sums: Option[int]-shaped none/some, and a distinct sum type.
        Value::Enum(3, 0, Box::new([])),
        Value::Enum(3, 1, Box::new([Value::Int(1)])),
        Value::Enum(3, 1, Box::new([Value::Int(2)])),
        Value::Enum(4, 1, Box::new([Value::Int(1)])),
        // Nested sum: some(some(1)) vs some(none) over Option[Option[int]].
        Value::Enum(
            5,
            1,
            Box::new([Value::Enum(3, 1, Box::new([Value::Int(1)]))]),
        ),
        Value::Enum(5, 1, Box::new([Value::Enum(3, 0, Box::new([]))])),
        // All three shapes nested at once: an enum payload carrying a struct whose
        // fields are an Option leaf, a bytes leaf, and a bool leaf. The two members
        // differ only in the innermost Option presence and the bool leaf, so
        // agreement must recurse through sum, product, and sum again to the scalars.
        Value::Enum(
            6,
            1,
            Box::new([Value::Record(
                2,
                Box::new([
                    Some(Value::Enum(3, 1, Box::new([Value::Int(9)]))),
                    Some(Value::Bytes(Rc::from([0x01u8, 0x02].as_slice()))),
                    Some(Value::Bool(true)),
                ]),
            )]),
        ),
        Value::Enum(
            6,
            1,
            Box::new([Value::Record(
                2,
                Box::new([
                    Some(Value::Enum(3, 0, Box::new([]))),
                    Some(Value::Bytes(Rc::from([0x01u8, 0x02].as_slice()))),
                    Some(Value::Bool(false)),
                ]),
            )]),
        ),
        // Collections: reached inside a compared aggregate in real programs, so the
        // relation must recurse through them. Lists differing by element, order,
        // length, and instantiation index; the empty list; a list of structs holding
        // Options; and maps differing by value, key, and enum-valued payload.
        Value::list(7, Rc::new(vec![Value::Int(1), Value::Int(2)])),
        Value::list(7, Rc::new(vec![Value::Int(2), Value::Int(1)])),
        Value::list(7, Rc::new(vec![Value::Int(1)])),
        Value::list(7, Rc::new(vec![])),
        // A different list instantiation with equal elements: unequal by index.
        Value::list(8, Rc::new(vec![Value::Int(1), Value::Int(2)])),
        // A list of structs, each holding an Option leaf: recursion through list,
        // product, and sum. The two members differ only in the inner Option presence.
        Value::list(
            9,
            Rc::new(vec![Value::Record(
                2,
                Box::new([Some(Value::Enum(3, 1, Box::new([Value::Int(5)])))]),
            )]),
        ),
        Value::list(
            9,
            Rc::new(vec![Value::Record(
                2,
                Box::new([Some(Value::Enum(3, 0, Box::new([])))]),
            )]),
        ),
        // Maps in ascending key order, differing by a value and by a key.
        Value::map(
            10,
            Rc::new(vec![
                (KeyScalar::Str("ada".into()), Value::Int(10)),
                (KeyScalar::Str("grace".into()), Value::Int(12)),
            ]),
        ),
        Value::map(
            10,
            Rc::new(vec![
                (KeyScalar::Str("ada".into()), Value::Int(10)),
                (KeyScalar::Str("grace".into()), Value::Int(99)),
            ]),
        ),
        // A map with enum values: recursion reaches the sum payload.
        Value::map(
            11,
            Rc::new(vec![(
                KeyScalar::Int(1),
                Value::Enum(3, 1, Box::new([Value::Int(7)])),
            )]),
        ),
        Value::map(
            11,
            Rc::new(vec![(KeyScalar::Int(1), Value::Enum(3, 0, Box::new([])))]),
        ),
        // Entry identities: same root differing by key value and by key-tuple length, a
        // distinct root with an equal single key, and a composite key tuple. Paired with
        // a single-field record whose type index and field value would ALIAS an identity
        // if identities reused the product domain — the all-pairs sweep asserts they stay
        // unequal under both relations, the injectivity probe for the RootId newtype.
        Value::Id(0, Rc::from([KeyScalar::Int(1)].as_slice())),
        Value::Id(0, Rc::from([KeyScalar::Int(2)].as_slice())),
        Value::Id(
            0,
            Rc::from([KeyScalar::Int(1), KeyScalar::Str("a".into())].as_slice()),
        ),
        Value::Id(1, Rc::from([KeyScalar::Int(1)].as_slice())),
        Value::Record(0, Box::new([Some(Value::Int(1))])),
    ]
}

#[test]
fn kernel_value_equality_agrees_with_runtime_value_equality() {
    let values = corpus();
    for a in &values {
        for b in &values {
            let runtime = a == b;
            let kernel = value_equality(&to_domain(a), &to_domain(b));
            assert_eq!(
                runtime, kernel,
                "disagreement on {a:?} == {b:?}: runtime={runtime}, kernel={kernel}"
            );
        }
    }
}

#[test]
fn every_value_equals_itself_under_both_relations() {
    for value in corpus() {
        assert!(value == value.clone());
        assert!(value_equality(&to_domain(&value), &to_domain(&value)));
    }
}
