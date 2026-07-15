//! E02: the lexical transaction region and exact mutations, executed end to end.
//!
//! A mutating export owns exactly one `transaction` region; its staged writes are
//! published as a unit only when the region commits. These tests drive the whole
//! production path — capture -> compile -> verify -> attach -> VM — against a single
//! *persistent* ephemeral attachment, so a later read invocation observes the
//! committed effect of an earlier mutating one. That persistence is what makes the
//! transaction region observable: a committed transaction is visible afterward, a
//! rolled-back one is not, and a required field left unset at commit rolls the whole
//! region back rather than publishing a partial entry.
//!
//! `marrow run` still parks a durable export in the trough (its in-process store open
//! returns at F02b), so the transaction region has no CLI execution path yet; the
//! ephemeral attachment is its production runtime, and these tests drive it directly.

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A counter store with one mutating export per operation and read-only observers.
/// Every mutation sits inside the export's single `transaction` region.
const SOURCE: &str = "resource Counter\n\
     \x20   required value: int\n\
     \x20   label: string\n\
     \n\
     store ^counters(id: int): Counter\n\
     \n\
     pub fn set(id: int, v: int)\n\
     \x20   transaction\n\
     \x20       ^counters(id) = Counter(value: v)\n\
     \n\
     pub fn setLabel(id: int, text: string)\n\
     \x20   transaction\n\
     \x20       ^counters(id).label = text\n\
     \n\
     pub fn eraseEntry(id: int)\n\
     \x20   transaction\n\
     \x20       delete ^counters(id)\n\
     \n\
     pub fn labelOnly(id: int, text: string)\n\
     \x20   transaction\n\
     \x20       ^counters(id).label = text\n\
     \n\
     pub fn setThenOverflow(id: int, big: int)\n\
     \x20   transaction\n\
     \x20       ^counters(id) = Counter(value: 1)\n\
     \x20       ^counters(id).value = big + big\n\
     \n\
     pub fn setThenMaybeDiverge(id: int, v: int, boom: bool)\n\
     \x20   transaction\n\
     \x20       ^counters(id) = Counter(value: v)\n\
     \x20       if boom\n\
     \x20           unreachable(\"the invariant broke mid-transaction\")\n\
     \n\
     pub fn getValue(id: int): int?\n\
     \x20   return ^counters(id).value\n\
     \n\
     pub fn getLabel(id: int): string?\n\
     \x20   return ^counters(id).label\n";

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

/// The verifier rejection code for a source that compiles but fails verification,
/// or `None` if it verifies.
fn verify_rejection(source: &str) -> Option<String> {
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
    marrow_verify::verify(&compiled.image.bytes)
        .err()
        .map(|rejection| rejection.code().to_string())
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

/// Run `name(args)` against `attachment`, returning its VM value (a `run` fault
/// panics — a fault case uses [`run_faulting`] instead).
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

/// Run `name(args)` expecting a source-mapped runtime fault, returning its code.
fn run_faulting(
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

/// A `DurableRun` is not `Debug`; this renders just enough for a panic message.
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
        Ephemeral::Parked => panic!("the flat counter image must be executable, not parked"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

/// A committed transaction is observable by a later read invocation on the same
/// attachment: `set` commits its one region, and a subsequent `getValue` reads the
/// committed value back.
#[test]
fn a_committed_transaction_is_observable_by_a_later_read() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // Before any write the store is empty.
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(None))
    );

    // A mutating export commits its transaction; the effect persists past the session.
    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(1), Value::Int(5)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Int(5)))))
    );
}

/// A sparse field committed in its own transaction reads back; a second transaction
/// replacing the whole entry drops the earlier sparse leaf (exact replacement).
#[test]
fn a_committed_field_write_reads_back_and_replacement_is_exact() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(2), Value::Int(1)],
    );
    run(
        &image,
        &mut attachment,
        "setLabel",
        vec![Value::Int(2), Value::Text("tag".into())],
    );
    assert_eq!(
        run(&image, &mut attachment, "getLabel", vec![Value::Int(2)]),
        Some(Value::Optional(Some(Box::new(Value::Text("tag".into())))))
    );

    // Replacing the whole entry rewrites it exactly, so the earlier sparse label does
    // not survive the replacement.
    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(2), Value::Int(9)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(2)]),
        Some(Value::Optional(Some(Box::new(Value::Int(9)))))
    );
    assert_eq!(
        run(&image, &mut attachment, "getLabel", vec![Value::Int(2)]),
        Some(Value::Optional(None)),
        "the whole-entry replacement dropped the earlier sparse label"
    );
}

/// An erase committed in its own transaction removes the entry; a later read observes
/// it absent.
#[test]
fn a_committed_erase_removes_the_entry() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(3), Value::Int(7)],
    );
    run(&image, &mut attachment, "eraseEntry", vec![Value::Int(3)]);
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(3)]),
        Some(Value::Optional(None)),
        "the committed erase removed the entry"
    );
}

/// A transaction that faults before its commit rolls back: the staged write is
/// discarded and a later read observes the pre-transaction state. This is the
/// late-rollback-restores-state law, observed across sessions on one attachment.
#[test]
fn a_fault_before_commit_rolls_the_transaction_back() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // Seed a committed value, then run a transaction that stages a replacement and
    // faults before committing.
    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(4), Value::Int(1)],
    );
    let code = run_faulting(
        &image,
        &mut attachment,
        "setThenOverflow",
        vec![Value::Int(4), Value::Int(5_000_000_000_000_000_000)],
    );
    assert_eq!(code, "run.overflow");

    // The staged replacement was rolled back; the earlier committed value stands.
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(4)]),
        Some(Value::Optional(Some(Box::new(Value::Int(1))))),
        "a fault before commit must restore the pre-transaction state"
    );
}

/// A transaction that leaves a required field unset rolls back at commit with
/// `run.required_missing` rather than publishing a partial entry; a later read
/// observes nothing was written.
#[test]
fn a_required_field_unset_at_commit_rolls_back() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    let code = run_faulting(
        &image,
        &mut attachment,
        "labelOnly",
        vec![Value::Int(5), Value::Text("hi".into())],
    );
    assert_eq!(code, "run.required_missing");

    // Neither the label nor a marker survived the rolled-back commit.
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(5)]),
        Some(Value::Optional(None))
    );
    assert_eq!(
        run(&image, &mut attachment, "getLabel", vec![Value::Int(5)]),
        Some(Value::Optional(None)),
        "the whole transaction rolled back, so the staged label is gone"
    );
}

/// A durable read after the transaction's commit is refused at verify with
/// `image.flow`: the commit consumes the session's engine transaction, so a mutating
/// export observes the store inside its region and returns values captured there. A
/// read into a local before the block closes is the supported form; a read after it
/// cannot reach a live transaction and is rejected before it could run.
#[test]
fn a_durable_read_after_commit_is_rejected() {
    let read_after = "resource Counter\n\
         \x20   required value: int\n\
         \x20   label: string\n\
         \n\
         store ^counters(id: int): Counter\n\
         \n\
         pub fn setAndGet(id: int, v: int): int?\n\
         \x20   transaction\n\
         \x20       ^counters(id) = Counter(value: v)\n\
         \x20   return ^counters(id).value\n";
    assert_eq!(verify_rejection(read_after).as_deref(), Some("image.flow"));

    // The supported form captures the value inside the region and returns the local.
    let read_inside = "resource Counter\n\
         \x20   required value: int\n\
         \x20   label: string\n\
         \n\
         store ^counters(id: int): Counter\n\
         \n\
         pub fn setAndGet(id: int, v: int): int?\n\
         \x20   var result: int? = absent\n\
         \x20   transaction\n\
         \x20       ^counters(id) = Counter(value: v)\n\
         \x20       result = ^counters(id).value\n\
         \x20   return result\n";
    let image = compile_verify(read_inside);
    let mut attachment = attach(&image);
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "setAndGet",
            vec![Value::Int(1), Value::Int(5)]
        ),
        Some(Value::Optional(Some(Box::new(Value::Int(5)))))
    );
}

/// An `unreachable` fault reached conditionally inside a transaction rolls the
/// region back, exactly like an arithmetic fault: the C01 divergence machinery and
/// the transaction effects compose. The non-diverging path commits normally.
#[test]
fn an_unreachable_fault_inside_a_transaction_rolls_back() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // The diverging path faults and discards the staged write.
    let code = run_faulting(
        &image,
        &mut attachment,
        "setThenMaybeDiverge",
        vec![Value::Int(6), Value::Int(3), Value::Bool(true)],
    );
    assert_eq!(code, "run.unreachable");
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(6)]),
        Some(Value::Optional(None)),
        "the unreachable fault rolled the transaction back"
    );

    // The same export on its non-diverging path commits the write.
    run(
        &image,
        &mut attachment,
        "setThenMaybeDiverge",
        vec![Value::Int(6), Value::Int(3), Value::Bool(false)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(6)]),
        Some(Value::Optional(Some(Box::new(Value::Int(3)))))
    );
}
