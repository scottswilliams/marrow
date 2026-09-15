//! Entry-identity runtime value.
//!
//! `Id(^root)` is a first-class runtime value wrapping a store root and a key tuple.
//! These tests drive the whole production path — capture -> compile -> verify -> attach
//! -> VM — proving the value's construction (`Id(^books, k)`), equality (`==`/`!=` for
//! identities of the same root), its use as a function parameter and return, and its
//! dereference (`^books[id]`) composing with an ordinary entry read. An entry identity
//! is not durably stored here: it is a runtime/lookup value only.

use marrow_vm::Value;

use crate::common::{Diagnostics, Project, Session};

// A single-`int` keyed root `^books[id: int]: Book` with a required `title`.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn shelve(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn make(id: int): Id(^books) {
    return Id(^books, id)
}

pub fn titleVia(id: Id(^books)): string? {
    return ^books[id].title
}

pub fn same(a: Id(^books), b: Id(^books)): bool {
    return a == b
}

pub fn different(a: Id(^books), b: Id(^books)): bool {
    return a != b
}
"#;

/// An ephemeral session over `source` against the entry-identity ledger.
fn open(source: &str) -> Session {
    Project::single(source).ids(IDS).session()
}

/// The diagnostics a source the checker must reject reports.
fn errors(source: &str) -> Diagnostics {
    let Err(diagnostics) = Project::single(source).ids(IDS).try_image() else {
        panic!("the checker must reject this program");
    };
    diagnostics
}

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

#[test]
fn construct_dereference_reads_the_named_entry() {
    let mut session = open(SOURCE);

    session.call("shelve", vec![Value::Int(1), Value::Text("dune".into())]);
    session.call(
        "shelve",
        vec![Value::Int(2), Value::Text("hyperion".into())],
    );

    // `make` returns an `Id(^books)` value; `titleVia` dereferences it.
    let id = session
        .call("make", vec![Value::Int(2)])
        .expect("make returns an identity value");
    assert_eq!(session.call("titleVia", vec![id]), some_text("hyperion"));
}

// A `place` bound to an identity operand `^books[id]`: the identity spreads into the
// root's key columns at the binding, so a whole-entry write and a field read through the
// place both key off the one pre-evaluated address, exactly as an inline `^books[id]`
// operation does; see the named-places reference.
const PLACE_SOURCE: &str = r#"resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn make(id: int): Id(^books) {
    return Id(^books, id)
}

pub fn shelveViaPlace(bid: Id(^books), title: string) {
    transaction {
        place p = ^books[bid]
        p = Book(title: title)
    }
}

pub fn titleViaPlace(bid: Id(^books)): string? {
    place p = ^books[bid]
    return p.title
}
"#;

#[test]
fn a_place_bound_to_an_identity_operand_writes_and_reads_the_entry() {
    let mut session = open(PLACE_SOURCE);

    let id = session
        .call("make", vec![Value::Int(3)])
        .expect("identity value");
    session.call(
        "shelveViaPlace",
        vec![id.clone(), Value::Text("neuromancer".into())],
    );
    assert_eq!(
        session.call("titleViaPlace", vec![id]),
        some_text("neuromancer"),
    );
}

#[test]
fn dereference_of_absent_entry_is_absent() {
    let mut session = open(SOURCE);
    let id = session
        .call("make", vec![Value::Int(99)])
        .expect("identity value");
    assert_eq!(
        session.call("titleVia", vec![id]),
        Some(Value::Optional(None))
    );
}

#[test]
fn identity_equality_is_key_tuple_equality() {
    let mut session = open(SOURCE);

    let id1 = session.call("make", vec![Value::Int(7)]).expect("id1");
    let id1b = session.call("make", vec![Value::Int(7)]).expect("id1b");
    let id2 = session.call("make", vec![Value::Int(8)]).expect("id2");

    assert_eq!(
        session.call("same", vec![id1.clone(), id1b]),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        session.call("same", vec![id1.clone(), id2.clone()]),
        Some(Value::Bool(false)),
    );
    assert_eq!(
        session.call("different", vec![id1, id2]),
        Some(Value::Bool(true)),
    );
}

// --- Adversarial rejections: the identity value's boundaries. ---

const PREAMBLE: &str =
    "resource Book {\n    required title: string\n}\n\nstore ^books[id: int]: Book\n\n";

fn program(body: &str) -> String {
    format!("{PREAMBLE}{body}")
}

#[test]
fn an_identity_type_over_an_undeclared_root_is_unsupported() {
    let diagnostics = errors(&program("pub fn f(x: Id(^nope)): int {\n    return 0\n}\n"));
    assert!(diagnostics.has_code("check.unsupported"));
}

#[test]
fn an_identity_constructor_over_an_undeclared_root_is_rejected() {
    let diagnostics = errors(&program(
        "pub fn f(): Id(^books) {\n    return Id(^nope, 1)\n}\n",
    ));
    assert!(diagnostics.has_code("check.type"));
}

#[test]
fn an_identity_constructor_with_the_wrong_key_arity_is_rejected() {
    // The root has one key column; supplying none is a key-arity error.
    let diagnostics = errors(&program(
        "pub fn f(): Id(^books) {\n    return Id(^books)\n}\n",
    ));
    assert!(diagnostics.has_code("check.type"));
}

#[test]
fn an_identity_constructor_with_the_wrong_key_type_is_rejected() {
    // The single key column is `int`; a string operand does not coerce.
    let diagnostics = errors(&program(
        "pub fn f(): Id(^books) {\n    return Id(^books, \"x\")\n}\n",
    ));
    assert!(diagnostics.has_code("check.type"));
}

#[test]
fn comparing_an_identity_with_a_scalar_is_rejected() {
    let diagnostics = errors(&program(
        "pub fn f(a: Id(^books)): bool {\n    return a == 5\n}\n",
    ));
    assert!(diagnostics.has_code("check.type"));
}

#[test]
fn an_identity_is_not_an_orderable_collection_key() {
    // Entry identities are not admitted in a key position, so a map keyed by one is
    // an unsupported type.
    let diagnostics = errors(&program(
        "pub fn f(): int {\n    const m: Map<Id(^books), int> = Map()\n    return length(m)\n}\n",
    ));
    assert!(diagnostics.has_code("check.unsupported"));
}

#[test]
fn a_declaration_named_id_is_reserved() {
    let diagnostics = errors(&program("pub fn Id(): int {\n    return 0\n}\n"));
    assert!(!diagnostics.is_empty());
}
