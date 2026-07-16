//! Bounded nested `for` traversal, executed end to end from source (E04).
//!
//! `for k in ^root at most N [from f] on more` freezes the first `N` immediate keys of
//! a durable root or single-level branch family, runs the body once per frozen key in
//! ascending order, and runs the `on more` block when an `(N+1)`th key existed and the
//! frozen bodies all completed normally. These tests drive the whole production path —
//! capture -> compile -> verify -> attach -> VM — over one persistent ephemeral
//! attachment, seeding through ordinary writes and reading the traversal back.

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
     high-water 0\n\
     end\n";

/// A `Book { title }` root with a single-level `notes(pos: int)` branch. `put`/`putNote`
/// seed entries; the `sum*` exports traverse and fold the visited keys, adding 1000 in
/// the `on more` block so one returned int witnesses both which keys were frozen (their
/// sum, in order) and whether `on more` ran.
const SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \n\
     \x20   notes(pos: int)\n\
     \x20       required text: string\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn put(id: int, t: string)\n\
     \x20   transaction\n\
     \x20       ^books(id) = Book(title: t)\n\
     \n\
     pub fn putNote(id: int, pos: int, t: string)\n\
     \x20   transaction\n\
     \x20       ^books(id).notes(pos) = Book.notes(text: t)\n\
     \n\
     pub fn sumFirst2(): int\n\
     \x20   var total = 0\n\
     \x20   for k in ^books at most 2\n\
     \x20       total += k\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn sumAll(): int\n\
     \x20   var total = 0\n\
     \x20   for k in ^books at most 100\n\
     \x20       total += k\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn sumFrom(f: int): int\n\
     \x20   var total = 0\n\
     \x20   for k in ^books at most 100 from f\n\
     \x20       total += k\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn sumNotes(id: int): int\n\
     \x20   var total = 0\n\
     \x20   for p in ^books(id).notes at most 100\n\
     \x20       total += p\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn breakAfterFirst(): int\n\
     \x20   var total = 0\n\
     \x20   for k in ^books at most 2\n\
     \x20       total += k\n\
     \x20       break\n\
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
        Ephemeral::Ready(attachment) => attachment,
        Ephemeral::Parked => panic!("a flat root with a simple branch must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn seed_books(image: &VerifiedImage, attachment: &mut marrow_kernel::durable::EphemeralAttachment) {
    for id in [1i64, 2, 3] {
        run(
            image,
            attachment,
            "put",
            vec![Value::Int(id), Value::Text("t".into())],
        );
    }
}

#[test]
fn a_root_traversal_folds_frozen_keys_in_order_and_runs_on_more() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);

    // `at most 2` over books {1,2,3}: frozen [1,2] (sum 3), a third existed so `on more`
    // adds 1000.
    assert_eq!(
        run(&image, &mut attachment, "sumFirst2", vec![]),
        Some(Value::Int(1003))
    );
    // `at most 100`: all three frozen (sum 6), no further key so `on more` does not run.
    assert_eq!(
        run(&image, &mut attachment, "sumAll", vec![]),
        Some(Value::Int(6))
    );
}

#[test]
fn a_root_traversal_from_seeks_inclusive() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);

    // `from 2` over {1,2,3}: frozen [2,3] (sum 5), exhausted so no `on more`.
    assert_eq!(
        run(&image, &mut attachment, "sumFrom", vec![Value::Int(2)]),
        Some(Value::Int(5))
    );
    // `from 4` past the last key: no keys, no `on more`.
    assert_eq!(
        run(&image, &mut attachment, "sumFrom", vec![Value::Int(4)]),
        Some(Value::Int(0))
    );
}

#[test]
fn a_branch_traversal_scopes_to_its_parent_entry() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);
    for pos in [10i64, 20] {
        run(
            &image,
            &mut attachment,
            "putNote",
            vec![Value::Int(1), Value::Int(pos), Value::Text("n".into())],
        );
    }

    // Book 1 has notes at {10, 20}: frozen sum 30, exhausted so no `on more`.
    assert_eq!(
        run(&image, &mut attachment, "sumNotes", vec![Value::Int(1)]),
        Some(Value::Int(30))
    );
    // Book 2 has no notes: an empty layer yields no keys and no `on more`.
    assert_eq!(
        run(&image, &mut attachment, "sumNotes", vec![Value::Int(2)]),
        Some(Value::Int(0))
    );
}

#[test]
fn a_body_break_skips_the_on_more_block() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);

    // The body breaks on the first key: total is 1 and `on more` does not run even
    // though a further key existed beyond the frozen two.
    assert_eq!(
        run(&image, &mut attachment, "breakAfterFirst", vec![]),
        Some(Value::Int(1))
    );
}
