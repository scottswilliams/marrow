//! The counter-as-allocator idiom, executed end to end from source (DX04).
//!
//! Marrow has no `nextId` built-in: an application that needs a fresh, monotonically
//! increasing key mints one from a durable counter it owns. This pins the documented
//! journey ([Counter allocation](../../../docs/language/idioms.md)) green: a single
//! `name`-keyed `^idseq` counter root, a `place seq` bind, the `seq.value ?? 0`
//! read-with-default, the write-back, and the payload create all share the export's one
//! `transaction`, so the increment and the create commit as a unit. The test drives the
//! whole production path — capture -> compile -> verify -> attach -> VM — against one
//! persistent ephemeral attachment, so a later read observes an earlier allocation.
//!
//! Source and identity ledger live on disk as the shared harness's
//! `fixtures/v01/counter_allocation` fixture; this file is the assertions.

mod common;

use common::Project;
use marrow_vm::Value;

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

/// Allocating two ids in sequence yields 1 then 2, each create lands under its minted
/// key, and the shared counter ends at the last value allocated. The counter is minted
/// by the first allocation, so `seq.value ?? 0` supplies the first-use value with no
/// separate initialization.
#[test]
fn the_counter_allocates_monotonic_keys_and_binds_each_create() {
    let mut session = Project::from_fixture("counter_allocation").session();

    assert_eq!(
        session.call("createBook", vec![Value::Text("alpha".into())]),
        Some(Value::Int(1)),
        "the first allocation reads the absent counter as 0 and mints 1",
    );
    assert_eq!(
        session.call("createBook", vec![Value::Text("beta".into())]),
        Some(Value::Int(2)),
        "the second allocation advances the persisted counter to 2",
    );

    assert_eq!(
        session.call("titleOf", vec![Value::Int(1)]),
        some_text("alpha"),
        "the first create landed under key 1",
    );
    assert_eq!(
        session.call("titleOf", vec![Value::Int(2)]),
        some_text("beta"),
        "the second create landed under key 2",
    );

    assert_eq!(
        session.call("seqValue", vec![]),
        some_int(2),
        "the counter persists its last allocated value",
    );
}
