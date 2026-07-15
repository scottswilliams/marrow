//! No engine, store, session, binding, ceiling, or path object enters a VM value.
//!
//! A [`Value`](marrow_vm::Value) is pure runtime data: scalars and the composite
//! shapes built from them. Application code never receives a store handle, engine,
//! session, ceiling owner, attachment id, or resolved durable address as a value —
//! those live below the language boundary in the path kernel. This gate enforces the
//! absence two ways: a source scan proves the value module names no such type, and an
//! exhaustive match over the closed variant set is a compile-time tripwire that any
//! new variant must pass through review here.

use marrow_vm::Value;

/// The concrete engine/store/session/binding/ceiling/path type names that must never
/// appear in the VM value module — a value carrying one would have to name its type.
const FORBIDDEN_TYPES: &[&str] = &[
    "ByteEngine",
    "MemoryEngine",
    "NativeEngine",
    "DurableStore",
    "ReadView",
    "WriteTxn",
    "TxnSession",
    "ReadSession",
    "EphemeralAttachment",
    "AttachmentId",
    "DeploymentCeiling",
    "AuthorizedSite",
    "SemanticPath",
    "InvocationGrant",
];

/// The value module names no engine/store/session/binding/ceiling/path type, so no
/// such handle can be a field of any `Value` variant.
#[test]
fn the_value_module_names_no_engine_or_path_type() {
    let source = include_str!("../src/value.rs");
    for forbidden in FORBIDDEN_TYPES {
        assert!(
            !source.contains(forbidden),
            "the VM value module must not name `{forbidden}`: no engine, store, \
             session, binding, ceiling, or path object may enter a VM value",
        );
    }
}

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
            | Value::Map(_, _, _) => {}
        }
    }
    assert_pure_data(&Value::Int(0));
}
