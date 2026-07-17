//! The lexical transaction region and exact mutations, executed end to end.
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
const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn set(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: v)
    }
}

pub fn setLabel(id: int, text: string) {
    transaction {
        ^counters[id].label = text
    }
}

pub fn eraseEntry(id: int) {
    transaction {
        delete ^counters[id]
    }
}

pub fn labelOnly(id: int, text: string) {
    transaction {
        ^counters[id].label = text
    }
}

pub fn setThenOverflow(id: int, big: int) {
    transaction {
        ^counters[id] = Counter(value: 1)
        ^counters[id].value = big + big
    }
}

pub fn setThenMaybeDiverge(id: int, v: int, boom: bool) {
    transaction {
        ^counters[id] = Counter(value: v)
        if boom {
            unreachable("the invariant broke mid-transaction")
        }
    }
}

pub fn getValue(id: int): int? {
    return ^counters[id].value
}

pub fn getLabel(id: int): string? {
    return ^counters[id].label
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
        Ephemeral::Ready(attachment) => *attachment,
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
    let read_after = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn setAndGet(id: int, v: int): int? {
    transaction {
        ^counters[id] = Counter(value: v)
    }
    return ^counters[id].value
}
"#;
    assert_eq!(verify_rejection(read_after).as_deref(), Some("image.flow"));

    // The supported form captures the value inside the region and returns the local.
    let read_inside = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn setAndGet(id: int, v: int): int? {
    var result: int? = absent
    transaction {
        ^counters[id] = Counter(value: v)
        result = ^counters[id].value
    }
    return result
}
"#;
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

/// The typed check-time diagnostic codes a source produces, or an empty vector when
/// it compiles. A mutating helper called without an ambient transaction is refused
/// here — at check time, with a call-site span — not only at verify.
fn compile_error_codes(source: &str) -> Vec<String> {
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
    match marrow_compile::compile(&project) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics.iter().map(|d| d.code.to_string()).collect(),
    }
}

const HELPER_STORE: &str = r#"resource Counter {
    required value: int
}

store ^counters[id: int]: Counter

fn writeIt(id: int, v: int) {
    ^counters[id] = Counter(value: v)
}
"#;

/// A mutating helper called from an export with no ambient transaction is refused at
/// check time, at the call-site span, before an image is minted.
#[test]
fn a_mutating_helper_called_without_a_transaction_is_a_check_error() {
    let source =
        format!("{HELPER_STORE}\npub fn plainCaller(id: int, v: int) {{\n    writeIt(id, v)\n}}\n");
    assert_eq!(
        compile_error_codes(&source),
        vec!["check.requires_transaction".to_string()],
    );
}

/// The same helper wrapped in an ambient `transaction` block checks, verifies, and
/// commits its write.
#[test]
fn a_mutating_helper_inside_a_transaction_checks_and_runs() {
    let source = format!(
        "{HELPER_STORE}\n\
         pub fn wrappedCaller(id: int, v: int) {{\n\
         \x20   transaction {{\n\
         \x20       writeIt(id, v)\n\
         \x20   }}\n\
         }}\n\
         pub fn getValue(id: int): int? {{\n\
         \x20   return ^counters[id].value\n\
         }}\n"
    );
    assert!(
        compile_error_codes(&source).is_empty(),
        "the wrapped call checks"
    );

    let image = compile_verify(&source);
    let mut attachment = attach(&image);
    run(
        &image,
        &mut attachment,
        "wrappedCaller",
        vec![Value::Int(1), Value::Int(9)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Int(9))))),
    );
}

/// The requirement propagates transitively: a helper that calls a mutating helper
/// itself requires an ambient transaction, so an export that calls it unwrapped is
/// refused at the outer call site.
#[test]
fn the_transaction_requirement_propagates_transitively() {
    let source = format!(
        "{HELPER_STORE}\n\
         fn writeOuter(id: int, v: int) {{\n\
         \x20   writeIt(id, v)\n\
         }}\n\
         pub fn plainCaller(id: int, v: int) {{\n\
         \x20   writeOuter(id, v)\n\
         }}\n"
    );
    assert_eq!(
        compile_error_codes(&source),
        vec!["check.requires_transaction".to_string()],
    );
}

/// A direct durable mutation in an export body with no ambient transaction is refused
/// at check time at the mutation's span (not only at verify).
#[test]
fn a_direct_mutation_outside_a_transaction_is_a_check_error() {
    let source = format!(
        "{HELPER_STORE}\npub fn plainWrite(id: int, v: int) {{\n    ^counters[id] = Counter(value: v)\n}}\n"
    );
    assert_eq!(
        compile_error_codes(&source),
        vec!["check.requires_transaction".to_string()],
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
