//! A named `place`/pin composes as a base for group-leaf and branch-entry operations
//! wherever the equivalent inline `^root…` path is admitted (DX06 item 1).
//!
//! Before DX06 a bound place composed only as a traversal base (DX02) and for a single
//! top-level field; a branch-entry write, a branch-field read, or a group-leaf operation
//! through a place was refused with a message that misnamed the failure ("no field",
//! "not in scope", "not yet supported"). These tests drive the whole production path
//! (capture -> compile -> verify -> attach -> VM) and prove parity with the inline form by
//! cross-reading: a write performed through a place is observed through the inline address
//! and the reverse, so the place-composed operation seals the *same* durable node — the
//! same operation site — the inline address does. A place bound over a branch entry is
//! itself a base for a deeper branch.

use marrow_verify::{SealedExport, SealedInstr, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.subtitle 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id field Book.details.language 22222222222222222222222222222222\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id field Book.notes.pinned 33333333333333333333333333333333\n\
     id root Book.notes.tags 34343434343434343434343434343434\n\
     id key Book.notes.tags.tagId 35353535353535353535353535353535\n\
     id field Book.notes.tags.weight 36363636363636363636363636363636\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Book {
    required title: string
    subtitle: string

    details {
        pages: int
        language: string
    }

    notes[noteId: string] {
        required text: string
        pinned: bool

        tags[tagId: int] {
            required weight: int
        }
    }
}

store ^books[id: int]: Book

pub fn putBook(id: int, t: string) {
    transaction {
        ^books[id] = Book(title: t)
    }
}

// --- branch entry: whole-entry write through a place, read inline ---

pub fn addNoteViaPlace(id: int, nid: string, t: string) {
    transaction {
        place b = ^books[id]
        b.notes[nid] = Book.notes(text: t)
    }
}

pub fn noteTextInline(id: int, nid: string): string? {
    return ^books[id].notes[nid].text
}

// --- branch entry: whole-entry write inline, read through a place ---

pub fn addNoteInline(id: int, nid: string, t: string) {
    transaction {
        ^books[id].notes[nid] = Book.notes(text: t)
    }
}

pub fn noteTextViaPlace(id: int, nid: string): string? {
    place b = ^books[id]
    return b.notes[nid].text
}

// --- branch field set through a place, read inline ---

pub fn setPinnedViaPlace(id: int, nid: string, v: bool) {
    transaction {
        place b = ^books[id]
        b.notes[nid].pinned = v
    }
}

pub fn pinnedInline(id: int, nid: string): bool? {
    return ^books[id].notes[nid].pinned
}

// --- whole branch entry read through a place with `if const` ---

pub fn noteTextGuardedViaPlace(id: int, nid: string): string {
    place b = ^books[id]
    if const note = b.notes[nid] {
        return note.text
    }
    return "none"
}

// --- branch entry deleted through a place ---

pub fn deleteNoteViaPlace(id: int, nid: string) {
    transaction {
        place b = ^books[id]
        delete b.notes[nid]
    }
}

// --- a place over a branch entry is a base for a deeper branch ---

pub fn addTagViaBranchPlace(id: int, nid: string, tid: int, w: int) {
    transaction {
        place note = ^books[id].notes[nid]
        note.tags[tid] = Book.notes.tags(weight: w)
    }
}

pub fn tagWeightInline(id: int, nid: string, tid: int): int? {
    return ^books[id].notes[nid].tags[tid].weight
}

// --- group leaf: set through a place, read inline ---

pub fn setPagesViaPlace(id: int, p: int) {
    transaction {
        place b = ^books[id]
        b.details.pages = p
    }
}

pub fn pagesInline(id: int): int? {
    return ^books[id].details.pages
}

// --- group leaf: set inline, read through a place ---

pub fn setPagesInline(id: int, p: int) {
    transaction {
        ^books[id].details.pages = p
    }
}

pub fn pagesViaPlace(id: int): int? {
    place b = ^books[id]
    return b.details.pages
}

// --- whole group read through a place with `if const` ---

pub fn pagesGuardedViaPlace(id: int): int {
    place b = ^books[id]
    if const d = b.details {
        if const p = d.pages {
            return p
        }
    }
    return -1
}

// --- group leaf cleared through a place ---

pub fn clearPagesViaPlace(id: int) {
    transaction {
        place b = ^books[id]
        delete b.details.pages
    }
}

// --- whole group replaced through a place ---

pub fn replaceDetailsViaPlace(id: int, p: int) {
    transaction {
        place b = ^books[id]
        b.details = Book.details(pages: p)
    }
}

// --- exists over a branch family named through a place (DX06 item 2) ---

pub fn hasNotesViaPlace(id: int): bool {
    place b = ^books[id]
    return exists(b.notes)
}

pub fn hasNotesInline(id: int): bool {
    return exists(^books[id].notes)
}

// --- a per-iteration pin's presence participates in the presence lattice (DX06 item 4) ---

pub fn touchNotesPinExists(id: int) {
    transaction {
        for nid, p in ^books[id].notes at most 10 {
            if exists(p) {
                p.pinned = true
            }
        } on more {}
    }
}

pub fn touchNotesPinConst(id: int) {
    transaction {
        for nid, p in ^books[id].notes at most 10 {
            if const note = p {
                p.pinned = true
            }
        } on more {}
    }
}

pub fn setPinnedGuardedViaPlace(id: int, nid: string, v: bool) {
    transaction {
        place b = ^books[id].notes[nid]
        if exists(b) {
            b.pinned = v
        }
    }
}
"#;

fn compile_verify() -> VerifiedImage {
    let project = common::Project::new()
        .source("src/main.mw", SOURCE)
        .ids(IDS);
    project.image()
}

mod common;

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
        Ephemeral::Parked => panic!("a flat root with simple branches must be executable"),
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

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

fn some_bool(v: bool) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Bool(v)))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

/// A composed access to a name that is neither a field nor a branch is refused with the
/// same real diagnostic the inline form gives — the branch/field is named, not misreported
/// as "not in scope" or "not yet supported". This is the "misnames the failure" fix: the
/// place form and the inline form now share one resolution owner and one message.
#[test]
fn a_bad_place_composition_reports_the_inline_diagnostic() {
    let program = |access: &str| -> String {
        format!(
            "{SOURCE}\npub fn probe(id: int, nid: string): int? {{\n    place b = ^books[id]\n    return {access}\n}}\n"
        )
    };
    // An unknown branch beneath a place names the real situation (a missing keyed branch),
    // the same message the inline `^books[id].bogus[nid]` form produces.
    let diags = common::Project::new()
        .source("src/main.mw", &program("b.bogus[nid].weight"))
        .ids(IDS)
        .try_image()
        .expect_err("an unknown branch is refused");
    let message = diags.only("check.type").message.clone();
    assert!(
        message.contains("bogus") && message.contains("keyed branch"),
        "the place-composed refusal names the missing branch: {message:?}",
    );

    // The inline sibling produces the identical message, proving one shared owner.
    let inline = common::Project::new()
        .source(
            "src/main.mw",
            &format!(
                "{SOURCE}\npub fn probe(id: int, nid: string): int? {{\n    return ^books[id].bogus[nid].weight\n}}\n"
            ),
        )
        .ids(IDS)
        .try_image()
        .expect_err("an unknown inline branch is refused");
    assert_eq!(
        inline.only("check.type").message,
        message,
        "the place and inline forms share one branch-resolution message",
    );
}

/// A whole branch entry written through a place is addressed exactly where the inline
/// form addresses it: an inline read observes the place-written note, and a place read
/// observes the inline-written note.
#[test]
fn branch_entry_write_through_a_place_addresses_the_inline_node() {
    let image = compile_verify();
    let mut a = attach(&image);
    run(
        &image,
        &mut a,
        "putBook",
        vec![Value::Int(1), Value::Text("b".into())],
    );

    run(
        &image,
        &mut a,
        "addNoteViaPlace",
        vec![
            Value::Int(1),
            Value::Text("n1".into()),
            Value::Text("hello".into()),
        ],
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "noteTextInline",
            vec![Value::Int(1), Value::Text("n1".into())]
        ),
        some_text("hello"),
    );

    run(
        &image,
        &mut a,
        "addNoteInline",
        vec![
            Value::Int(1),
            Value::Text("n2".into()),
            Value::Text("world".into()),
        ],
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "noteTextViaPlace",
            vec![Value::Int(1), Value::Text("n2".into())]
        ),
        some_text("world"),
    );
}

/// A branch-field set and a guarded whole-entry read through a place hit the inline node.
#[test]
fn branch_field_and_guarded_read_through_a_place_match_inline() {
    let image = compile_verify();
    let mut a = attach(&image);
    run(
        &image,
        &mut a,
        "putBook",
        vec![Value::Int(1), Value::Text("b".into())],
    );
    run(
        &image,
        &mut a,
        "addNoteInline",
        vec![
            Value::Int(1),
            Value::Text("n1".into()),
            Value::Text("hi".into()),
        ],
    );

    run(
        &image,
        &mut a,
        "setPinnedViaPlace",
        vec![Value::Int(1), Value::Text("n1".into()), Value::Bool(true)],
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "pinnedInline",
            vec![Value::Int(1), Value::Text("n1".into())]
        ),
        some_bool(true),
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "noteTextGuardedViaPlace",
            vec![Value::Int(1), Value::Text("n1".into())]
        ),
        Some(Value::Text("hi".into())),
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "noteTextGuardedViaPlace",
            vec![Value::Int(1), Value::Text("absent".into())]
        ),
        Some(Value::Text("none".into())),
    );
}

/// A place bound over a branch entry composes a deeper branch: the tag written through
/// the branch place is read at the inline four-level address.
#[test]
fn a_branch_place_composes_a_deeper_branch() {
    let image = compile_verify();
    let mut a = attach(&image);
    run(
        &image,
        &mut a,
        "putBook",
        vec![Value::Int(1), Value::Text("b".into())],
    );
    run(
        &image,
        &mut a,
        "addNoteInline",
        vec![
            Value::Int(1),
            Value::Text("n1".into()),
            Value::Text("hi".into()),
        ],
    );
    run(
        &image,
        &mut a,
        "addTagViaBranchPlace",
        vec![
            Value::Int(1),
            Value::Text("n1".into()),
            Value::Int(7),
            Value::Int(3),
        ],
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "tagWeightInline",
            vec![Value::Int(1), Value::Text("n1".into()), Value::Int(7)]
        ),
        some_int(3),
    );
}

/// A branch entry deleted through a place is gone at its inline address.
#[test]
fn a_branch_entry_deleted_through_a_place_is_gone_inline() {
    let image = compile_verify();
    let mut a = attach(&image);
    run(
        &image,
        &mut a,
        "putBook",
        vec![Value::Int(1), Value::Text("b".into())],
    );
    run(
        &image,
        &mut a,
        "addNoteInline",
        vec![
            Value::Int(1),
            Value::Text("n1".into()),
            Value::Text("hi".into()),
        ],
    );
    run(
        &image,
        &mut a,
        "deleteNoteViaPlace",
        vec![Value::Int(1), Value::Text("n1".into())],
    );
    assert_eq!(
        run(
            &image,
            &mut a,
            "noteTextInline",
            vec![Value::Int(1), Value::Text("n1".into())]
        ),
        absent(),
    );
}

/// A group leaf set, read, cleared, and whole-group replaced through a place all address
/// the same group cells the inline form does.
#[test]
fn group_leaf_operations_through_a_place_match_inline() {
    let image = compile_verify();
    let mut a = attach(&image);
    run(
        &image,
        &mut a,
        "putBook",
        vec![Value::Int(1), Value::Text("b".into())],
    );

    run(
        &image,
        &mut a,
        "setPagesViaPlace",
        vec![Value::Int(1), Value::Int(42)],
    );
    assert_eq!(
        run(&image, &mut a, "pagesInline", vec![Value::Int(1)]),
        some_int(42)
    );

    run(
        &image,
        &mut a,
        "setPagesInline",
        vec![Value::Int(1), Value::Int(7)],
    );
    assert_eq!(
        run(&image, &mut a, "pagesViaPlace", vec![Value::Int(1)]),
        some_int(7)
    );
    assert_eq!(
        run(&image, &mut a, "pagesGuardedViaPlace", vec![Value::Int(1)]),
        Some(Value::Int(7))
    );

    run(&image, &mut a, "clearPagesViaPlace", vec![Value::Int(1)]);
    assert_eq!(
        run(&image, &mut a, "pagesInline", vec![Value::Int(1)]),
        absent()
    );

    run(
        &image,
        &mut a,
        "replaceDetailsViaPlace",
        vec![Value::Int(1), Value::Int(9)],
    );
    assert_eq!(
        run(&image, &mut a, "pagesInline", vec![Value::Int(1)]),
        some_int(9)
    );
}

/// `exists(place.branch)` is the family-populated probe, not a missing-field error: it
/// answers whether the branch family beneath the place-addressed entry has any child,
/// matching the inline `exists(^root[key].branch)` form and its `DurFamilyExists` site.
#[test]
fn exists_over_a_branch_family_named_through_a_place_matches_inline() {
    let image = compile_verify();
    let mut a = attach(&image);
    run(
        &image,
        &mut a,
        "putBook",
        vec![Value::Int(1), Value::Text("b".into())],
    );

    assert_eq!(
        run(&image, &mut a, "hasNotesViaPlace", vec![Value::Int(1)]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run(&image, &mut a, "hasNotesInline", vec![Value::Int(1)]),
        Some(Value::Bool(false))
    );

    run(
        &image,
        &mut a,
        "addNoteInline",
        vec![
            Value::Int(1),
            Value::Text("n1".into()),
            Value::Text("hi".into()),
        ],
    );
    assert_eq!(
        run(&image, &mut a, "hasNotesViaPlace", vec![Value::Int(1)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(&image, &mut a, "hasNotesInline", vec![Value::Int(1)]),
        Some(Value::Bool(true))
    );

    let family_sites = |name: &str| -> usize {
        image
            .functions()
            .iter()
            .find(|f| f.name() == name)
            .expect("function present")
            .instrs()
            .iter()
            .filter(|instr| matches!(instr, SealedInstr::DurFamilyExists(_)))
            .count()
    };
    assert_eq!(
        family_sites("hasNotesViaPlace"),
        1,
        "the place-base exists lowers to the family-populated probe, not a cell probe",
    );
    assert_eq!(family_sites("hasNotesInline"), 1);
}

/// A per-iteration pin's presence participates in the presence lattice exactly where the
/// equivalent branch `place` binding's does: a sparse-field set guarded by `exists(p)` or
/// by an `if const` binding of the pin lowers to the strict present-entry form
/// (`DurSetSparsePresent`), never the bare create-or-reconcile `DurSetSparse`. This pins
/// the parity DX06 item 4 verified as already-held, so a regression is conspicuous.
#[test]
fn a_pin_guarded_sparse_set_lowers_strict_at_parity_with_a_place() {
    let image = compile_verify();
    let counts = |name: &str| -> (usize, usize) {
        let instrs = image
            .functions()
            .iter()
            .find(|f| f.name() == name)
            .expect("function present")
            .instrs();
        let strict = instrs
            .iter()
            .filter(|i| matches!(i, SealedInstr::DurSetSparsePresent { .. }))
            .count();
        let bare = instrs
            .iter()
            .filter(|i| matches!(i, SealedInstr::DurSetSparse(_)))
            .count();
        (strict, bare)
    };
    for name in [
        "touchNotesPinExists",
        "touchNotesPinConst",
        "setPinnedGuardedViaPlace",
    ] {
        assert_eq!(
            counts(name),
            (1, 0),
            "`{name}` lowers the guarded branch set strict, with no bare create-or-reconcile set",
        );
    }
}

/// The place-composed and inline forms of one operation seal the identical durable site:
/// the whole-entry branch write lowers to the same `DurCreateEntry`/`DurReplaceEntry`
/// site set whether the parent is a place or an inline address.
#[test]
fn place_composed_and_inline_branch_writes_seal_the_same_site() {
    let image = compile_verify();
    let sites = |name: &str| -> Vec<u16> {
        image
            .functions()
            .iter()
            .find(|f| f.name() == name)
            .expect("function present")
            .instrs()
            .iter()
            .filter_map(|instr| match instr {
                SealedInstr::DurCreateEntry(site) | SealedInstr::DurReplaceEntry(site) => {
                    Some(*site)
                }
                _ => None,
            })
            .collect()
    };
    let via_place = sites("addNoteViaPlace");
    let inline = sites("addNoteInline");
    assert!(
        !inline.is_empty(),
        "the inline branch write emits entry sites"
    );
    assert_eq!(
        via_place, inline,
        "the place-composed branch write seals the same durable node site as the inline form",
    );
}
