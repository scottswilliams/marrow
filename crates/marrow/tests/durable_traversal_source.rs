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
     \x20   return total\n\
     \n\
     pub fn continueOnSecond(): int\n\
     \x20   var total = 0\n\
     \x20   for k in ^books at most 2\n\
     \x20       if k == 2\n\
     \x20           continue\n\
     \x20       total += k\n\
     \x20   on more\n\
     \x20       total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn returnOnSecond(): int\n\
     \x20   for k in ^books at most 2\n\
     \x20       if k == 2\n\
     \x20           return k\n\
     \x20   on more\n\
     \x20       return -1\n\
     \x20   return 0\n\
     \n\
     pub fn faultOnSecond(): int\n\
     \x20   for k in ^books at most 2\n\
     \x20       if k == 2\n\
     \x20           unreachable(\"boom\")\n\
     \x20   on more\n\
     \x20       return -1\n\
     \x20   return 0\n\
     \n\
     pub fn nestedNotes(): int\n\
     \x20   var total = 0\n\
     \x20   for id in ^books at most 100\n\
     \x20       for pos in ^books(id).notes at most 2\n\
     \x20           total += pos\n\
     \x20       on more\n\
     \x20           total = total + 100\n\
     \x20   on more\n\
     \x20       total = total + 100000\n\
     \x20   return total\n\
     \n\
     pub fn eraseWhileTraversing(): int\n\
     \x20   var total = 0\n\
     \x20   transaction\n\
     \x20       for k in ^books at most 100\n\
     \x20           total += k\n\
     \x20           delete ^books(k)\n\
     \x20       on more\n\
     \x20           total = total + 1000\n\
     \x20   return total\n\
     \n\
     pub fn createWhileTraversing(): int\n\
     \x20   var total = 0\n\
     \x20   transaction\n\
     \x20       for k in ^books at most 2\n\
     \x20           total += k\n\
     \x20           ^books(k + 100) = Book(title: \"x\")\n\
     \x20       on more\n\
     \x20           total = total + 1000\n\
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

/// Run a read-only export and return the dotted code of the runtime fault it raises.
fn run_fault(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> String {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Err(fault)) => fault.code().to_string(),
        other => panic!("{name} did not fault: {:?}", DebugRun(&other)),
    }
}

struct DebugRun<'a>(&'a DurableRun);
impl std::fmt::Debug for DebugRun<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DurableRun::Ran(Ok(value)) => write!(f, "Ran(Ok({value:?}))"),
            DurableRun::Ran(Err(fault)) => write!(f, "Ran(Err({}))", fault.code()),
            DurableRun::Parked => write!(f, "Parked"),
            DurableRun::Failed(code) => write!(f, "Failed({code})"),
        }
    }
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

#[test]
fn the_population_boundary_decides_the_on_more_arm() {
    // `sumFirst2` is `at most 2` over `^books`, adding 1000 in `on more`. Growing the
    // population one entry at a time walks the 0 / 1 / N / N+1 boundary.
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // 0 entries: no keys, no `on more`.
    assert_eq!(
        run(&image, &mut attachment, "sumFirst2", vec![]),
        Some(Value::Int(0))
    );
    // 1 entry (< N): the one key, still no further key.
    run(
        &image,
        &mut attachment,
        "put",
        vec![Value::Int(1), Value::Text("t".into())],
    );
    assert_eq!(
        run(&image, &mut attachment, "sumFirst2", vec![]),
        Some(Value::Int(1))
    );
    // 2 entries (= N): both frozen, no (N+1)th key, so `on more` does not run.
    run(
        &image,
        &mut attachment,
        "put",
        vec![Value::Int(2), Value::Text("t".into())],
    );
    assert_eq!(
        run(&image, &mut attachment, "sumFirst2", vec![]),
        Some(Value::Int(3))
    );
    // 3 entries (N+1): frozen [1,2], a third existed, so `on more` adds 1000.
    run(
        &image,
        &mut attachment,
        "put",
        vec![Value::Int(3), Value::Text("t".into())],
    );
    assert_eq!(
        run(&image, &mut attachment, "sumFirst2", vec![]),
        Some(Value::Int(1003))
    );
}

#[test]
fn every_abnormal_body_exit_decides_the_on_more_timing() {
    // Over books {1,2,3} with `at most 2`, a further key (3) always existed at freeze.
    // `on more` runs iff the frozen bodies all completed normally.
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);

    // `continue` completes the loop normally, so `on more` still runs: k=1 adds 1,
    // k=2 continues, then `on more` adds 1000.
    assert_eq!(
        run(&image, &mut attachment, "continueOnSecond", vec![]),
        Some(Value::Int(1001))
    );
    // `return` from a body leaves without running `on more`, even though a third key
    // existed: it returns the key 2 directly.
    assert_eq!(
        run(&image, &mut attachment, "returnOnSecond", vec![]),
        Some(Value::Int(2))
    );
    // A fault in a body aborts the whole traversal; `on more` is never reached.
    assert_eq!(
        run_fault(&image, &mut attachment, "faultOnSecond", vec![]),
        "run.unreachable"
    );
}

#[test]
fn nested_root_and_branch_traversals_each_carry_their_own_on_more() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);
    // Book 1 carries three notes; the inner `at most 2` freezes two and its `on more`
    // fires. Books 2 and 3 carry none.
    for pos in [10i64, 20, 30] {
        run(
            &image,
            &mut attachment,
            "putNote",
            vec![Value::Int(1), Value::Int(pos), Value::Text("n".into())],
        );
    }

    // Inner over book 1: frozen [10,20] (sum 30) + inner `on more` 100 = 130. Books 2
    // and 3 add nothing (empty inner layers, no inner `on more`). The outer layer has
    // exactly three books, so the outer `on more` does not run.
    assert_eq!(
        run(&image, &mut attachment, "nestedNotes", vec![]),
        Some(Value::Int(130))
    );
}

#[test]
fn the_frozen_key_set_is_immune_to_writes_the_bodies_perform() {
    // A body that erases every entry it visits still visits all three frozen keys —
    // the frozen set is captured before any body runs, so the erases cannot cut the
    // traversal short. `at most 100`, so no `on more`.
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);
    assert_eq!(
        run(&image, &mut attachment, "eraseWhileTraversing", vec![]),
        Some(Value::Int(6)),
    );
    // The erases committed: a re-run over the now-empty store visits nothing.
    assert_eq!(
        run(&image, &mut attachment, "eraseWhileTraversing", vec![]),
        Some(Value::Int(0)),
    );
}

#[test]
fn the_on_more_decision_is_immune_to_entries_a_body_creates() {
    // A body that creates new entries beyond the frozen bound does not change the
    // `on more` decision: it was fixed at freeze. `at most 2` over {1,2,3} freezes
    // [1,2]; a third key existed at freeze, so `on more` adds 1000 (= 1003) regardless
    // of the two new books the bodies create.
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    seed_books(&image, &mut attachment);
    assert_eq!(
        run(&image, &mut attachment, "createWhileTraversing", vec![]),
        Some(Value::Int(1003)),
    );
}

#[test]
fn a_descendant_only_child_is_skipped_without_visiting_its_subtree() {
    // Books 1 and 3 have payloads; book 2 has only notes (a descendant-only root, no
    // title marker). A root traversal freezes only the payload-bearing books [1,3], so
    // the descendant-only book 2 and its whole note subtree are skipped: `sumAll` is
    // 1 + 3 = 4, never touching book 2's descendants. The O(1)-seek bound over a large
    // fan-out is proven at the kernel tier.
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    run(
        &image,
        &mut attachment,
        "put",
        vec![Value::Int(1), Value::Text("t".into())],
    );
    run(
        &image,
        &mut attachment,
        "put",
        vec![Value::Int(3), Value::Text("t".into())],
    );
    // Book 2 gets a fan-out of notes but no payload of its own.
    for pos in 0..20i64 {
        run(
            &image,
            &mut attachment,
            "putNote",
            vec![Value::Int(2), Value::Int(pos), Value::Text("n".into())],
        );
    }
    assert_eq!(
        run(&image, &mut attachment, "sumAll", vec![]),
        Some(Value::Int(4)),
    );
}
