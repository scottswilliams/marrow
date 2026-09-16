//! Nested keyed `branch` whole-entry and field-exact operations, executed end to end.
//!
//! A branch may itself declare a keyed `branch`: `^root(k).notes(nid).tags(tid)` addresses
//! a distinct durable node two levels below the root, by the three-element key-path
//! `[root_key, note_key, tag_key]`. Every law that holds for a single-level branch holds
//! uniformly at depth — the payload-only replace/erase law (adjudication 1), the deep set
//! under absent ancestors leaving them descendant-only (adjudication 2), and bounded
//! traversal over an inner layer (adjudication 3). These tests drive the whole production
//! path — capture -> compile -> verify -> attach -> VM — over one persistent ephemeral
//! attachment, so a committed write is observable by a later read invocation.

use crate::common::{Diagnostics, Project, Steer};
use marrow_vm::Value;

// application, product, the top-level `title` field, the root and its key, then the
// `notes` branch (a `root` placement) with its key and required `text`, then the nested
// `tags` branch inside `notes` (its own `root` placement) with its key and its int and
// sparse-bool fields.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id root Book.notes.tags 40404040404040404040404040404040\n\
     id key Book.notes.tags.tagId 41414141414141414141414141414141\n\
     id field Book.notes.tags.weight 42424242424242424242424242424242\n\
     id field Book.notes.tags.hot 43434343434343434343434343434343\n\
     high-water 0\n\
     end\n";

/// A `Book { title }` root with a `notes(noteId: string)` branch that itself holds a
/// nested `tags(tagId: int)` branch of a required `weight: int` and a sparse `hot: bool`.
/// The exports exercise the nested constructor, field-exact reads/writes at depth, deep
/// sets under absent ancestors, whole-entry erase preserving descendants, and bounded
/// traversal over the inner `tags` layer.
const SOURCE: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string

        tags[tagId: int] {
            required weight: int
            hot: bool
        }
    }
}

store ^books[id: int]: Book

pub fn setRoot(id: int, t: string) {
    transaction {
        ^books[id] = Book(title: t)
    }
}

pub fn addNote(id: int, nid: string, body: string) {
    transaction {
        ^books[id].notes[nid] = Book.notes(text: body)
    }
}

pub fn addTag(id: int, nid: string, tid: int, w: int) {
    transaction {
        ^books[id].notes[nid].tags[tid] = Book.notes.tags(weight: w)
    }
}

pub fn addFullTag(id: int, nid: string, tid: int, w: int, h: bool) {
    transaction {
        ^books[id].notes[nid].tags[tid] = Book.notes.tags(weight: w, hot: h)
    }
}

pub fn setTagWeight(id: int, nid: string, tid: int, w: int) {
    transaction {
        place tag = ^books[id].notes[nid].tags[tid]
        if exists(tag) {
            tag.weight = w
        }
    }
}

pub fn setTagHot(id: int, nid: string, tid: int, h: bool) {
    transaction {
        place tag = ^books[id].notes[nid].tags[tid]
        if exists(tag) {
            tag.hot = h
        }
    }
}

pub fn clearTagHot(id: int, nid: string, tid: int) {
    transaction {
        delete ^books[id].notes[nid].tags[tid].hot
    }
}

pub fn eraseTag(id: int, nid: string, tid: int) {
    transaction {
        delete ^books[id].notes[nid].tags[tid]
    }
}

pub fn eraseNote(id: int, nid: string) {
    transaction {
        delete ^books[id].notes[nid]
    }
}

pub fn tagWeight(id: int, nid: string, tid: int): int? {
    return ^books[id].notes[nid].tags[tid].weight
}

pub fn tagHot(id: int, nid: string, tid: int): bool? {
    return ^books[id].notes[nid].tags[tid].hot
}

pub fn tagWeightMaterialized(id: int, nid: string, tid: int): int? {
    if const t = ^books[id].notes[nid].tags[tid] {
        return t.weight
    }
    return absent
}

pub fn tagPresent(id: int, nid: string, tid: int): bool {
    return exists(^books[id].notes[nid].tags[tid])
}

pub fn notePresent(id: int, nid: string): bool {
    return exists(^books[id].notes[nid])
}

pub fn rootPresent(id: int): bool {
    return exists(^books[id])
}

pub fn sumTags(id: int, nid: string): int {
    var total = 0
    for t in ^books[id].notes[nid].tags at most 100 {
        total += t
    } on more {
        total = total + 1000
    }
    return total
}

pub fn sumTagsBounded(id: int, nid: string): int {
    var total = 0
    for t in ^books[id].notes[nid].tags at most 2 {
        total += t
    } on more {
        total = total + 1000
    }
    return total
}
"#;

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

fn some_bool(b: bool) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Bool(b)))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

fn present(b: bool) -> Option<Value> {
    Some(Value::Bool(b))
}

fn s(v: &str) -> Value {
    Value::Text(v.into())
}

/// The nested constructor writes a whole sub-branch entry, and field-exact reads and a
/// whole-entry materialized read observe its fields two levels below the root.
#[test]
fn a_nested_branch_constructor_and_field_reads_round_trip() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    let key = || vec![Value::Int(1), s("n"), Value::Int(7)];

    session.call(
        "addFullTag",
        vec![
            Value::Int(1),
            s("n"),
            Value::Int(7),
            Value::Int(42),
            Value::Bool(true),
        ],
    );
    assert_eq!(session.call("tagWeight", key()), some_int(42));
    assert_eq!(session.call("tagHot", key()), some_bool(true));
    assert_eq!(
        session.call("tagWeightMaterialized", key()),
        some_int(42),
        "a whole nested-branch entry materializes its record two levels down",
    );
    assert_eq!(session.call("tagPresent", key()), present(true));
}

/// Adjudication 2: a deep whole-entry write on the sub-branch under absent ancestors is
/// admitted and creates the tag node, while both ancestors (note and root) stay
/// descendant-only — no ancestor markers, presence facts only from explicit probes.
#[test]
fn a_deep_write_under_absent_ancestors_leaves_them_descendant_only() {
    let mut session = Project::single(SOURCE).ids(IDS).session();

    // Whole-entry create of the tag with nothing else written.
    session.call(
        "addTag",
        vec![Value::Int(2), s("n"), Value::Int(5), Value::Int(9)],
    );
    let tag = || vec![Value::Int(2), s("n"), Value::Int(5)];
    assert_eq!(session.call("tagPresent", tag()), present(true));
    assert_eq!(session.call("tagWeight", tag()), some_int(9));
    assert_eq!(
        session.call("notePresent", vec![Value::Int(2), s("n")]),
        present(false),
        "the note ancestor has no marker: descendant-only",
    );
    assert_eq!(
        session.call("rootPresent", vec![Value::Int(2)]),
        present(false),
        "the root ancestor has no marker: descendant-only",
    );
}

/// The four-state marker/target laws over a nested branch entry, read field-exact:
/// marker absent (both reads absent), marker present with the sparse absent (weight
/// present, hot absent), both present, and a whole replace that omits the sparse field
/// drops it — the payload-only replace law (adjudication 1) at depth.
#[test]
fn a_nested_branch_entry_upholds_the_four_state_laws() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    let key = || vec![Value::Int(5), s("n"), Value::Int(3)];

    assert_eq!(session.call("tagPresent", key()), present(false));
    assert_eq!(session.call("tagWeight", key()), absent());
    assert_eq!(session.call("tagHot", key()), absent());

    session.call(
        "addTag",
        vec![Value::Int(5), s("n"), Value::Int(3), Value::Int(8)],
    );
    assert_eq!(session.call("tagWeight", key()), some_int(8));
    assert_eq!(
        session.call("tagHot", key()),
        absent(),
        "an omitted sparse field reads absent while the required field is present",
    );

    session.call(
        "addFullTag",
        vec![
            Value::Int(5),
            s("n"),
            Value::Int(3),
            Value::Int(8),
            Value::Bool(true),
        ],
    );
    assert_eq!(session.call("tagHot", key()), some_bool(true));

    // A whole replace that omits the sparse field drops it (exact replacement).
    session.call(
        "addTag",
        vec![Value::Int(5), s("n"), Value::Int(3), Value::Int(8)],
    );
    assert_eq!(
        session.call("tagHot", key()),
        absent(),
        "a whole replace omitting the sparse field drops it at depth",
    );
}

/// Adjudication 1 at depth: a whole-entry erase of a middle branch (`notes`) is
/// payload-only — it removes the note's marker and fields but preserves its keyed `tags`
/// descendants, and a whole-entry erase of the tag removes only that tag.
#[test]
fn a_middle_branch_erase_preserves_nested_descendants() {
    let mut session = Project::single(SOURCE).ids(IDS).session();

    session.call("addNote", vec![Value::Int(6), s("n"), s("body")]);
    session.call(
        "addTag",
        vec![Value::Int(6), s("n"), Value::Int(1), Value::Int(11)],
    );
    assert_eq!(
        session.call("notePresent", vec![Value::Int(6), s("n")]),
        present(true)
    );

    // Erase the note payload: payload-only, so the nested tag survives.
    session.call("eraseNote", vec![Value::Int(6), s("n")]);
    assert_eq!(
        session.call("notePresent", vec![Value::Int(6), s("n")]),
        present(false),
        "the note payload is gone",
    );
    assert_eq!(
        session.call("tagWeight", vec![Value::Int(6), s("n"), Value::Int(1)]),
        some_int(11),
        "a payload-only note erase preserves its nested tag descendant",
    );

    // Erase the tag: removes only the tag entry.
    session.call("eraseTag", vec![Value::Int(6), s("n"), Value::Int(1)]);
    assert_eq!(
        session.call("tagPresent", vec![Value::Int(6), s("n"), Value::Int(1)]),
        present(false),
    );
}

/// A field-exact clear of the sparse `hot` on a nested tag leaves the required `weight`
/// intact — the field-exact clear is scoped to its own leaf two levels down.
#[test]
fn a_deep_field_exact_clear_preserves_the_required_field() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    let key = || vec![Value::Int(7), s("n"), Value::Int(2)];

    session.call(
        "addFullTag",
        vec![
            Value::Int(7),
            s("n"),
            Value::Int(2),
            Value::Int(3),
            Value::Bool(true),
        ],
    );
    session.call("clearTagHot", vec![Value::Int(7), s("n"), Value::Int(2)]);
    assert_eq!(session.call("tagHot", key()), absent());
    assert_eq!(
        session.call("tagWeight", key()),
        some_int(3),
        "the field-exact clear left the required field intact",
    );
}

/// Adjudication 3: bounded traversal over the inner `tags` layer iterates the tag keys of
/// one fixed `[book, note]` ancestor path in ascending order, honors the `at most N`
/// bound with the `on more` bit, and is scoped to that note — a tag under a different note
/// is not visited.
#[test]
fn bounded_traversal_iterates_an_inner_branch_layer_under_a_fixed_ancestor_path() {
    let mut session = Project::single(SOURCE).ids(IDS).session();

    // Three tags under (book 8, note "n"), and one under a sibling note "m".
    for tid in [3, 1, 2] {
        session.call(
            "addTag",
            vec![Value::Int(8), s("n"), Value::Int(tid), Value::Int(0)],
        );
    }
    session.call(
        "addTag",
        vec![Value::Int(8), s("m"), Value::Int(99), Value::Int(0)],
    );

    // Sum all tag keys under note "n": 1 + 2 + 3 = 6, no `on more`.
    assert_eq!(
        session.call("sumTags", vec![Value::Int(8), s("n")]),
        Some(Value::Int(6)),
        "the inner layer iterates its own note's tags in ascending order",
    );
    // Bounded at 2: freezes tags 1 and 2 (sum 3), a third existed → +1000.
    assert_eq!(
        session.call("sumTagsBounded", vec![Value::Int(8), s("n")]),
        Some(Value::Int(1003)),
        "the bound freezes the first two keys and the on-more bit fires",
    );
    // The sibling note "m" has exactly one tag (key 99); its layer is independent.
    assert_eq!(
        session.call("sumTags", vec![Value::Int(8), s("m")]),
        Some(Value::Int(99)),
        "the inner traversal is scoped to its ancestor path",
    );
}

// --- A sub-branch is not a field of a materialized branch value. ---

/// Compile `SOURCE` plus `body`, returning the rejection diagnostics.
fn compile_diags(body: &str) -> Diagnostics {
    match Project::single(&format!("{SOURCE}\n{body}"))
        .ids(IDS)
        .try_image()
    {
        Ok(_) => panic!("expected the checker to reject chaining a sub-branch off a record value"),
        Err(diagnostics) => diagnostics,
    }
}

#[test]
fn chaining_a_subbranch_off_a_materialized_branch_steers_to_the_durable_path() {
    // `if const n = ^books[id].notes[nid]` materializes the branch entry as a local record;
    // its nested `tags` branch is a distinct durable node, not a projectable field, so the
    // chain is refused with the same steering `check.type` a top-level branch gets.
    let diagnostics = compile_diags(
        "pub fn tagWeight(id: int, nid: string, tid: int): int? {\n    if const n = ^books[id].notes[nid] {\n        return n.tags[tid].weight\n    }\n    return absent\n}\n",
    );
    let diagnostic = diagnostics.only("check.type");
    // A materialized branch entry is not owned by the resource registry, so the steer
    // names the branch alone and the renderer gives the generic durable-path fix.
    assert_eq!(
        diagnostic.steer(),
        Some(&Steer::KeyedBranch {
            branch: "tags".to_string(),
            resource: None,
        }),
        "{}",
        diagnostic.message()
    );
}
