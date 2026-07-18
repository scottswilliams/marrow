//! MR01: the presence lattice is keyed by `(root, key-slot)`, not by key-slot alone. Two
//! int-keyed roots (`^aaa` + `^bbb`) share a resource shape — a required `tag` and a sparse
//! `note` — and a function reads a single key parameter `k` used against both roots, so the
//! same key-slot addresses both. A presence guard proving `^aaa[k]` present must not be
//! read as proving `^bbb[k]` present (no phantom marker), and a write named on `^bbb[k]`
//! must address `^bbb`, never `^aaa`.
//!
//! The negative case exercises both at once: guarded by `^aaa[k]` presence, a sparse write
//! to the *absent* `^bbb[k]` stages a field leaf on an unmarked entry whose required `tag`
//! is unset, so the transaction rolls back with `run.required_missing` — which is only
//! reachable if the write went to `^bbb` (not a phantom write to the present `^aaa`) and if
//! `^bbb[k]` was NOT mis-proven present (a strict present write would instead fault as a
//! marker mismatch). Both roots are clean afterward.

use marrow_kernel::durable::EphemeralAttachment;
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Aaa 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Aaa.tag 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Aaa.note 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root aaa 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key aaa.k 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id product Bbb 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Bbb.tag 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id field Bbb.note 1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f\n\
     id root bbb 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key bbb.k 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Aaa {
    required tag: string
    note: string
}

resource Bbb {
    required tag: string
    note: string
}

store ^aaa[k: int]: Aaa
store ^bbb[k: int]: Bbb

pub fn putAaa(k: int, tag: string) {
    transaction {
        ^aaa[k] = Aaa(tag: tag)
    }
}

pub fn putBbb(k: int, tag: string) {
    transaction {
        ^bbb[k] = Bbb(tag: tag)
    }
}

pub fn aaaNote(k: int): string? {
    return ^aaa[k].note
}

pub fn bbbTag(k: int): string? {
    return ^bbb[k].tag
}

pub fn bbbNote(k: int): string? {
    return ^bbb[k].note
}

pub fn setAaaNoteIfPresent(k: int, n: string) {
    transaction {
        if const a = ^aaa[k] {
            ^aaa[k].note = n
        }
    }
}

pub fn setBbbNoteUnderAaaGuard(k: int, n: string) {
    transaction {
        if const a = ^aaa[k] {
            ^bbb[k].note = n
        }
    }
}
"#;

fn compile_verify() -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        SOURCE.as_bytes().to_vec(),
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
    attachment: &mut EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        other => panic!("{name} did not run cleanly: {:?}", DebugRun(&other)),
    }
}

fn run_faulting(
    image: &VerifiedImage,
    attachment: &mut EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> String {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Err(fault)) => fault.code().to_string(),
        other => panic!("{name} did not fault: {:?}", DebugRun(&other)),
    }
}

fn attach(image: &VerifiedImage) -> EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a two-root image must be executable, not parked"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

fn some_text(v: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(v.into())))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

/// Positive control: a presence guard on `^aaa[k]` followed by a sparse write on that same
/// entry commits normally, and the sibling root `^bbb[k]` — sharing the key-slot — is never
/// touched.
#[test]
fn a_present_guarded_write_addresses_its_own_root_only() {
    let image = compile_verify();
    let mut store = attach(&image);

    run(&image, &mut store, "putAaa", vec![Value::Int(1), text("a")]);
    run(
        &image,
        &mut store,
        "setAaaNoteIfPresent",
        vec![Value::Int(1), text("hello")],
    );

    assert_eq!(
        run(&image, &mut store, "aaaNote", vec![Value::Int(1)]),
        some_text("hello"),
        "the present-guarded sparse write committed on ^aaa",
    );
    // ^bbb[1] shares the key-slot but was never written.
    assert_eq!(
        run(&image, &mut store, "bbbTag", vec![Value::Int(1)]),
        absent(),
        "the sibling root ^bbb was not phantom-written",
    );
    assert_eq!(
        run(&image, &mut store, "bbbNote", vec![Value::Int(1)]),
        absent(),
    );
}

/// A presence guard proving `^aaa[k]` present does not phantom-mark `^bbb[k]` present, and a
/// write named on `^bbb[k]` addresses `^bbb`, not `^aaa`. With `^bbb[k]` absent, the sparse
/// write stages a leaf on an unmarked entry whose required `tag` is unset, so the
/// transaction rolls back with `run.required_missing`. Both roots are clean afterward.
#[test]
fn a_cross_root_guarded_write_does_not_phantom_the_sibling_root() {
    let image = compile_verify();
    let mut store = attach(&image);

    // ^aaa[1] is present; ^bbb[1] is deliberately absent.
    run(&image, &mut store, "putAaa", vec![Value::Int(1), text("a")]);

    let code = run_faulting(
        &image,
        &mut store,
        "setBbbNoteUnderAaaGuard",
        vec![Value::Int(1), text("leak")],
    );
    assert_eq!(
        code, "run.required_missing",
        "the write addressed the absent ^bbb (not the present ^aaa) and ^bbb[k] was not \
         mis-proven present, so the unmarked sparse leaf rolls back at commit",
    );

    // The rolled-back transaction left both roots clean: ^bbb[1] never came into existence,
    // and the guard never wrote ^aaa's own note.
    assert_eq!(
        run(&image, &mut store, "bbbTag", vec![Value::Int(1)]),
        absent(),
        "^bbb[1] was never created",
    );
    assert_eq!(
        run(&image, &mut store, "bbbNote", vec![Value::Int(1)]),
        absent(),
        "no phantom ^bbb note survived the rollback",
    );
    assert_eq!(
        run(&image, &mut store, "aaaNote", vec![Value::Int(1)]),
        absent(),
        "the guard did not phantom-write ^aaa's own note",
    );
}
