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

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, RuntimeFault, Value, mint_ephemeral, run_export};

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
const SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \n\
     \x20   notes(noteId: string)\n\
     \x20       required text: string\n\
     \n\
     \x20       tags(tagId: int)\n\
     \x20           required weight: int\n\
     \x20           hot: bool\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn setRoot(id: int, t: string)\n\
     \x20   transaction\n\
     \x20       ^books(id) = Book(title: t)\n\
     \n\
     pub fn addNote(id: int, nid: string, body: string)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(nid) = Book.notes(text: body)\n\
     \n\
     pub fn addTag(id: int, nid: string, tid: int, w: int)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(nid).tags(tid) = Book.notes.tags(weight: w)\n\
     \n\
     pub fn addFullTag(id: int, nid: string, tid: int, w: int, h: bool)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(nid).tags(tid) = Book.notes.tags(weight: w, hot: h)\n\
     \n\
     pub fn setTagWeight(id: int, nid: string, tid: int, w: int)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(nid).tags(tid).weight = w\n\
     \n\
     pub fn setTagHot(id: int, nid: string, tid: int, h: bool)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(nid).tags(tid).hot = h\n\
     \n\
     pub fn clearTagHot(id: int, nid: string, tid: int)\n\
     \x20   transaction\n\
     \x20       delete ^books(id).notes(nid).tags(tid).hot\n\
     \n\
     pub fn eraseTag(id: int, nid: string, tid: int)\n\
     \x20   transaction\n\
     \x20       delete ^books(id).notes(nid).tags(tid)\n\
     \n\
     pub fn eraseNote(id: int, nid: string)\n\
     \x20   transaction\n\
     \x20       delete ^books(id).notes(nid)\n\
     \n\
     pub fn tagWeight(id: int, nid: string, tid: int): int?\n\
     \x20   return ^books(id).notes(nid).tags(tid).weight\n\
     \n\
     pub fn tagHot(id: int, nid: string, tid: int): bool?\n\
     \x20   return ^books(id).notes(nid).tags(tid).hot\n\
     \n\
     pub fn tagWeightMaterialized(id: int, nid: string, tid: int): int?\n\
     \x20   if const t = ^books(id).notes(nid).tags(tid)\n\
     \x20       return t.weight\n\
     \x20   return absent\n\
     \n\
     pub fn tagPresent(id: int, nid: string, tid: int): bool\n\
     \x20   return exists(^books(id).notes(nid).tags(tid))\n\
     \n\
     pub fn notePresent(id: int, nid: string): bool\n\
     \x20   return exists(^books(id).notes(nid))\n\
     \n\
     pub fn rootPresent(id: int): bool\n\
     \x20   return exists(^books(id))\n\
     \n\
     pub fn sumTags(id: int, nid: string): int\n\
     \x20   var total = 0\n\
     \x20   for t in ^books(id).notes(nid).tags at most 100\n\
     \x20       total += t\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn sumTagsBounded(id: int, nid: string): int\n\
     \x20   var total = 0\n\
     \x20   for t in ^books(id).notes(nid).tags at most 2\n\
     \x20       total += t\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n";

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

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => attachment,
        Ephemeral::Parked => {
            panic!("a nested single-column scalar-field branch must be executable")
        }
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_nested_branch_constructor_and_field_reads_round_trip() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    let key = || vec![Value::Int(1), s("n"), Value::Int(7)];

    run(
        &image,
        &mut attachment,
        "addFullTag",
        vec![
            Value::Int(1),
            s("n"),
            Value::Int(7),
            Value::Int(42),
            Value::Bool(true),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "tagWeight", key()),
        some_int(42)
    );
    assert_eq!(
        run(&image, &mut attachment, "tagHot", key()),
        some_bool(true)
    );
    assert_eq!(
        run(&image, &mut attachment, "tagWeightMaterialized", key()),
        some_int(42),
        "a whole nested-branch entry materializes its record two levels down",
    );
    assert_eq!(
        run(&image, &mut attachment, "tagPresent", key()),
        present(true)
    );
}

/// Adjudication 2: a deep write on the sub-branch under absent ancestors is admitted and
/// creates the tag node, while both ancestors (note and root) stay descendant-only — no
/// ancestor markers, presence facts only from explicit probes. Holds for a whole-entry
/// create and for a field-exact required set that reconcile-creates the node.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_deep_write_under_absent_ancestors_leaves_them_descendant_only() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // Whole-entry create of the tag with nothing else written.
    run(
        &image,
        &mut attachment,
        "addTag",
        vec![Value::Int(2), s("n"), Value::Int(5), Value::Int(9)],
    );
    let tag = || vec![Value::Int(2), s("n"), Value::Int(5)];
    assert_eq!(
        run(&image, &mut attachment, "tagPresent", tag()),
        present(true)
    );
    assert_eq!(
        run(&image, &mut attachment, "tagWeight", tag()),
        some_int(9)
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(2), s("n")]
        ),
        present(false),
        "the note ancestor has no marker: descendant-only",
    );
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(2)]),
        present(false),
        "the root ancestor has no marker: descendant-only",
    );

    // Field-exact required set on a fresh tag under absent ancestors reconcile-creates the
    // tag node, still leaving both ancestors descendant-only.
    run(
        &image,
        &mut attachment,
        "setTagWeight",
        vec![Value::Int(3), s("m"), Value::Int(1), Value::Int(4)],
    );
    let tag2 = || vec![Value::Int(3), s("m"), Value::Int(1)];
    assert_eq!(
        run(&image, &mut attachment, "tagWeight", tag2()),
        some_int(4)
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(3), s("m")]
        ),
        present(false)
    );
    assert_eq!(
        run(&image, &mut attachment, "rootPresent", vec![Value::Int(3)]),
        present(false)
    );
}

/// Reconcile soundness at depth: staging a *sparse* tag field on an absent tag whose
/// required `weight` is missing rolls the whole transaction back with
/// `run.required_missing` — the reconcile validates the tag node's own required fields two
/// levels down, not an ancestor's.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_deep_sparse_set_missing_the_required_field_rolls_back() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    assert_eq!(
        run_fault(
            &image,
            &mut attachment,
            "setTagHot",
            vec![Value::Int(4), s("n"), Value::Int(1), Value::Bool(true)],
        ),
        marrow_codes::Code::RunRequiredMissing.as_str(),
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagPresent",
            vec![Value::Int(4), s("n"), Value::Int(1)]
        ),
        present(false),
        "the rolled-back deep set persisted nothing",
    );
}

/// The four-state marker/target laws over a nested branch entry, read field-exact:
/// marker absent (both reads absent), marker present with the sparse absent (weight
/// present, hot absent), both present, and a whole replace that omits the sparse field
/// drops it — the payload-only replace law (adjudication 1) at depth.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_nested_branch_entry_upholds_the_four_state_laws() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    let key = || vec![Value::Int(5), s("n"), Value::Int(3)];

    assert_eq!(
        run(&image, &mut attachment, "tagPresent", key()),
        present(false)
    );
    assert_eq!(run(&image, &mut attachment, "tagWeight", key()), absent());
    assert_eq!(run(&image, &mut attachment, "tagHot", key()), absent());

    run(
        &image,
        &mut attachment,
        "addTag",
        vec![Value::Int(5), s("n"), Value::Int(3), Value::Int(8)],
    );
    assert_eq!(
        run(&image, &mut attachment, "tagWeight", key()),
        some_int(8)
    );
    assert_eq!(
        run(&image, &mut attachment, "tagHot", key()),
        absent(),
        "an omitted sparse field reads absent while the required field is present",
    );

    run(
        &image,
        &mut attachment,
        "addFullTag",
        vec![
            Value::Int(5),
            s("n"),
            Value::Int(3),
            Value::Int(8),
            Value::Bool(true),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "tagHot", key()),
        some_bool(true)
    );

    // A whole replace that omits the sparse field drops it (exact replacement).
    run(
        &image,
        &mut attachment,
        "addTag",
        vec![Value::Int(5), s("n"), Value::Int(3), Value::Int(8)],
    );
    assert_eq!(
        run(&image, &mut attachment, "tagHot", key()),
        absent(),
        "a whole replace omitting the sparse field drops it at depth",
    );
}

/// Adjudication 1 at depth: a whole-entry erase of a middle branch (`notes`) is
/// payload-only — it removes the note's marker and fields but preserves its keyed `tags`
/// descendants, and a whole-entry erase of the tag removes only that tag.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_middle_branch_erase_preserves_nested_descendants() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "addNote",
        vec![Value::Int(6), s("n"), s("body")],
    );
    run(
        &image,
        &mut attachment,
        "addTag",
        vec![Value::Int(6), s("n"), Value::Int(1), Value::Int(11)],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(6), s("n")]
        ),
        present(true)
    );

    // Erase the note payload: payload-only, so the nested tag survives.
    run(
        &image,
        &mut attachment,
        "eraseNote",
        vec![Value::Int(6), s("n")],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "notePresent",
            vec![Value::Int(6), s("n")]
        ),
        present(false),
        "the note payload is gone",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagWeight",
            vec![Value::Int(6), s("n"), Value::Int(1)]
        ),
        some_int(11),
        "a payload-only note erase preserves its nested tag descendant",
    );

    // Erase the tag: removes only the tag entry.
    run(
        &image,
        &mut attachment,
        "eraseTag",
        vec![Value::Int(6), s("n"), Value::Int(1)],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tagPresent",
            vec![Value::Int(6), s("n"), Value::Int(1)]
        ),
        present(false),
    );
}

/// A field-exact clear of the sparse `hot` on a nested tag leaves the required `weight`
/// intact — the field-exact clear is scoped to its own leaf two levels down.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_deep_field_exact_clear_preserves_the_required_field() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    let key = || vec![Value::Int(7), s("n"), Value::Int(2)];

    run(
        &image,
        &mut attachment,
        "addFullTag",
        vec![
            Value::Int(7),
            s("n"),
            Value::Int(2),
            Value::Int(3),
            Value::Bool(true),
        ],
    );
    run(
        &image,
        &mut attachment,
        "clearTagHot",
        vec![Value::Int(7), s("n"), Value::Int(2)],
    );
    assert_eq!(run(&image, &mut attachment, "tagHot", key()), absent());
    assert_eq!(
        run(&image, &mut attachment, "tagWeight", key()),
        some_int(3),
        "the field-exact clear left the required field intact",
    );
}

/// Adjudication 3: bounded traversal over the inner `tags` layer iterates the tag keys of
/// one fixed `[book, note]` ancestor path in ascending order, honors the `at most N`
/// bound with the `on more` bit, and is scoped to that note — a tag under a different note
/// is not visited.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn bounded_traversal_iterates_an_inner_branch_layer_under_a_fixed_ancestor_path() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // Three tags under (book 8, note "n"), and one under a sibling note "m".
    for tid in [3, 1, 2] {
        run(
            &image,
            &mut attachment,
            "addTag",
            vec![Value::Int(8), s("n"), Value::Int(tid), Value::Int(0)],
        );
    }
    run(
        &image,
        &mut attachment,
        "addTag",
        vec![Value::Int(8), s("m"), Value::Int(99), Value::Int(0)],
    );

    // Sum all tag keys under note "n": 1 + 2 + 3 = 6, no `on more`.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumTags",
            vec![Value::Int(8), s("n")]
        ),
        Some(Value::Int(6)),
        "the inner layer iterates its own note's tags in ascending order",
    );
    // Bounded at 2: freezes tags 1 and 2 (sum 3), a third existed → +1000.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumTagsBounded",
            vec![Value::Int(8), s("n")]
        ),
        Some(Value::Int(1003)),
        "the bound freezes the first two keys and the on-more bit fires",
    );
    // The sibling note "m" has exactly one tag (key 99); its layer is independent.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumTags",
            vec![Value::Int(8), s("m")]
        ),
        Some(Value::Int(99)),
        "the inner traversal is scoped to its ancestor path",
    );
}
