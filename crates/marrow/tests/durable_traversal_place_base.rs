//! A named `place` or a per-iteration pin is a durable traversal base (DX02).
//!
//! `for k[, p] in <place>.branch at most N on more` traverses the keyed branch family
//! beneath the entry a `place`/pin already addresses, exactly as an inline
//! `^root[key].branch` base does. The place's key-path — evaluated once at its binding —
//! is the traversal's ancestor key-path; the branch adds its own immediate key. These
//! tests drive the whole production path (capture -> compile -> verify -> attach -> VM)
//! over one persistent ephemeral attachment: a simple root place, a per-iteration pin used
//! as an inner base, a two-binding traversal that deletes through the pin, the `on more`
//! overflow arm through a place base, and a composite-root place whose whole two-column
//! key-path locates the branch. A place bound through an entry identity is refused as a
//! traversal base (its key-path carries a typed identity column the traversal ancestor pop
//! does not yet accept); that refusal is pinned here too.

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.pos 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id product Enroll 41414141414141414141414141414141\n\
     id field Enroll.term 42424242424242424242424242424242\n\
     id root grades 43434343434343434343434343434343\n\
     id key grades.student 44444444444444444444444444444444\n\
     id key grades.course 45454545454545454545454545454545\n\
     id root Enroll.marks 46464646464646464646464646464646\n\
     id key Enroll.marks.slot 47474747474747474747474747474747\n\
     id field Enroll.marks.value 48484848484848484848484848484848\n\
     high-water 0\n\
     end\n";

/// A `Book { title }` root with a single-level `notes(pos: int)` branch and a composite
/// `^grades[student, course]` root whose `Enroll` carries a `marks(slot: int)` branch.
/// The `sum*ViaPlace` exports bind a `place`/pin over an entry and traverse a branch
/// beneath it, folding the visited keys and adding 1000 in `on more`, so one returned
/// int witnesses which keys were frozen and whether `on more` ran.
const SOURCE: &str = r#"resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

resource Enroll {
    required term: string

    marks[slot: int] {
        required value: int
    }
}

store ^grades[student: string, course: string]: Enroll

pub fn putBook(id: int, t: string) {
    transaction {
        ^books[id] = Book(title: t)
    }
}

pub fn putNote(id: int, pos: int, t: string) {
    transaction {
        ^books[id].notes[pos] = Book.notes(text: t)
    }
}

pub fn putGrade(s: string, c: string, t: string) {
    transaction {
        ^grades[s, c] = Enroll(term: t)
    }
}

pub fn putMark(s: string, c: string, slot: int, v: int) {
    transaction {
        ^grades[s, c].marks[slot] = Enroll.marks(value: v)
    }
}

pub fn sumNotesViaPlace(id: int): int {
    var total = 0
    place b = ^books[id]
    for pos in b.notes at most 100 {
        total += pos
    } on more {
        total = total + 1000
    }
    return total
}

pub fn sumNotesViaPlaceFirst2(id: int): int {
    var total = 0
    place b = ^books[id]
    for pos in b.notes at most 2 {
        total += pos
    } on more {
        total = total + 1000
    }
    return total
}

pub fn clearNotesViaPlace(id: int): int {
    var total = 0
    transaction {
        place b = ^books[id]
        for pos, note in b.notes at most 100 {
            total += pos
            delete note
        } on more {
            total = total + 1000
        }
    }
    return total
}

pub fn sumAllNotesViaPin(): int {
    var total = 0
    for id, book in ^books at most 100 {
        for pos in book.notes at most 100 {
            total += pos
        } on more {
            total = total + 1000
        }
    } on more {
        total = total + 100000
    }
    return total
}

pub fn sumMarksViaPlace(s: string, c: string): int {
    var total = 0
    place g = ^grades[s, c]
    for slot in g.marks at most 100 {
        total += slot
    } on more {
        total = total + 1000
    }
    return total
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

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a flat root with a simple branch must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn seed_notes(image: &VerifiedImage, attachment: &mut marrow_kernel::durable::EphemeralAttachment) {
    for id in [1i64, 2, 3] {
        run(
            image,
            attachment,
            "putBook",
            vec![Value::Int(id), Value::Text("t".into())],
        );
    }
    for pos in [10i64, 20] {
        run(
            image,
            attachment,
            "putNote",
            vec![Value::Int(1), Value::Int(pos), Value::Text("n".into())],
        );
    }
}

#[test]
fn a_root_place_is_a_branch_traversal_base() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_notes(&image, &mut attachment);

    // `place b = ^books[1]; for pos in b.notes` folds book 1's notes {10,20} = 30; the
    // layer is exhausted, so `on more` does not run.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumNotesViaPlace",
            vec![Value::Int(1)]
        ),
        Some(Value::Int(30))
    );
    // Book 2 has no notes: the branch under the place is empty.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumNotesViaPlace",
            vec![Value::Int(2)]
        ),
        Some(Value::Int(0))
    );
}

#[test]
fn a_place_base_carries_the_on_more_overflow_arm() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    run(
        &image,
        &mut attachment,
        "putBook",
        vec![Value::Int(1), Value::Text("t".into())],
    );
    for pos in [10i64, 20, 30] {
        run(
            &image,
            &mut attachment,
            "putNote",
            vec![Value::Int(1), Value::Int(pos), Value::Text("n".into())],
        );
    }

    // `at most 2` over three notes freezes {10,20} = 30, and a third key existed so the
    // `on more` arm through the place base adds 1000.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumNotesViaPlaceFirst2",
            vec![Value::Int(1)]
        ),
        Some(Value::Int(1030))
    );
}

#[test]
fn an_identity_keyed_place_base_is_refused_until_the_verifier_accepts_its_column() {
    // A place bound through an entry identity carries its root as a typed identity column,
    // which the bounded-traversal ancestor pop does not yet accept. Admitting it would break
    // the checker-accept ⇒ verify-accept agreement law, so the checker refuses it truthfully
    // with `check.unsupported` at the traversed branch. (The verifier acceptance that would
    // make this a round trip is a separate soundness-critical lane; see the lane report.)
    let source = format!(
        "{SCHEMA_ONLY}\n{}",
        "pub fn sumNotesViaIdPlace(id: int): int {\n    var total = 0\n    place b = ^books[Id(^books, id)]\n    for pos in b.notes at most 100 {\n        total += pos\n    } on more {\n        total = total + 1000\n    }\n    return total\n}\n"
    );
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
    let diagnostics = match marrow_compile::compile(&project) {
        Ok(_) => panic!("an identity-keyed place base must be refused"),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected diagnostics, got {other:?}"),
    };
    let hit = diagnostics
        .iter()
        .find(|d| d.code == "check.unsupported")
        .unwrap_or_else(|| {
            panic!(
                "expected check.unsupported, got {:?}",
                diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
            )
        });
    assert!(
        hit.line() >= 1 && hit.column() >= 1,
        "the refusal carries a located span: {hit:?}"
    );
}

/// The books-only schema, for the identity-keyed refusal fixture above.
const SCHEMA_ONLY: &str = r#"resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book"#;

#[test]
fn a_per_iteration_pin_is_an_inner_traversal_base() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_notes(&image, &mut attachment);
    for pos in [40i64, 50] {
        run(
            &image,
            &mut attachment,
            "putNote",
            vec![Value::Int(2), Value::Int(pos), Value::Text("n".into())],
        );
    }

    // `for id, book in ^books { for pos in book.notes … }`: the outer pin `book` is the
    // inner traversal base. Book 1 notes {10,20}=30, book 2 notes {40,50}=90, book 3 none;
    // total 120, no inner or outer `on more`.
    assert_eq!(
        run(&image, &mut attachment, "sumAllNotesViaPin", vec![]),
        Some(Value::Int(120))
    );
}

#[test]
fn a_two_binding_place_base_deletes_through_the_pin() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_notes(&image, &mut attachment);

    // `place b = ^books[1]; for pos, note in b.notes { delete note }`: the pin's key-path
    // is the place's captured root slot followed by each frozen note key. It visits both
    // notes (sum 30) and erases them; `at most 100`, so no `on more`.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "clearNotesViaPlace",
            vec![Value::Int(1)]
        ),
        Some(Value::Int(30))
    );
    // The deletes committed: a re-run visits nothing.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "clearNotesViaPlace",
            vec![Value::Int(1)]
        ),
        Some(Value::Int(0))
    );
    // The reading place traversal agrees the notes are gone.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumNotesViaPlace",
            vec![Value::Int(1)]
        ),
        Some(Value::Int(0))
    );
}

#[test]
fn a_composite_root_place_locates_a_branch_by_its_whole_key_path() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    run(
        &image,
        &mut attachment,
        "putGrade",
        vec![
            Value::Text("amy".into()),
            Value::Text("cs".into()),
            Value::Text("fall".into()),
        ],
    );
    for slot in [3i64, 4] {
        run(
            &image,
            &mut attachment,
            "putMark",
            vec![
                Value::Text("amy".into()),
                Value::Text("cs".into()),
                Value::Int(slot),
                Value::Int(0),
            ],
        );
    }
    // A different (student, course) carries its own mark that must not leak into amy/cs.
    run(
        &image,
        &mut attachment,
        "putGrade",
        vec![
            Value::Text("bob".into()),
            Value::Text("cs".into()),
            Value::Text("fall".into()),
        ],
    );
    run(
        &image,
        &mut attachment,
        "putMark",
        vec![
            Value::Text("bob".into()),
            Value::Text("cs".into()),
            Value::Int(9),
            Value::Int(0),
        ],
    );

    // `place g = ^grades[amy, cs]; for slot in g.marks`: both composite key columns are
    // captured at the binding and locate the branch under amy/cs — marks {3,4}=7, scoped to
    // that parent, never bob's slot 9.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumMarksViaPlace",
            vec![Value::Text("amy".into()), Value::Text("cs".into())]
        ),
        Some(Value::Int(7))
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumMarksViaPlace",
            vec![Value::Text("bob".into()), Value::Text("cs".into())]
        ),
        Some(Value::Int(9))
    );
}
