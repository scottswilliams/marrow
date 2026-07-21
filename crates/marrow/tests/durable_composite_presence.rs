//! Composite-root `place` presence shortcuts, executed end to end (DX05 gap 4 pin).
//!
//! A `place` bound to a composite-key root (`place e = ^t[a, b]`) carries several key
//! slots yet is still a root (the PL01 provenance). Its presence shortcuts run through the
//! whole production path — capture -> compile -> verify -> attach -> VM — over one
//! persistent ephemeral attachment:
//!
//! - `if exists(e) { e.f = v }` is a guarded (strict) sparse set through the place;
//! - `if const e = ^t[a, b] { … }` binds the whole entry through the composite root;
//! - `exists(^t[a, b])` probes a composite-root entry inline.
//!
//! This pins that all three compose over a composite key, closing the DX05 gap the PL01
//! explicit-place work already made executable.

use marrow_kernel::durable::EphemeralAttachment;
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// application, product, the required `grade` and sparse `note` fields, the composite root
// placement, and its two key columns (student, course).
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Enrollment 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Enrollment.grade 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Enrollment.note 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root enrollments 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key enrollments.student 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key enrollments.course 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Enrollment {
    required grade: int
    note: string
}

store ^enrollments[student: string, course: string]: Enrollment

pub fn enroll(s: string, c: string, g: int) {
    transaction {
        ^enrollments[s, c] = Enrollment(grade: g)
    }
}

pub fn setNoteIfPresent(s: string, c: string, note: string) {
    transaction {
        place e = ^enrollments[s, c]
        if exists(e) {
            e.note = note
        }
    }
}

pub fn noteOf(s: string, c: string): string? {
    if const e = ^enrollments[s, c] {
        return e.note
    }
    return absent
}

pub fn present(s: string, c: string): bool {
    return exists(^enrollments[s, c])
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

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

fn attach(image: &VerifiedImage) -> EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("the enrollments root must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
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
        DurableRun::Ran(Err(fault)) => panic!("{name} faulted at run: {}", fault.code()),
        DurableRun::Parked => panic!("{name} parked — the composite-root shortcut is not executable"),
        DurableRun::Failed(code) => panic!("{name} failed to mint its attachment: {code}"),
    }
}

fn s(v: &str) -> Value {
    Value::Text(v.into())
}

#[test]
fn composite_root_place_presence_shortcuts_run_end_to_end() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);

    // Seed one composite-key entry.
    run(&image, &mut store, "enroll", vec![s("ada"), s("cs"), Value::Int(95)]);

    // `exists(^t[a, b])` inline over a composite root: present for the seeded key, absent
    // for an unseeded one.
    assert_eq!(
        run(&image, &mut store, "present", vec![s("ada"), s("cs")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(&image, &mut store, "present", vec![s("bob"), s("cs")]),
        Some(Value::Bool(false))
    );

    // The guarded (strict) sparse set through the composite-root place writes only where the
    // entry is present.
    run(
        &image,
        &mut store,
        "setNoteIfPresent",
        vec![s("ada"), s("cs"), s("top")],
    );
    // A guarded set against an absent composite entry is a no-op: it neither writes the note
    // nor creates the entry.
    run(
        &image,
        &mut store,
        "setNoteIfPresent",
        vec![s("bob"), s("cs"), s("ignored")],
    );

    // `if const e = ^t[a, b]` binds the whole entry through the composite root and reads the
    // sparse field back.
    assert_eq!(
        run(&image, &mut store, "noteOf", vec![s("ada"), s("cs")]),
        Some(Value::Optional(Some(Box::new(s("top")))))
    );
    // The no-op guarded set left `bob` absent.
    assert_eq!(
        run(&image, &mut store, "present", vec![s("bob"), s("cs")]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run(&image, &mut store, "noteOf", vec![s("bob"), s("cs")]),
        Some(Value::Optional(None))
    );
}
