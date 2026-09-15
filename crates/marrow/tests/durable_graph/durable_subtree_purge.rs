//! Full subtree removal by composition, and the payload-only ghost it removes.
//!
//! A whole-entry `delete` is payload-only: it removes the addressed node's own payload
//! and marker while its keyed `branch` descendants persist at their own addresses — the
//! *descendant-only ghost* documented on
//! [Durable Places](../../../docs/language/durable-places.md). Removing an entry *and*
//! every descendant is therefore written as a composition: a bounded nested traversal
//! deletes each per-iteration pin innermost-first, then deletes the entry's own payload.
//! These two tests pin the contrast — the composition purge leaves nothing reachable,
//! while a bare whole-entry `delete` leaves the descendants — driving the whole
//! production path (capture -> compile -> verify -> attach -> VM) over one persistent
//! ephemeral attachment.

use crate::common::{Project, Session};
use marrow_vm::Value;

// application, product, the top-level `title` field, the root and its key, the `notes`
// branch (a `root` placement) with its key and required `text`, then the nested `tags`
// branch inside `notes` with its key and required `weight`.
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
     high-water 0\n\
     end\n";

/// A `Book { title }` root with a `notes(noteId: string)` branch that itself holds a
/// nested `tags(tagId: int)` branch. `seed` builds a three-level entry. `purge` is the
/// documented composition removal; `deleteEntry` is a bare whole-entry `delete`. The
/// remaining exports observe presence, payload, and family population at each level.
const SOURCE: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string

        tags[tagId: int] {
            required weight: int
        }
    }
}

store ^books[id: int]: Book

pub fn seed(id: int) {
    transaction {
        ^books[id] = Book(title: "root")
        ^books[id].notes["n1"] = Book.notes(text: "hello")
        ^books[id].notes["n1"].tags[7] = Book.notes.tags(weight: 3)
    }
}

pub fn purge(id: int) {
    transaction {
        for noteId, note in ^books[id].notes at most 1000 {
            for tagId, tag in ^books[id].notes[noteId].tags at most 1000 {
                delete tag
            } on more {}
            delete note
        } on more {}
        delete ^books[id]
    }
}

pub fn deleteEntry(id: int) {
    transaction {
        delete ^books[id]
    }
}

pub fn rootExists(id: int): bool {
    return exists(^books[id])
}

pub fn rootTitle(id: int): string? {
    return ^books[id].title
}

pub fn noteText(id: int, noteId: string): string? {
    if const note = ^books[id].notes[noteId] {
        return note.text
    }
    return absent
}

pub fn tagWeight(id: int, noteId: string, tagId: int): int? {
    return ^books[id].notes[noteId].tags[tagId].weight
}

pub fn notesPopulated(id: int): bool {
    return exists(^books[id].notes)
}

pub fn countNotes(id: int): int {
    var n = 0
    for noteId in ^books[id].notes at most 1000 {
        n += 1
    } on more {}
    return n
}
"#;

fn seed(session: &mut Session, id: i64) {
    session.call("seed", vec![Value::Int(id)]);
}

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

/// The seeded three-level entry is fully present before any removal: the root, the note,
/// and the tag each read their payload and each family is populated.
fn assert_fully_seeded(session: &mut Session, id: i64) {
    assert_eq!(
        session.call("rootExists", vec![Value::Int(id)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("rootTitle", vec![Value::Int(id)]),
        some_text("root")
    );
    assert_eq!(
        session.call("noteText", vec![Value::Int(id), Value::Text("n1".into())]),
        some_text("hello"),
    );
    assert_eq!(
        session.call(
            "tagWeight",
            vec![Value::Int(id), Value::Text("n1".into()), Value::Int(7)],
        ),
        some_int(3),
    );
    assert_eq!(
        session.call("notesPopulated", vec![Value::Int(id)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("countNotes", vec![Value::Int(id)]),
        Some(Value::Int(1))
    );
}

#[test]
fn the_composition_purge_removes_the_root_and_every_descendant() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed(&mut session, 1);
    assert_fully_seeded(&mut session, 1);

    session.call("purge", vec![Value::Int(1)]);

    // The root payload is gone.
    assert_eq!(
        session.call("rootExists", vec![Value::Int(1)]),
        Some(Value::Bool(false)),
        "the root no longer exists",
    );
    assert_eq!(
        session.call("rootTitle", vec![Value::Int(1)]),
        absent(),
        "the root payload reads absent",
    );
    // Every descendant is gone: the note, the tag, the note family, and the traversal.
    assert_eq!(
        session.call("noteText", vec![Value::Int(1), Value::Text("n1".into())]),
        absent(),
        "the descendant note was removed by the composition",
    );
    assert_eq!(
        session.call(
            "tagWeight",
            vec![Value::Int(1), Value::Text("n1".into()), Value::Int(7)],
        ),
        absent(),
        "the deepest descendant tag was removed by the composition",
    );
    assert_eq!(
        session.call("notesPopulated", vec![Value::Int(1)]),
        Some(Value::Bool(false)),
        "the note family is empty after the purge",
    );
    assert_eq!(
        session.call("countNotes", vec![Value::Int(1)]),
        Some(Value::Int(0)),
        "a traversal of the note family visits nothing",
    );
}

#[test]
fn a_bare_whole_entry_delete_leaves_the_descendant_ghost() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed(&mut session, 2);
    assert_fully_seeded(&mut session, 2);

    session.call("deleteEntry", vec![Value::Int(2)]);

    // The root payload is gone, exactly as for the purge.
    assert_eq!(
        session.call("rootExists", vec![Value::Int(2)]),
        Some(Value::Bool(false)),
        "the root payload is removed by a bare whole-entry delete",
    );
    assert_eq!(
        session.call("rootTitle", vec![Value::Int(2)]),
        absent(),
        "the root payload reads absent",
    );
    // But the descendants persist at their own addresses — the documented ghost.
    assert_eq!(
        session.call("noteText", vec![Value::Int(2), Value::Text("n1".into())]),
        some_text("hello"),
        "the descendant note survives the payload-only delete",
    );
    assert_eq!(
        session.call(
            "tagWeight",
            vec![Value::Int(2), Value::Text("n1".into()), Value::Int(7)],
        ),
        some_int(3),
        "the deepest descendant tag survives the payload-only delete",
    );
    assert_eq!(
        session.call("notesPopulated", vec![Value::Int(2)]),
        Some(Value::Bool(true)),
        "the note family remains populated — the ghost is reachable",
    );
    assert_eq!(
        session.call("countNotes", vec![Value::Int(2)]),
        Some(Value::Int(1)),
        "a traversal still visits the surviving note",
    );
}
