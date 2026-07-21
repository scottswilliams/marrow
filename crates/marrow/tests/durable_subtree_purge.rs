//! Full subtree removal by composition, and the payload-only ghost it removes (DX03).
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

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

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

fn compile_verify(source: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a flat root with nested scalar branches must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn run(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        DurableRun::Ran(Err(fault)) => panic!("{name} faulted: {}", fault.code()),
        DurableRun::Parked => panic!("{name} parked"),
        DurableRun::Failed(code) => panic!("{name} failed: {code}"),
    }
}

fn seed(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    id: i64,
) {
    run(image, attachment, "seed", vec![Value::Int(id)]);
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
fn assert_fully_seeded(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    id: i64,
) {
    assert_eq!(
        run(image, attachment, "rootExists", vec![Value::Int(id)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(image, attachment, "rootTitle", vec![Value::Int(id)]),
        some_text("root")
    );
    assert_eq!(
        run(
            image,
            attachment,
            "noteText",
            vec![Value::Int(id), Value::Text("n1".into())]
        ),
        some_text("hello"),
    );
    assert_eq!(
        run(
            image,
            attachment,
            "tagWeight",
            vec![Value::Int(id), Value::Text("n1".into()), Value::Int(7)],
        ),
        some_int(3),
    );
    assert_eq!(
        run(image, attachment, "notesPopulated", vec![Value::Int(id)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(image, attachment, "countNotes", vec![Value::Int(id)]),
        Some(Value::Int(1))
    );
}

#[test]
fn the_composition_purge_removes_the_root_and_every_descendant() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed(&image, &mut attachment, 1);
    assert_fully_seeded(&image, &mut attachment, 1);

    run(&image, &mut attachment, "purge", vec![Value::Int(1)]);

    // The root payload is gone.
    assert_eq!(
        run(&image, &mut attachment, "rootExists", vec![Value::Int(1)]),
        Some(Value::Bool(false)),
        "the root no longer exists",
    );
    assert_eq!(
        run(&image, &mut attachment, "rootTitle", vec![Value::Int(1)]),
        absent(),
        "the root payload reads absent",
    );
    // Every descendant is gone: the note, the tag, the note family, and the traversal.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(1), Value::Text("n1".into())]
        ),
        absent(),
        "the descendant note was removed by the composition",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagWeight",
            vec![Value::Int(1), Value::Text("n1".into()), Value::Int(7)],
        ),
        absent(),
        "the deepest descendant tag was removed by the composition",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notesPopulated",
            vec![Value::Int(1)]
        ),
        Some(Value::Bool(false)),
        "the note family is empty after the purge",
    );
    assert_eq!(
        run(&image, &mut attachment, "countNotes", vec![Value::Int(1)]),
        Some(Value::Int(0)),
        "a traversal of the note family visits nothing",
    );
}

#[test]
fn a_bare_whole_entry_delete_leaves_the_descendant_ghost() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed(&image, &mut attachment, 2);
    assert_fully_seeded(&image, &mut attachment, 2);

    run(&image, &mut attachment, "deleteEntry", vec![Value::Int(2)]);

    // The root payload is gone, exactly as for the purge.
    assert_eq!(
        run(&image, &mut attachment, "rootExists", vec![Value::Int(2)]),
        Some(Value::Bool(false)),
        "the root payload is removed by a bare whole-entry delete",
    );
    assert_eq!(
        run(&image, &mut attachment, "rootTitle", vec![Value::Int(2)]),
        absent(),
        "the root payload reads absent",
    );
    // But the descendants persist at their own addresses — the documented ghost.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(2), Value::Text("n1".into())]
        ),
        some_text("hello"),
        "the descendant note survives the payload-only delete",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagWeight",
            vec![Value::Int(2), Value::Text("n1".into()), Value::Int(7)],
        ),
        some_int(3),
        "the deepest descendant tag survives the payload-only delete",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notesPopulated",
            vec![Value::Int(2)]
        ),
        Some(Value::Bool(true)),
        "the note family remains populated — the ghost is reachable",
    );
    assert_eq!(
        run(&image, &mut attachment, "countNotes", vec![Value::Int(2)]),
        Some(Value::Int(1)),
        "a traversal still visits the surviving note",
    );
}
