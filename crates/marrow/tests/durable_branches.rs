//! Keyed `branch` whole-entry operations, executed end to end.
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

use marrow_verify::{SealedExport, SealedInstr, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, RuntimeFault, Value, mint_ephemeral, run_export};

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
const SOURCE: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string
        pinned: bool
    }
}

store ^books[id: int]: Book

pub fn addNote(id: int, nid: string, body: string) {
    transaction {
        ^books[id].notes[nid] = Book.notes(text: body)
    }
}

pub fn addFullNote(id: int, nid: string, body: string, flag: bool) {
    transaction {
        ^books[id].notes[nid] = Book.notes(text: body, pinned: flag)
    }
}

pub fn notePinned(id: int, nid: string): bool? {
    if const n = ^books[id].notes[nid] {
        return n.pinned
    }
    return absent
}

pub fn setRoot(id: int, t: string) {
    transaction {
        ^books[id] = Book(title: t)
    }
}

pub fn eraseRoot(id: int) {
    transaction {
        delete ^books[id]
    }
}

pub fn eraseNote(id: int, nid: string) {
    transaction {
        delete ^books[id].notes[nid]
    }
}

pub fn rootPresent(id: int): bool {
    return exists(^books[id])
}

pub fn notePresent(id: int, nid: string): bool {
    return exists(^books[id].notes[nid])
}

pub fn rootTitle(id: int): string? {
    return ^books[id].title
}

pub fn noteText(id: int, nid: string): string? {
    if const n = ^books[id].notes[nid] {
        return n.text
    }
    return absent
}
"#;

// application, product, the top-level `title` field, the root and its key, then two
// branches of different shape: `notes` (a `root` placement) keyed by string with one
// string field, and `tags` keyed by int with an int field and a sparse bool field.
const IDS_TWO: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id root Book.tags 40404040404040404040404040404040\n\
     id key Book.tags.tagId 41414141414141414141414141414141\n\
     id field Book.tags.weight 42424242424242424242424242424242\n\
     id field Book.tags.hot 43434343434343434343434343434343\n\
     high-water 0\n\
     end\n";

/// A flat-executable root with two single-column-keyed scalar-field branches of
/// deliberately different shape: `notes(noteId: string)` holds one string field, while
/// `tags(tagId: int)` holds an int field and a sparse bool field. The differing key
/// types and field shapes make a crossed branch alignment observable — a note read
/// through the tags plan (or a tag through the notes plan) cannot reproduce the written
/// fields.
const SOURCE_TWO: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string
    }

    tags[tagId: int] {
        required weight: int
        hot: bool
    }
}

store ^books[id: int]: Book

pub fn addNote(id: int, nid: string, body: string) {
    transaction {
        ^books[id].notes[nid] = Book.notes(text: body)
    }
}

pub fn addTag(id: int, tid: int, w: int, flag: bool) {
    transaction {
        ^books[id].tags[tid] = Book.tags(weight: w, hot: flag)
    }
}

pub fn noteText(id: int, nid: string): string? {
    if const n = ^books[id].notes[nid] {
        return n.text
    }
    return absent
}

pub fn tagWeight(id: int, tid: int): int? {
    if const t = ^books[id].tags[tid] {
        return t.weight
    }
    return absent
}

pub fn tagHot(id: int, tid: int): bool? {
    if const t = ^books[id].tags[tid] {
        return t.hot
    }
    return absent
}
"#;

fn compile_verify(source: &str) -> VerifiedImage {
    compile_verify_ids(source, IDS)
}

fn compile_verify_ids(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
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

fn function_instrs<'a>(image: &'a VerifiedImage, name: &str) -> &'a [SealedInstr] {
    image
        .functions()
        .iter()
        .find(|function| function.name() == name)
        .expect("function present")
        .instrs()
}

/// A sparse-field set through a two-key branch `place` dominated by an `exists(p)`
/// guard lowers to the strict present-entry form (`DurSetSparsePresent`) carrying the
/// place's whole `[root, branch]` key-path, not the bare create-or-reconcile
/// `DurSetSparse`: a presence-guarded set over a branch place lowers to the strict
/// form, so the guard is enforced at the kernel marker rather than silently widened.
#[test]
fn a_guarded_branch_place_sparse_set_lowers_strict_over_the_whole_key_path() {
    let image = compile_verify_ids(FIELD_SOURCE, IDS);
    let instrs = function_instrs(&image, "setPinnedViaPlace");
    let strict: Vec<&[u16]> = instrs
        .iter()
        .filter_map(|instr| match instr {
            SealedInstr::DurSetSparsePresent { key_slots, .. } => Some(key_slots.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(
        strict.len(),
        1,
        "the guarded branch-place set lowers strict"
    );
    assert_eq!(
        strict[0].len(),
        2,
        "the strict branch set carries the whole `[root, branch]` key-path",
    );
    assert!(
        !instrs
            .iter()
            .any(|instr| matches!(instr, SealedInstr::DurSetSparse(_))),
        "no bare create-or-reconcile set remains",
    );
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
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a flat root with a simple branch must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
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

fn some_bool(b: bool) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Bool(b)))))
}

/// The four-state marker/target laws over a branch entry with a required `text` field
/// and a sparse `pinned` field, read through the materialized record:
///
/// - marker absent: `exists` false, required and sparse reads absent;
/// - marker present, sparse target absent: required reads present, sparse reads absent;
/// - marker present, sparse target present: both read present;
/// - a whole replace that omits the sparse field drops it (exact replacement).
///
/// A branch whole-entry create supplies every required field (the constructor enforces
/// it), so a required field is never missing while the marker is present — there is no
/// partial-marker state to read.
#[test]
fn a_branch_entry_upholds_the_four_state_required_and_optional_laws() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    let key = || vec![Value::Int(5), Value::Text("a".into())];

    // Marker absent: exists false, both field reads absent.
    assert_eq!(
        run(&image, &mut attachment, "notePresent", key()),
        present(false)
    );
    assert_eq!(run(&image, &mut attachment, "noteText", key()), absent());
    assert_eq!(run(&image, &mut attachment, "notePinned", key()), absent());

    // Marker present, sparse absent: required present, sparse absent.
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(5),
            Value::Text("a".into()),
            Value::Text("hi".into()),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "notePresent", key()),
        present(true)
    );
    assert_eq!(
        run(&image, &mut attachment, "noteText", key()),
        some_text("hi")
    );
    assert_eq!(
        run(&image, &mut attachment, "notePinned", key()),
        absent(),
        "an omitted sparse field reads absent while the required field is present"
    );

    // Marker present, sparse present: both read present.
    run(
        &image,
        &mut attachment,
        "addFullNote",
        vec![
            Value::Int(5),
            Value::Text("a".into()),
            Value::Text("ho".into()),
            Value::Bool(true),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "noteText", key()),
        some_text("ho")
    );
    assert_eq!(
        run(&image, &mut attachment, "notePinned", key()),
        some_bool(true)
    );

    // A whole replace that omits the sparse field drops it (exact replacement).
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(5),
            Value::Text("a".into()),
            Value::Text("hi again".into()),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "noteText", key()),
        some_text("hi again")
    );
    assert_eq!(
        run(&image, &mut attachment, "notePinned", key()),
        absent(),
        "a whole replace omitting the sparse field drops it"
    );
}

/// Two keyed branches of different shape on one flat-executable root, driven end to
/// end. The lowerer aligns each branch's materialized record type and whole-payload
/// site to its source-ordered name, key, and field plan positionally; a crossed
/// alignment would pair one branch's fields with the other branch's record or site.
/// The branches differ in key type and field shape — a one-string-field `notes` keyed
/// by string, a two-field `tags` keyed by int — so creating an entry in each and
/// reading both back materialized pins the alignment: each branch's own fields land on
/// that branch, and a swap could not reproduce them.
#[test]
fn two_branches_of_different_shape_keep_their_own_fields() {
    let image = compile_verify_ids(SOURCE_TWO, IDS_TWO);
    let mut attachment = attach(&image);

    // A note (one string field) and a tag (an int field and a bool field) under the
    // same book: two branches, one root, one persistent attachment.
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(1),
            Value::Text("n".into()),
            Value::Text("hello".into()),
        ],
    );
    run(
        &image,
        &mut attachment,
        "addTag",
        vec![
            Value::Int(1),
            Value::Int(9),
            Value::Int(42),
            Value::Bool(true),
        ],
    );

    // Each branch materializes its own fields: the string note text, and the int/bool
    // tag fields. A crossed record/site alignment could not read these back.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(1), Value::Text("n".into())]
        ),
        some_text("hello")
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagWeight",
            vec![Value::Int(1), Value::Int(9)]
        ),
        some_int(42)
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagHot",
            vec![Value::Int(1), Value::Int(9)]
        ),
        some_bool(true)
    );

    // The two branch families are independent: reading each branch at a key that was
    // written only to the other is absent, so a create did not spill across the
    // positional alignment onto the sibling branch.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagWeight",
            vec![Value::Int(1), Value::Int(100)]
        ),
        absent(),
        "the note write did not appear in the tags branch"
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(1), Value::Text("missing".into())]
        ),
        absent(),
        "the tag write did not appear in the notes branch"
    );
}

// --- Field-exact branch operations (E03w slice A). ---
//
// `^root(k).branch(bk).field = v`, its clear (`delete ^root(k).branch(bk).field`),
// its read, and its presence test address one leaf of a branch entry directly, one
// level below the root. A field-exact set on a not-yet-present branch entry stages
// that leaf and reconciles the *branch* node's marker and required fields at commit
// exactly as a root field set reconciles the root node — proving the reconcile
// extension is node-parametric.

/// The same `Book`/`notes(noteId)` schema as `SOURCE`/`IDS`, with field-exact branch
/// operations: a sparse-field set/clear, a required-field set that reconcile-creates
/// the branch entry, a field read, and a field presence test.
const FIELD_SOURCE: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string
        pinned: bool
    }
}

store ^books[id: int]: Book

pub fn addNote(id: int, nid: string, body: string) {
    transaction {
        ^books[id].notes[nid] = Book.notes(text: body)
    }
}

pub fn setPinned(id: int, nid: string, flag: bool) {
    transaction {
        ^books[id].notes[nid].pinned = flag
    }
}

pub fn clearPinned(id: int, nid: string) {
    transaction {
        delete ^books[id].notes[nid].pinned
    }
}

pub fn setText(id: int, nid: string, body: string) {
    transaction {
        ^books[id].notes[nid].text = body
    }
}

pub fn notePinned(id: int, nid: string): bool? {
    return ^books[id].notes[nid].pinned
}

pub fn pinnedPresent(id: int, nid: string): bool {
    return exists(^books[id].notes[nid].pinned)
}

pub fn noteText(id: int, nid: string): string? {
    if const n = ^books[id].notes[nid] {
        return n.text
    }
    return absent
}

pub fn setPinnedViaPlace(id: int, nid: string, flag: bool) {
    transaction {
        place note = ^books[id].notes[nid]
        if exists(note) {
            note.pinned = flag
        }
    }
}

pub fn notePinnedViaPlace(id: int, nid: string): bool? {
    place note = ^books[id].notes[nid]
    return note.pinned
}

pub fn rootPresent(id: int): bool {
    return exists(^books[id])
}
"#;

/// Run an export whose commit is expected to fault, returning the runtime fault code.
fn run_fault(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> &'static str {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Err(fault)) => RuntimeFault::code(&fault),
        other => panic!("{name} did not fault as expected: {:?}", DebugRun(&other)),
    }
}

/// A field-exact sparse set and clear on a present branch entry change only that
/// field, and its field read and presence test observe it, while the branch's other
/// fields are undisturbed.
#[test]
fn a_field_exact_sparse_set_and_clear_leave_sibling_branch_fields_intact() {
    let image = compile_verify_ids(FIELD_SOURCE, IDS);
    let mut attachment = attach(&image);
    let key = || vec![Value::Int(6), Value::Text("a".into())];

    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(6),
            Value::Text("a".into()),
            Value::Text("hi".into()),
        ],
    );
    assert_eq!(run(&image, &mut attachment, "notePinned", key()), absent());
    assert_eq!(
        run(&image, &mut attachment, "pinnedPresent", key()),
        present(false)
    );

    // Field-exact set of the sparse `pinned`: the required `text` is preserved.
    run(
        &image,
        &mut attachment,
        "setPinned",
        vec![Value::Int(6), Value::Text("a".into()), Value::Bool(true)],
    );
    assert_eq!(
        run(&image, &mut attachment, "notePinned", key()),
        some_bool(true)
    );
    assert_eq!(
        run(&image, &mut attachment, "pinnedPresent", key()),
        present(true)
    );
    assert_eq!(
        run(&image, &mut attachment, "noteText", key()),
        some_text("hi"),
        "a field-exact sparse set preserves the branch's required field"
    );

    // Field-exact clear of the sparse `pinned`: text still preserved.
    run(
        &image,
        &mut attachment,
        "clearPinned",
        vec![Value::Int(6), Value::Text("a".into())],
    );
    assert_eq!(run(&image, &mut attachment, "notePinned", key()), absent());
    assert_eq!(
        run(&image, &mut attachment, "noteText", key()),
        some_text("hi"),
        "a field-exact clear preserves the branch's required field"
    );
}

/// A field-exact set of a branch entry's *required* field on an absent branch stages
/// that leaf and reconcile-creates the branch node's marker at commit (all required
/// fields present), leaving the root descendant-only. This proves the commit reconcile
/// extends to a branch node's marker and record, not the root's.
#[test]
fn a_field_exact_required_set_reconcile_creates_the_branch_node() {
    let image = compile_verify_ids(FIELD_SOURCE, IDS);
    let mut attachment = attach(&image);

    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(7)]),
        present(false)
    );
    run(
        &image,
        &mut attachment,
        "setText",
        vec![
            Value::Int(7),
            Value::Text("a".into()),
            Value::Text("made".into()),
        ],
    );
    // The branch marker was created by the reconcile, so a whole-entry read materializes.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(7), Value::Text("a".into())]
        ),
        some_text("made"),
        "the branch node's marker was created by the commit reconcile"
    );
    // The branch set did not create the root: it stays descendant-only.
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(7)]),
        present(false),
        "a field-exact branch set does not create the root node"
    );
}

/// Reconcile soundness: staging a *sparse* branch field on an absent branch entry
/// whose required field is missing rolls the whole transaction back with
/// `run.required_missing`, and nothing persists. If the reconcile mistakenly checked
/// the root node's required fields (the root's `title`) instead of the branch node's
/// (`text`), it would not roll back here.
#[test]
fn a_branch_sparse_set_missing_the_required_branch_field_rolls_back() {
    let image = compile_verify_ids(FIELD_SOURCE, IDS);
    let mut attachment = attach(&image);

    assert_eq!(
        run_fault(
            &image,
            &mut attachment,
            "setPinned",
            vec![Value::Int(8), Value::Text("a".into()), Value::Bool(true)]
        ),
        marrow_codes::Code::RunRequiredMissing.as_str(),
        "a staged branch sparse set with the branch's required field missing rolls back",
    );
    // The rolled-back transaction persisted nothing: the branch field is absent.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "pinnedPresent",
            vec![Value::Int(8), Value::Text("a".into())]
        ),
        present(false),
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(8), Value::Text("a".into())]
        ),
        absent(),
    );
}

/// A field-exact set on one branch entry does not leak to a sibling branch entry.
#[test]
fn a_field_exact_set_is_scoped_to_its_branch_entry() {
    let image = compile_verify_ids(FIELD_SOURCE, IDS);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(9),
            Value::Text("a".into()),
            Value::Text("one".into()),
        ],
    );
    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(9),
            Value::Text("b".into()),
            Value::Text("two".into()),
        ],
    );
    run(
        &image,
        &mut attachment,
        "setPinned",
        vec![Value::Int(9), Value::Text("a".into()), Value::Bool(true)],
    );

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePinned",
            vec![Value::Int(9), Value::Text("a".into())]
        ),
        some_bool(true)
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePinned",
            vec![Value::Int(9), Value::Text("b".into())]
        ),
        absent(),
        "a field-exact set on entry a did not leak to sibling b"
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "noteText",
            vec![Value::Int(9), Value::Text("b".into())]
        ),
        some_text("two")
    );
}

/// Field-exact operations thread through a two-key branch `place`: a field read and a
/// guarded (`if exists`) sparse set address the branch entry through the place's
/// pre-evaluated `[root, branch]` key-path, and the guarded set preserves the branch's
/// required field.
#[test]
fn branch_place_field_operations_read_and_guarded_set_through_the_two_key_place() {
    let image = compile_verify_ids(FIELD_SOURCE, IDS);
    let mut attachment = attach(&image);
    let key = || vec![Value::Int(10), Value::Text("a".into())];

    run(
        &image,
        &mut attachment,
        "addNote",
        vec![
            Value::Int(10),
            Value::Text("a".into()),
            Value::Text("hi".into()),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "notePinnedViaPlace", key()),
        absent()
    );

    run(
        &image,
        &mut attachment,
        "setPinnedViaPlace",
        vec![Value::Int(10), Value::Text("a".into()), Value::Bool(true)],
    );
    assert_eq!(
        run(&image, &mut attachment, "notePinnedViaPlace", key()),
        some_bool(true),
        "a branch-place field read and guarded set thread through the two-key place",
    );
    assert_eq!(
        run(&image, &mut attachment, "noteText", key()),
        some_text("hi"),
        "the guarded branch-place set preserved the required field",
    );
}

// --- DX05 gap 2: a branch is not a field of a materialized entry record. ---

fn compile_diags(body: &str) -> Vec<marrow_compile::SourceDiagnostic> {
    let source = format!("{SOURCE}\n{body}");
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    match marrow_compile::compile(&project) {
        Ok(_) => panic!("expected the checker to reject chaining a branch off a record value"),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => panic!("source-triggered compiler failures must remain diagnostics"),
    }
}

#[test]
fn chaining_a_branch_off_a_materialized_record_steers_to_the_durable_path() {
    // `if const b = ^books[id]` materializes the whole entry as a local record value; a
    // keyed branch is a distinct durable node, not a projectable field of that record. The
    // access is refused with a steering `check.type` at the branch name, naming the branch,
    // the rule, and the durable-path fix.
    let diagnostics = compile_diags(
        "pub fn noteText(id: int, nid: string): string? {\n    if const b = ^books[id] {\n        return b.notes[nid].text\n    }\n    return absent\n}\n",
    );
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.code == "check.type")
        .unwrap_or_else(|| panic!("no check.type diagnostic in {diagnostics:#?}"));
    // The span points at the branch name `notes` in `b.notes`.
    assert_eq!(diagnostic.line(), 70, "{}", diagnostic.message);
    // Voice: fact (the branch in source spelling), rule, then the canonical durable-path fix.
    assert!(
        diagnostic.message.contains("`notes` is a keyed branch of `Book`"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.contains("distinct durable node")
            && diagnostic.message.contains("`^books["),
        "{}",
        diagnostic.message
    );
}
