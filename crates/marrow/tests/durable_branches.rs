//! E03 S5: keyed `branch` whole-entry operations, executed end to end.
//!
//! A single-level single-column-keyed scalar-field `branch` is a distinct durable
//! node one level below the root, addressed by the two-element key-path
//! `^root(k).branch(bk)`. Creating a branch entry under an absent root leaves the
//! root *descendant-only*: it has keyed descendants but no marker, so it reads
//! payload-absent and `exists` is false, while its branch entry is fully present.
//! Giving the root a payload with `create` does not disturb the branch, and a whole
//! branch entry materializes as its own record whose fields read locally.
//!
//! These tests drive the whole production path — capture -> compile -> verify ->
//! attach -> VM — against one persistent ephemeral attachment, so a committed branch
//! or root write is observable by a later read invocation.

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// application, product, the top-level `title` field, the root placement and its key,
// then the `notes` branch (a `root` placement), its key, and its two fields.
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
     id field Book.notes.pinned 33333333333333333333333333333333\n\
     high-water 0\n\
     end\n";

/// A root of one scalar field with a single-level single-column-keyed scalar-field
/// branch `notes`. Whole-entry operations over both the root and the branch.
const SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \n\
     \x20   notes(noteId: string)\n\
     \x20       required text: string\n\
     \x20       pinned: bool\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn addNote(id: int, nid: string, body: string)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(nid) = Book.notes(text: body)\n\
     \n\
     pub fn setRoot(id: int, t: string)\n\
     \x20   transaction\n\
     \x20       ^books(id) = Book(title: t)\n\
     \n\
     pub fn eraseRoot(id: int)\n\
     \x20   transaction\n\
     \x20       delete ^books(id)\n\
     \n\
     pub fn eraseNote(id: int, nid: string)\n\
     \x20   transaction\n\
     \x20       delete ^books(id).notes(nid)\n\
     \n\
     pub fn rootPresent(id: int): bool\n\
     \x20   return exists(^books(id))\n\
     \n\
     pub fn notePresent(id: int, nid: string): bool\n\
     \x20   return exists(^books(id).notes(nid))\n\
     \n\
     pub fn rootTitle(id: int): string?\n\
     \x20   return ^books(id).title\n\
     \n\
     pub fn noteText(id: int, nid: string): string?\n\
     \x20   if const n = ^books(id).notes(nid)\n\
     \x20       return n.text\n\
     \x20   return absent\n";

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

fn run(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        other => panic!("{name} did not run cleanly: {:?}", DebugRun(&other)),
    }
}

struct DebugRun<'a>(&'a DurableRun);
impl std::fmt::Debug for DebugRun<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DurableRun::Ran(Ok(_)) => write!(f, "Ran(Ok(value))"),
            DurableRun::Ran(Err(fault)) => write!(f, "Ran(Err({}))", fault.code()),
            DurableRun::Parked => write!(f, "Parked"),
            DurableRun::Failed(code) => write!(f, "Failed({code})"),
        }
    }
}

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => attachment,
        Ephemeral::Parked => panic!("a flat root with a simple branch must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

fn present(b: bool) -> Option<Value> {
    Some(Value::Bool(b))
}

/// Creating a branch entry under an absent root leaves the root descendant-only: it
/// reads payload-absent and `exists` is false, while the branch entry is present.
/// Giving the root a payload with `create` does not disturb the branch.
#[test]
fn a_branch_create_leaves_the_root_descendant_only_and_root_create_preserves_the_branch() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // Before any write both the root and the branch are absent.
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(1)]),
        present(false)
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(1), Value::Text("a".into())]
        ),
        present(false)
    );

    // Create a branch entry under the (absent) root.
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(1),
            Value::Text("a".into()),
            Value::Text("hello".into()),
        ],
    );

    // The root is now descendant-only: payload-absent, `exists` false, no title.
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(1)]),
        present(false),
        "a descendant-only root reads payload-absent"
    );
    assert_eq!(
        run(&image, &mut attachment, "rootTitle", vec![Value::Int(1)]),
        absent()
    );
    // The branch entry itself is fully present and materializes its record.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(1), Value::Text("a".into())]
        ),
        present(true)
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(1), Value::Text("a".into())]
        ),
        some_text("hello")
    );

    // Give the root a payload; the branch is not disturbed.
    run(
        &image,
        &mut attachment,
        "setRoot",
        vec![Value::Int(1), Value::Text("T".into())],
    );
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(1)]),
        present(true),
        "root create gave the descendant-only node a payload"
    );
    assert_eq!(
        run(&image, &mut attachment, "rootTitle", vec![Value::Int(1)]),
        some_text("T")
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(1), Value::Text("a".into())]
        ),
        present(true),
        "root create did not disturb the branch"
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(1), Value::Text("a".into())]
        ),
        some_text("hello")
    );
}

/// A whole-entry root erase is payload-only: it removes the root's marker and fields
/// but preserves its keyed branch descendants, so the root returns to descendant-only.
#[test]
fn a_root_erase_preserves_keyed_branches() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "setRoot",
        vec![Value::Int(2), Value::Text("T".into())],
    );
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(2),
            Value::Text("a".into()),
            Value::Text("hi".into()),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(2)]),
        present(true)
    );

    // Erase the whole root entry: payload-only, so the branch survives.
    run(&image, &mut attachment, "eraseRoot", vec![Value::Int(2)]);
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(2)]),
        present(false),
        "the root payload is gone"
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(2), Value::Text("a".into())]
        ),
        present(true),
        "a payload-only root erase preserves keyed branches"
    );
}

/// A branch entry erase removes only that branch entry's payload; the root and other
/// branch entries are untouched.
#[test]
fn a_branch_erase_removes_only_the_addressed_branch_entry() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "setRoot",
        vec![Value::Int(3), Value::Text("T".into())],
    );
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(3),
            Value::Text("a".into()),
            Value::Text("one".into()),
        ],
    );
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(3),
            Value::Text("b".into()),
            Value::Text("two".into()),
        ],
    );

    run(
        &image,
        &mut attachment,
        "eraseNote",
        vec![Value::Int(3), Value::Text("a".into())],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(3), Value::Text("a".into())]
        ),
        present(false),
        "the addressed branch entry is gone"
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(3), Value::Text("b".into())]
        ),
        some_text("two"),
        "a sibling branch entry is untouched"
    );
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(3)]),
        present(true),
        "the root payload is untouched"
    );
}

/// A whole-entry branch replace rewrites the branch entry exactly.
#[test]
fn a_branch_replace_is_exact() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(4),
            Value::Text("a".into()),
            Value::Text("first".into()),
        ],
    );
    // Replace the same branch entry (create-or-replace through the upsert shape).
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(4),
            Value::Text("a".into()),
            Value::Text("second".into()),
        ],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(4), Value::Text("a".into())]
        ),
        some_text("second"),
        "the branch replace overwrote the earlier text"
    );
}
