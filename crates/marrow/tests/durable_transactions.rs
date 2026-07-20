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

pub fn setThenSpin(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: v)
        var n: int = 0
        while n < 9000000000000000000 {
            n = n + 1
        }
    }
}

pub fn spinReadOnly(id: int): int? {
    var n: int = 0
    while n < 9000000000000000000 {
        n = n + 1
    }
    return ^counters[id].value
}

pub fn getValue(id: int): int? {
    return ^counters[id].value
}

pub fn getLabel(id: int): string? {
    return ^counters[id].label
}
"#;

/// In-region guard-return exports (DX01): a `return` inside the owned region commits
/// the region's staged writes, then returns. Every export here reuses the `Counter`
/// schema so the shared identity ledger covers it. `addOnce` is the guard-return
/// shape the Workshop app teaches: the present path returns `false` (committing an
/// empty stage), the absent path stages the write and returns `true` at the closing
/// brace. `setAndReport` returns a value read inside the region on one path.
/// `setNested` returns from inside two nested guards. `setDoubled` calls a helper
/// whose own early `return` is not a region exit — only the owning export's returns
/// commit.
const GUARD_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn addOnce(id: int, v: int): bool {
    transaction {
        if exists(^counters[id]) {
            return false
        }
        ^counters[id] = Counter(value: v)
    }
    return true
}

pub fn setAndReport(id: int, v: int): int? {
    transaction {
        ^counters[id] = Counter(value: v)
        if v > 0 {
            return ^counters[id].value
        }
    }
    return absent
}

pub fn setNested(id: int, v: int, ready: bool): bool {
    transaction {
        if not exists(^counters[id]) {
            if ready {
                ^counters[id] = Counter(value: v)
                return true
            }
        }
    }
    return false
}

fn doubled(x: int): int {
    if x > 1000 {
        return 1000
    }
    return x + x
}

pub fn setDoubled(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: doubled(v))
    }
}

pub fn unconditionalReturn(id: int, v: int): int? {
    transaction {
        ^counters[id] = Counter(value: v)
        return ^counters[id].value
    }
}

pub fn bothArms(id: int, v: int, hi: bool): bool {
    transaction {
        if hi {
            ^counters[id] = Counter(value: v)
            return true
        } else {
            ^counters[id] = Counter(value: 0)
            return false
        }
    }
}

pub fn setAndDouble(id: int, v: int): int {
    transaction {
        ^counters[id] = Counter(value: v)
        return checked v + v
            on out_of_range {
                return 0
            }
    }
}

pub fn getValue(id: int): int? {
    return ^counters[id].value
}
"#;

/// A `Result`-returning export whose in-region `return` commits (DX01). `setUnlessBig`
/// stages the write, then on the over-limit path returns `err(...)` *after* the stage
/// — the author-explicit trap: the staged write commits before the `err` returns.
const RESULT_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn setUnlessBig(id: int, v: int): Result<int, string> {
    transaction {
        ^counters[id] = Counter(value: v)
        if v > 100 {
            return err("value is large")
        }
    }
    return ok(v)
}

pub fn getValue(id: int): int? {
    return ^counters[id].value
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

/// DX01: an in-region guard-return commits the region's staged writes, then returns.
/// The present path returns `false` at the guard, committing an empty stage; the
/// absent path stages the write and returns `true` at the closing brace. Both paths
/// commit, so a re-add is rejected without disturbing the first entry.
#[test]
fn an_in_region_guard_return_commits_and_returns() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    // Absent path: stage and commit, returning `true`.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "addOnce",
            vec![Value::Int(1), Value::Int(5)]
        ),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Int(5))))),
        "the absent-path in-region return committed the staged write",
    );

    // Present path: the guard returns `false` and commits nothing new, leaving the
    // first entry intact.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "addOnce",
            vec![Value::Int(1), Value::Int(9)]
        ),
        Some(Value::Bool(false)),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Int(5))))),
        "the guard-return path staged nothing, so the first value stands",
    );
}

/// DX01: an in-region `return <expr>` evaluates the expression (a durable read runs
/// pre-commit), commits, then returns. The captured value is the committed one.
#[test]
fn an_in_region_value_return_commits_the_read_value() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "setAndReport",
            vec![Value::Int(2), Value::Int(7)]
        ),
        Some(Value::Optional(Some(Box::new(Value::Int(7))))),
        "the in-region return read the staged value pre-commit and committed it",
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(2)]),
        Some(Value::Optional(Some(Box::new(Value::Int(7))))),
    );
}

/// DX01: a `return` from inside two nested guards commits the region's staged write.
#[test]
fn a_return_in_a_nested_guard_commits() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    // The nested guards reach the in-region return and commit.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "setNested",
            vec![Value::Int(3), Value::Int(4), Value::Bool(true)]
        ),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(3)]),
        Some(Value::Optional(Some(Box::new(Value::Int(4))))),
    );

    // The path that falls through the guards to the closing brace stages nothing.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "setNested",
            vec![Value::Int(4), Value::Int(4), Value::Bool(false)]
        ),
        Some(Value::Bool(false)),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(4)]),
        Some(Value::Optional(None)),
    );
}

/// DX01: a helper's own early `return` inside the owner's region is not a region exit
/// — only the owning export's returns commit. The helper returns a value to the owner,
/// which commits at its closing brace.
#[test]
fn a_helper_return_is_not_a_region_exit() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    // The helper's `x + x` return path.
    run(
        &image,
        &mut attachment,
        "setDoubled",
        vec![Value::Int(5), Value::Int(6)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(5)]),
        Some(Value::Optional(Some(Box::new(Value::Int(12))))),
    );

    // The helper's early `return 1000` path.
    run(
        &image,
        &mut attachment,
        "setDoubled",
        vec![Value::Int(6), Value::Int(2000)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(6)]),
        Some(Value::Optional(Some(Box::new(Value::Int(1000))))),
    );
}

/// DX01 all-paths-return region: a `transaction` whose only path returns from inside
/// (no trailing return after the block) is a legal, whole region — the checker accepts
/// it (the region diverges, so the function returns on every path) and the verifier
/// admits it (no unreachable closing commit is emitted). The in-region return commits
/// the staged write and returns the read value.
#[test]
fn an_unconditional_in_region_return_commits() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "unconditionalReturn",
            vec![Value::Int(1), Value::Int(8)]
        ),
        Some(Value::Optional(Some(Box::new(Value::Int(8))))),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Int(8))))),
        "the unconditional in-region return committed the staged write",
    );
}

/// DX01 all-paths-return region: an `if`/`else` where both arms return from inside the
/// region is a whole region with no fall-through. Each arm commits its own staged write
/// before returning, and no closing-brace commit is emitted.
#[test]
fn both_arms_returning_in_region_commit() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "bothArms",
            vec![Value::Int(2), Value::Int(9), Value::Bool(true)]
        ),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(2)]),
        Some(Value::Optional(Some(Box::new(Value::Int(9))))),
        "the true arm committed its staged write",
    );

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "bothArms",
            vec![Value::Int(3), Value::Int(9), Value::Bool(false)]
        ),
        Some(Value::Bool(false)),
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(3)]),
        Some(Value::Optional(Some(Box::new(Value::Int(0))))),
        "the false arm committed its staged write",
    );
}

/// DX01: a `return checked` inside a region commits on its success path — the
/// `CheckedBind::Return` lowering site places the commit before the return. The whole
/// region is an all-paths-return region (the checked form and its diverging arm both
/// exit), so no closing commit is emitted.
#[test]
fn a_return_checked_in_region_commits() {
    let image = compile_verify(GUARD_SOURCE);
    let mut attachment = attach(&image);

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "setAndDouble",
            vec![Value::Int(4), Value::Int(5)]
        ),
        Some(Value::Int(10)),
        "the checked success path returned 2v after committing",
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(4)]),
        Some(Value::Optional(Some(Box::new(Value::Int(5))))),
        "the return-checked in-region path committed the staged write",
    );
}

/// DX01 adversarial: `return err(...)` after a staged write COMMITS the write. The
/// in-region return is a commit site regardless of the returned `Result` tag — the
/// author-explicit trap the decision of record pins. The over-limit path returns an
/// `err` value, yet the staged write is durably committed and reads back.
#[test]
fn a_return_err_after_staged_writes_commits_them() {
    let image = compile_verify(RESULT_SOURCE);
    let mut attachment = attach(&image);

    // Over-limit path: the export returns `err(...)`, but the staged write commits.
    run(
        &image,
        &mut attachment,
        "setUnlessBig",
        vec![Value::Int(1), Value::Int(200)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Int(200))))),
        "the in-region `return err(...)` committed the staged write",
    );

    // Under-limit path commits and returns `ok(...)`; the value reads back too.
    run(
        &image,
        &mut attachment,
        "setUnlessBig",
        vec![Value::Int(2), Value::Int(50)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(2)]),
        Some(Value::Optional(Some(Box::new(Value::Int(50))))),
    );
}

/// DX01 re-pin: prefix `try` still may not cross an owned region. Its implicit `err`
/// exit carries no commit, so a `try` on a path that returns before the region's
/// commit is refused at verify with `image.flow` — *a path returns without committing
/// the transaction* — exactly as before this lane. The sharpened rationale: a spelled
/// `return` is a visible commit sentence; `try`'s exit is implicit and carries none.
#[test]
fn a_try_crossing_a_region_is_still_rejected() {
    let try_crossing = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

fn check(v: int): Result<int, string> {
    if v > 0 {
        return ok(v)
    }
    return err("value must be positive")
}

pub fn setChecked(id: int, v: int): Result<int, string> {
    transaction {
        const w = try check(v)
        ^counters[id] = Counter(value: w)
    }
    return ok(v)
}
"#;
    assert_eq!(
        verify_rejection(try_crossing).as_deref(),
        Some("image.flow")
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
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => {
            diagnostics.iter().map(|d| d.code.to_string()).collect()
        }
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
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

/// BF01 exit-gate evidence: an instruction-budget exhaustion raised inside a
/// transaction is a source-uncatchable `run.budget` fault that rolls the whole
/// region back and leaves the attachment usable — an abort, never a poison. A value
/// committed by an earlier invocation survives the faulting one, and a *subsequent*
/// mutating invocation on the same attachment commits normally. This is the
/// budget-family instance of the rollback-isolation law already pinned above for
/// overflow, required-missing, and unreachable faults; it fixes budget exhaustion as
/// an ordinary rolling-back terminal fault before the E07 taxonomy freeze.
///
/// Ignored in the default suite: the instruction budget is a private VM constant
/// (`1 << 26`) with no runner, CLI, or environment override by design, so the
/// `setThenSpin` invocation burns the whole budget (~67M interpreted instructions;
/// the seed and follow-up invocations are cheap). Run it with:
///     cargo test -p marrow --test durable_transactions -- --ignored budget
#[test]
#[ignore = "burns the whole 1<<26 instruction budget (private VM const, no override) — ~1.3s debug; E07-gating evidence, run with --ignored"]
fn a_budget_exhaustion_inside_a_transaction_rolls_back_without_poisoning() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    // Seed a committed value in its own transaction.
    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(7), Value::Int(1)],
    );

    // A transaction stages a replacement, then exhausts the instruction budget before
    // reaching its commit; the terminal observes the typed `run.budget` fault.
    let code = run_faulting(
        &image,
        &mut attachment,
        "setThenSpin",
        vec![Value::Int(7), Value::Int(9)],
    );
    assert_eq!(code, "run.budget");

    // The staged replacement rolled back: the earlier committed value stands.
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(7)]),
        Some(Value::Optional(Some(Box::new(Value::Int(1))))),
        "the budget fault rolled the region back to the pre-transaction state"
    );

    // A subsequent mutating invocation on the same attachment commits normally: the
    // budget abort left the store usable rather than poisoning it.
    run(
        &image,
        &mut attachment,
        "set",
        vec![Value::Int(7), Value::Int(42)],
    );
    assert_eq!(
        run(&image, &mut attachment, "getValue", vec![Value::Int(7)]),
        Some(Value::Optional(Some(Box::new(Value::Int(42))))),
        "a later transaction commits, so the budget abort did not poison the attachment"
    );
}

/// BF01 out-of-region pin: the identical budget exhaustion outside any transaction is
/// the plain source-uncatchable fault death. A read-only export carries no
/// transaction region, so there is nothing to roll back and the behavior is
/// unchanged — the terminal observes `run.budget` and no durable state is involved.
///
/// Ignored for the same private-budget reason as the sibling above.
#[test]
#[ignore = "burns the whole 1<<26 instruction budget (private VM const, no override) — ~1.3s debug; E07-gating evidence, run with --ignored"]
fn a_budget_exhaustion_outside_a_region_is_the_plain_fault_death() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);
    let code = run_faulting(&image, &mut attachment, "spinReadOnly", vec![Value::Int(8)]);
    assert_eq!(code, "run.budget");
}
