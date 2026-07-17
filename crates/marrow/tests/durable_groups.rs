//! Root-level durable `group` operations, executed end to end.
//!
//! A root-level unkeyed `group` is a markerless value unit of its containing entry: its
//! presence is the entry's presence, and it is addressed by the root's own key-path. A
//! whole entry read joins the group's leaves; a group is read, replaced, and erased whole
//! through `^root(key).group`; a group leaf is read and rewritten through
//! `^root(key).group.leaf` as a whole-group read-modify-write (read the group, update the
//! leaf, replace the group), so a sibling leaf survives. Whole-entry and whole-group
//! replacement are exact — they rewrite the payload's own fields and drop omitted sparse
//! leaves — while leaving the entry's keyed `branch` descendants in place.
//!
//! These tests drive the whole production path — capture -> compile -> verify -> attach ->
//! VM — against one persistent ephemeral attachment, with a composite root key so the
//! group ops are exercised over a multi-column key-path.

use marrow_verify::VerifiedImage;
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// application, product, the top-level `title` field, the composite root placement and its
// two key columns, the `details` group and its two sparse leaves, then the `notes` branch
// (a `root` placement), its key, and its one field.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.shelf 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key books.id 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id field Book.details.language 22222222222222222222222222222222\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     high-water 0\n\
     end\n";

// A composite-keyed root of one required scalar field, a root-level group of two sparse
// scalar leaves, and a single-column-keyed scalar-field branch.
const SOURCE: &str = r#"resource Book {
    required title: string

    details {
        pages: int
        language: string
    }

    notes[noteId: string] {
        required text: string
    }
}

store ^books[shelf: int, id: int]: Book

pub fn setBook(shelf: int, id: int, t: string, p: int, lang: string) {
    transaction {
        ^books[shelf, id] = Book(title: t, details: Book.details(pages: p, language: lang))
    }
}

pub fn setBookNoDetails(shelf: int, id: int, t: string) {
    transaction {
        ^books[shelf, id] = Book(title: t)
    }
}

pub fn readTitle(shelf: int, id: int): string? {
    return ^books[shelf, id].title
}

pub fn readPages(shelf: int, id: int): int? {
    return ^books[shelf, id].details.pages
}

pub fn readLanguage(shelf: int, id: int): string? {
    return ^books[shelf, id].details.language
}

pub fn setPages(shelf: int, id: int, p: int) {
    transaction {
        ^books[shelf, id].details.pages = p
    }
}

pub fn clearPages(shelf: int, id: int) {
    transaction {
        delete ^books[shelf, id].details.pages
    }
}

pub fn replaceDetails(shelf: int, id: int, p: int) {
    transaction {
        ^books[shelf, id].details = Book.details(pages: p)
    }
}

pub fn eraseDetails(shelf: int, id: int) {
    transaction {
        delete ^books[shelf, id].details
    }
}

pub fn detailPagesWhole(shelf: int, id: int): int? {
    if const d = ^books[shelf, id].details {
        return d.pages
    }
    return absent
}

pub fn addNote(shelf: int, id: int, nid: string, body: string) {
    transaction {
        ^books[shelf, id].notes[nid] = Book.notes(text: body)
    }
}

pub fn noteText(shelf: int, id: int, nid: string): string? {
    if const n = ^books[shelf, id].notes[nid] {
        return n.text
    }
    return absent
}
"#;

fn compile_verify(source: &str, ids: &str) -> VerifiedImage {
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
        Ephemeral::Parked => panic!("a flat root with a root-level group must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a marrow_verify::SealedExport {
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

fn run_result(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> DurableRun {
    run_export(image, attachment, export(image, name), args)
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn s(text: &str) -> Value {
    Value::Text(text.into())
}

fn as_int(value: Option<Value>) -> Option<i64> {
    match value {
        Some(Value::Optional(Some(inner))) => match *inner {
            Value::Int(n) => Some(n),
            other => panic!("expected int, got {other:?}"),
        },
        Some(Value::Optional(None)) => None,
        other => panic!("expected optional int, got {other:?}"),
    }
}

fn as_str(value: Option<Value>) -> Option<String> {
    match value {
        Some(Value::Optional(Some(inner))) => match *inner {
            Value::Text(text) => Some(text.to_string()),
            other => panic!("expected string, got {other:?}"),
        },
        Some(Value::Optional(None)) => None,
        other => panic!("expected optional string, got {other:?}"),
    }
}

/// A whole-entry create with a supplied group joins the group's leaves into the entry:
/// both the whole-group read and the group-leaf reads recover the written leaves, and the
/// top-level field reads independently.
#[test]
fn a_group_bearing_entry_stores_and_reads_whole_and_by_leaf() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "setBook",
        vec![i(1), i(7), s("Small Gods"), i(384), s("en")],
    );

    assert_eq!(
        as_str(run(&image, &mut store, "readTitle", vec![i(1), i(7)])),
        Some("Small Gods".to_string())
    );
    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
        Some(384)
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
        Some("en".to_string())
    );
    // The whole group materializes and its leaf projects.
    assert_eq!(
        as_int(run(
            &image,
            &mut store,
            "detailPagesWhole",
            vec![i(1), i(7)]
        )),
        Some(384)
    );
    // A distinct composite key names a distinct entry: it reads absent.
    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(2), i(7)])),
        None
    );
}

/// A group-leaf assignment is a whole-group read-modify-write: it updates the addressed
/// leaf and preserves the sibling leaf.
#[test]
fn a_group_leaf_assignment_preserves_the_sibling_leaf() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "setBook",
        vec![i(1), i(7), s("Small Gods"), i(384), s("en")],
    );
    run(&image, &mut store, "setPages", vec![i(1), i(7), i(400)]);

    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
        Some(400)
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
        Some("en".to_string()),
        "the sibling leaf survives the leaf read-modify-write"
    );
}

/// Clearing a sparse group leaf clears only that leaf; the sibling leaf survives.
#[test]
fn clearing_a_group_leaf_clears_only_that_leaf() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "setBook",
        vec![i(1), i(7), s("Small Gods"), i(384), s("en")],
    );
    run(&image, &mut store, "clearPages", vec![i(1), i(7)]);

    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
        None
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
        Some("en".to_string())
    );
}

/// A whole-group replacement is exact: it rewrites the group's own leaves and drops the
/// leaves the assigned value omits, without disturbing the top-level field or the branch.
#[test]
fn a_whole_group_replace_is_exact_and_leaves_siblings_intact() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "setBook",
        vec![i(1), i(7), s("Small Gods"), i(384), s("en")],
    );
    run(
        &image,
        &mut store,
        "addNote",
        vec![i(1), i(7), s("n1"), s("hello")],
    );
    // Replace details with only `pages` supplied: `language` is dropped.
    run(
        &image,
        &mut store,
        "replaceDetails",
        vec![i(1), i(7), i(512)],
    );

    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
        Some(512)
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
        None,
        "the omitted leaf is dropped by exact replacement"
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readTitle", vec![i(1), i(7)])),
        Some("Small Gods".to_string()),
        "the top-level field is untouched by a whole-group replace"
    );
    assert_eq!(
        as_str(run(
            &image,
            &mut store,
            "noteText",
            vec![i(1), i(7), s("n1")]
        )),
        Some("hello".to_string()),
        "the keyed branch descendant is untouched by a whole-group replace"
    );
}

/// Erasing a group clears only its leaves; the top-level field and the branch survive.
#[test]
fn erasing_a_group_clears_only_its_leaves() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "setBook",
        vec![i(1), i(7), s("Small Gods"), i(384), s("en")],
    );
    run(
        &image,
        &mut store,
        "addNote",
        vec![i(1), i(7), s("n1"), s("hello")],
    );
    run(&image, &mut store, "eraseDetails", vec![i(1), i(7)]);

    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
        None
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
        None
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readTitle", vec![i(1), i(7)])),
        Some("Small Gods".to_string())
    );
    assert_eq!(
        as_str(run(
            &image,
            &mut store,
            "noteText",
            vec![i(1), i(7), s("n1")]
        )),
        Some("hello".to_string())
    );
}

/// A whole-entry assignment is exact over group leaves too: assigning a value that omits
/// the all-sparse group defaults it to its vacant form, dropping every leaf. The branch
/// descendant survives (whole assignment replaces only the entry's own payload).
#[test]
fn a_whole_entry_assignment_erases_omitted_group_leaves() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "setBook",
        vec![i(1), i(7), s("Small Gods"), i(384), s("en")],
    );
    run(
        &image,
        &mut store,
        "addNote",
        vec![i(1), i(7), s("n1"), s("hello")],
    );
    // Whole-assign a value with details omitted: the group defaults to vacant leaves.
    run(
        &image,
        &mut store,
        "setBookNoDetails",
        vec![i(1), i(7), s("Reaper Man")],
    );

    assert_eq!(
        as_str(run(&image, &mut store, "readTitle", vec![i(1), i(7)])),
        Some("Reaper Man".to_string())
    );
    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
        None,
        "the whole assignment drops the omitted group's leaves"
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
        None
    );
    assert_eq!(
        as_str(run(
            &image,
            &mut store,
            "noteText",
            vec![i(1), i(7), s("n1")]
        )),
        Some("hello".to_string()),
        "whole assignment leaves the keyed branch descendant in place"
    );
}

/// A group has no independent existence: a group-leaf read over an absent entry reads
/// absent, and a group-leaf write (a whole-group read-modify-write) over an absent entry
/// is a no-op — the group replace over a payload-absent entry is Missing and touches
/// nothing, so no entry is conjured from a leaf write.
#[test]
fn a_group_leaf_over_an_absent_entry_reads_absent_and_writes_are_no_ops() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(9), i(9)])),
        None
    );

    // A leaf write over the absent entry runs cleanly but stores nothing.
    match run_result(&image, &mut store, "setPages", vec![i(9), i(9), i(1)]) {
        DurableRun::Ran(Ok(_)) => {}
        other => panic!(
            "a group-leaf write over an absent entry must run cleanly: {:?}",
            DebugRun(&other)
        ),
    }
    assert_eq!(
        as_int(run(&image, &mut store, "readPages", vec![i(9), i(9)])),
        None,
        "a group-leaf write conjures no entry"
    );
    assert_eq!(
        as_str(run(&image, &mut store, "readTitle", vec![i(9), i(9)])),
        None,
        "the entry remains absent after a group-leaf write"
    );
}
