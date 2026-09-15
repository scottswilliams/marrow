//! No engine, store, session, binding, ceiling, or path object enters a VM value.
//!
//! A [`Value`](marrow_vm::Value) is pure runtime data: scalars and the composite
//! shapes built from them. Application code never receives a store handle, engine,
//! session, ceiling owner, attachment id, or resolved durable address as a value —
//! those live below the language boundary in the path kernel. The
//! exhaustive match over the closed variant set is the compile-time tripwire any new
//! variant must pass through.

use marrow_vm::Value;

/// A compile-time tripwire: this exhaustive match lists every `Value` variant. Adding
/// a variant forces it to be updated, and a variant carrying an engine, store,
/// session, binding, ceiling, or path handle would have to be justified here. Every
/// current variant carries only runtime data.
#[test]
fn every_value_variant_carries_only_runtime_data() {
    fn assert_pure_data(value: &Value) {
        match value {
            Value::Int(_)
            | Value::Bool(_)
            | Value::Text(_)
            | Value::Bytes(_)
            | Value::Date(_)
            | Value::Instant(_)
            | Value::Duration(_)
            | Value::Record(_, _)
            | Value::Optional(_)
            | Value::Enum(_, _, _)
            | Value::List(_, _, _)
            | Value::Map(_, _, _)
            // An entry identity carries only a root index and a key tuple of scalars —
            // no engine, store, session, binding, ceiling, or path handle.
            | Value::Id(_, _) => {}
        }
    }
    assert_pure_data(&Value::Int(0));
}
