//! Widened durable field values, stored and read end to end.
//!
//! A durable field's stored value is drawn from the closed storable set — a scalar, a
//! dense `struct` (product), or a closed `enum`/`Option`/`Result` (sum). The durable
//! value codec frames a composite inline in the one field-leaf cell, so a widened field
//! is executable, not parked. These tests drive the whole production path — capture ->
//! compile -> verify -> attach -> VM — against one persistent ephemeral attachment,
//! storing and reading back a record-typed field, an enum-typed field, and an
//! `Option`-typed field, including the sparse `Option[string]` three-state read
//! (absent cell vs present-`none` vs present-`some`) and a widened value used in an
//! expression after read.

use marrow_vm::Value;

use crate::common::{Project, Session};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0\n\
     id product Account d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0\n\
     id root accounts b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0\n\
     id key accounts.id c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0\n\
     id field Account.id e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0\n\
     id field Account.kind e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1\n\
     id field Account.owner e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2\n\
     id field Account.note e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3\n\
     id sum Access 50505050505050505050505050505050\n\
     id member Access.reader 51515151515151515151515151515151\n\
     id member Access.writer 52525252525252525252525252525252\n\
     id member Access.admin 53535353535353535353535353535353\n\
     id sum Option[string] 60606060606060606060606060606060\n\
     id member Option[string].none 61616161616161616161616161616161\n\
     id member Option[string].some 62626262626262626262626262626262\n\
     high-water 0\n\
     end\n";

// `Account` stores a required enum (`kind`), a required dense struct (`owner`), and a
// sparse `Option[string]` (`note`). The sparse `note` gives the cell-presence axis on
// top of the in-band `Option` value — the three-state fixture.
const SOURCE: &str = r#"resource Account {
    required id: int
    required kind: Access
    required owner: Name
    note: Option<string>
}

struct Name {
    first: string
    last: string
}

enum Access {
    reader
    writer
    admin
}

store ^accounts[id: int]: Account

fn ada(): Name {
    return Name(first: "Ada", last: "Lovelace")
}

fn wrapSome(s: string): Option<string> {
    return some(s)
}

fn wrapNone(): Option<string> {
    return none
}

pub fn createReader(id: int) {
    transaction {
        ^accounts[id] = Account(id: id, kind: Access::reader, owner: ada())
    }
}

pub fn createAdmin(id: int) {
    transaction {
        ^accounts[id] = Account(id: id, kind: Access::admin, owner: ada())
    }
}

pub fn readKind(id: int): Access? {
    return ^accounts[id].kind
}

pub fn isAdmin(id: int): bool {
    if const k = ^accounts[id].kind {
        return k == Access::admin
    }
    return false
}

pub fn readOwner(id: int): Name? {
    return ^accounts[id].owner
}

pub fn createNoteSome(id: int, s: string) {
    transaction {
        ^accounts[id] = Account(id: id, kind: Access::reader, owner: ada(), note: wrapSome(s))
    }
}

pub fn createNoteNone(id: int) {
    transaction {
        ^accounts[id] = Account(id: id, kind: Access::reader, owner: ada(), note: wrapNone())
    }
}

pub fn readNote(id: int): Option<string>? {
    return ^accounts[id].note
}
"#;

/// An ephemeral session over `source` against the widened-field ledger.
fn account(source: &str) -> Session {
    Project::single(source).ids(IDS).session()
}

/// Unwrap the present value of a field read (`Optional(Some(v))`), panicking on an
/// absent cell — the read of a set field always finds the cell.
fn present(value: Option<Value>) -> Value {
    match value {
        Some(Value::Optional(Some(inner))) => *inner,
        other => panic!("expected a present field read, got {other:?}"),
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn id(n: i64) -> Vec<Value> {
    vec![Value::Int(n)]
}

#[test]
fn a_proved_place_reads_required_structs_and_enums_as_values() {
    let source = format!(
        "{SOURCE}{}",
        r#"
pub fn describe(id: int): string {
    place a = ^accounts[id]
    if not exists(a) { return "missing" }
    const owner: Name = a.owner
    const kind: Access = a.kind
    const note: Option<string>? = a.note
    if kind == Access::admin { return owner.last }
    if const value = note {
        match value {
            some(text) => { return text }
            none => { return "none" }
        }
    }
    return owner.first
}
"#
    );
    let mut session = account(&source);
    let (present_reads, plain_reads) = {
        let image = session.image();
        let code = image
            .exports()
            .iter()
            .find_map(|export| {
                let function = image
                    .function(export.function())
                    .expect("verified function");
                (function.body().name() == "describe").then_some(function)
            })
            .expect("the describe export")
            .body()
            .instrs();
        (
            code.iter()
                .filter(|op| matches!(op, marrow_verify::SealedInstr::DurReadFieldPresent { .. }))
                .count(),
            code.iter()
                .filter(|op| matches!(op, marrow_verify::SealedInstr::DurReadField(_)))
                .count(),
        )
    };
    assert_eq!(present_reads, 2);
    assert_eq!(plain_reads, 1);
    assert_eq!(session.call("describe", id(1)), Some(text("missing")));
    session.call("createReader", id(1));
    assert_eq!(session.call("describe", id(1)), Some(text("Ada")));
    session.call("createAdmin", id(1));
    assert_eq!(session.call("describe", id(1)), Some(text("Lovelace")));
    session.call("createNoteNone", id(1));
    assert_eq!(session.call("describe", id(1)), Some(text("none")));
    session.call("createNoteSome", vec![Value::Int(1), text("hello")]);
    assert_eq!(session.call("describe", id(1)), Some(text("hello")));
}

#[test]
fn a_required_enum_field_round_trips_and_drives_an_expression() {
    let mut session = account(SOURCE);
    session.call("createReader", id(1));

    // The entry's required enum reads back as `Access::reader` (variant 0, empty payload).
    match present(session.call("readKind", id(1))) {
        Value::Enum(_, variant, payload) => {
            assert_eq!(variant, 0, "reader is variant 0");
            assert!(payload.is_empty(), "reader carries no payload");
        }
        other => panic!("not an enum: {other:?}"),
    }
    // A widened value used in an expression after read: `if const` binds the read enum
    // and `==` compares it — `reader` is not `admin`.
    assert_eq!(session.call("isAdmin", id(1)), Some(Value::Bool(false)));

    // A whole-entry replace with `admin` (variant 2) round-trips a different variant, and
    // the same read-and-compare now observes it.
    session.call("createAdmin", id(1));
    match present(session.call("readKind", id(1))) {
        Value::Enum(_, variant, _) => assert_eq!(variant, 2, "admin is variant 2"),
        other => panic!("not an enum: {other:?}"),
    }
    assert_eq!(session.call("isAdmin", id(1)), Some(Value::Bool(true)));
}

#[test]
fn a_record_field_round_trips_with_its_dense_leaves() {
    let mut session = account(SOURCE);
    session.call("createReader", id(2));

    // The dense struct reads back with both leaves present, in declaration order.
    match present(session.call("readOwner", id(2))) {
        Value::Record(_, slots) => {
            assert_eq!(slots.len(), 2);
            assert_eq!(slots[0], Some(text("Ada")));
            assert_eq!(slots[1], Some(text("Lovelace")));
        }
        other => panic!("not a record: {other:?}"),
    }
}

#[test]
fn a_sparse_option_field_reads_three_distinct_states() {
    let mut session = account(SOURCE);
    session.call("createReader", id(3));

    // State 1 — the cell is absent (the sparse field was never set): read yields `none`
    // at the presence axis (`Optional(None)`), not an in-band value.
    assert_eq!(session.call("readNote", id(3)), Some(Value::Optional(None)));

    // State 2 — present `none`: the cell holds the `Option` value `none` (variant 0).
    session.call("createNoteNone", id(3));
    match present(session.call("readNote", id(3))) {
        Value::Enum(_, variant, payload) => {
            assert_eq!(variant, 0, "none is variant 0");
            assert!(payload.is_empty());
        }
        other => panic!("present-none is not an enum: {other:?}"),
    }

    // State 3 — present `some("hi")`: the cell holds `some` (variant 1) with the payload.
    session.call("createNoteSome", vec![Value::Int(3), text("hi")]);
    match present(session.call("readNote", id(3))) {
        Value::Enum(_, variant, payload) => {
            assert_eq!(variant, 1, "some is variant 1");
            assert_eq!(payload.as_ref(), [text("hi")]);
        }
        other => panic!("present-some is not an enum: {other:?}"),
    }
}
