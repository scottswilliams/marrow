//! Conformance: the VM's runtime `Value` equality agrees with the kernel's
//! `value_equality`, the one owner of the relation (C02 V6).
//!
//! The VM's `Eq*` opcodes compute equality as `Value == Value` (a structural
//! derive). The kernel `value_equality` over [`ValueDomain`] is the specification.
//! Rather than convert on every comparison (the crate DAG allows delegation, but it
//! would only add cost to a structural derive), this test pins that the two agree
//! over the C02 value domain: for every pair of representative values, the kernel
//! relation and the runtime `==` return the same verdict.

use std::rc::Rc;

use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::equality::{ValueDomain, value_equality};
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
            unreachable!("a top-level optional is outside the C02 equality domain")
        }
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
