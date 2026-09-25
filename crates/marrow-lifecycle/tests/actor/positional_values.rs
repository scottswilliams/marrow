//! Stored positional values across a binding transition.
//!
//! A stored `struct` value and an enum payload are written into one cell by position, so
//! the declared names and order of their leaves are what the bytes mean. Ordinary attach
//! and explicit apply must both refuse an image that would read an existing cell with a
//! different leaf layout, and must leave the store exactly as it was.

use std::path::Path;

use marrow_codes::Code;
use marrow_lifecycle::{
    AdmissionRefusal, ApplyError, AttachOutcome, ChangedFact, HEAD_FILE, LifecycleError,
    LogicalHead, UnsupportedChange, active_binding, apply, attach, prepare,
};
use marrow_test_programs::program::{compile_bytes, export_id};
use marrow_test_support::Scratch;
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{DurableRun, Value, run_export};

use crate::support::store::provision_from;

/// One ledger for every program below: a `markers` root whose `Marker` resource stores
/// `at` (or the top-level pair `x`/`y`), and every enum anchor those programs reach.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Marker 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id root markers 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key markers.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id field Marker.at 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Marker.x 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id field Marker.y 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id sum Shape 50505050505050505050505050505050\n\
     id member Shape.rect 51515151515151515151515151515151\n\
     id sum Place 52525252525252525252525252525252\n\
     id member Place.at 53535353535353535353535353535353\n\
     id sum Option[Pos] 60606060606060606060606060606060\n\
     id member Option[Pos].none 61616161616161616161616161616161\n\
     id member Option[Pos].some 62626262626262626262626262626262\n\
     id sum Result[Pos,int] 70707070707070707070707070707070\n\
     id member Result[Pos,int].ok 71717171717171717171717171717171\n\
     id member Result[Pos,int].err 72727272727272727272727272727272\n\
     high-water 0\n\
     end\n";

const POS: &str = "struct Pos {\n    x: int\n    y: int\n}\n";
const POS_SWAPPED: &str = "struct Pos {\n    y: int\n    x: int\n}\n";
const SHAPE: &str = "enum Shape {\n    rect(width: int, height: int)\n}\n";
const SHAPE_SWAPPED: &str = "enum Shape {\n    rect(height: int, width: int)\n}\n";

/// A program storing `value: ty` in `Marker.at`, written by `put` and read by `read`.
fn program(decls: &str, ty: &str, value: &str) -> String {
    format!(
        "{decls}
resource Marker {{
    required at: {ty}
}}

store ^markers[id: int]: Marker

pub fn put(id: int, x: int) {{
    transaction {{
        ^markers[id] = Marker(at: {value})
    }}
}}

pub fn read(id: int): int {{
    if const m = ^markers[id] {{
        return 0
    }}
    return -1
}}
"
    )
}

/// The stored-struct program whose `read` returns the first leaf.
fn stored_struct(decl: &str) -> String {
    program(decl, "Pos", "Pos(x: x, y: 2)").replace("return 0", "return m.at.x")
}

fn compile(source: &str) -> VerifiedImage {
    verify(&compile_bytes(source, IDS)).expect("verify")
}

fn call(
    attachment: &mut marrow_lifecycle::NativeAttachment,
    image: &VerifiedImage,
    name: &str,
    args: Vec<Value>,
) -> Option<DurableRun> {
    run_export(
        attachment,
        marrow_image::ExportId::from_bytes(export_id(image, name)),
        args,
    )
}

/// Provision a store under `image` and write entry 1 with 7 in its first leaf.
fn provision_and_put(dir: &Path, image: &VerifiedImage) {
    provision_from(dir, image);
    let AttachOutcome::AlreadyActive(mut attachment) =
        attach(dir, prepare(image.clone())).expect("attach")
    else {
        panic!("provisioned binding")
    };
    assert!(matches!(
        call(
            &mut attachment,
            image,
            "put",
            vec![Value::Int(1), Value::Int(7)]
        ),
        Some(DurableRun::Ran(Ok(None)))
    ));
}

/// Every file in a store directory with its bytes, sorted by name.
fn snapshot(dir: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("list store")
        .map(|entry| {
            let entry = entry.expect("store entry");
            let bytes = std::fs::read(entry.path()).expect("read store artifact");
            (entry.file_name(), bytes)
        })
        .collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// Swapping the leaves of a stored struct would make every existing cell read its `x` as
/// `y`, so ordinary attach refuses it as a durable-contract change before touching the
/// store. The old image stays active and reads what it wrote, and a code-only edit of the
/// same program still rebinds.
#[test]
fn attach_refuses_a_reordered_positional_leaf_as_a_contract_change() {
    let scratch = Scratch::new("positional-attach");
    let image = compile(&stored_struct(POS));
    provision_and_put(scratch.store(), &image);
    let before = snapshot(scratch.store());

    match attach(
        scratch.store(),
        prepare(compile(&stored_struct(POS_SWAPPED))),
    ) {
        Err(LifecycleError::Refused(AdmissionRefusal::ContractChanged(refusal))) => {
            assert_eq!(refusal.changed, ChangedFact::DurableContract);
        }
        Err(error) => panic!("expected contract refusal, got {}", error.code().as_str()),
        Ok(AttachOutcome::Rebound { .. }) => {
            panic!("a reordered stored struct was rebound over the existing cells")
        }
        Ok(AttachOutcome::AlreadyActive(_)) => panic!("a changed image is not already active"),
    }
    assert_eq!(
        snapshot(scratch.store()),
        before,
        "a refused attach changes no store file"
    );

    {
        let AttachOutcome::AlreadyActive(mut attachment) =
            attach(scratch.store(), prepare(image.clone())).expect("attach old")
        else {
            panic!("the old image stays the active binding")
        };
        assert!(matches!(
            call(&mut attachment, &image, "read", vec![Value::Int(1)]),
            Some(DurableRun::Ran(Ok(Some(Value::Int(7)))))
        ));
    }

    let code_only = compile(&stored_struct(POS).replace("return -1", "return -2"));
    assert!(
        matches!(
            attach(scratch.store(), prepare(code_only)).expect("code-only attach"),
            AttachOutcome::Rebound { .. }
        ),
        "a code-only edit of a stored-struct program is a binding-only rebind"
    );
}

/// Apply refuses every change to a stored struct or enum payload leaf, at any depth,
/// before the store opens: the store bytes and the active binding stay as they were. A
/// top-level field reorder (fields are identified by ledger id) and a code-only edit still
/// apply.
#[test]
fn sparse_apply_refuses_changed_positional_leaves_without_store_changes() {
    let rows = [
        (
            "struct swap",
            program(POS, "Pos", "Pos(x: x, y: 2)"),
            program(POS_SWAPPED, "Pos", "Pos(x: x, y: 2)"),
        ),
        (
            "struct rename",
            program(POS, "Pos", "Pos(x: x, y: 2)"),
            program(&POS.replace("x: int", "z: int"), "Pos", "Pos(z: x, y: 2)"),
        ),
        (
            "payload swap",
            program(SHAPE, "Shape", "Shape::rect(width: x, height: 2)"),
            program(SHAPE_SWAPPED, "Shape", "Shape::rect(width: x, height: 2)"),
        ),
        (
            "payload rename",
            program(SHAPE, "Shape", "Shape::rect(width: x, height: 2)"),
            program(
                &SHAPE.replace("width:", "wide:"),
                "Shape",
                "Shape::rect(wide: x, height: 2)",
            ),
        ),
        (
            "nested swap",
            program(
                "struct Inner {\n    a: int\n    b: int\n}\nstruct Outer {\n    i: Inner\n    c: int\n}\n",
                "Outer",
                "Outer(i: Inner(a: x, b: 2), c: 3)",
            ),
            program(
                "struct Inner {\n    b: int\n    a: int\n}\nstruct Outer {\n    i: Inner\n    c: int\n}\n",
                "Outer",
                "Outer(i: Inner(a: x, b: 2), c: 3)",
            ),
        ),
        (
            "struct in payload",
            program(
                &format!("{POS}enum Place {{\n    at(pos: Pos)\n}}\n"),
                "Place",
                "Place::at(pos: Pos(x: x, y: 2))",
            ),
            program(
                &format!("{POS_SWAPPED}enum Place {{\n    at(pos: Pos)\n}}\n"),
                "Place",
                "Place::at(pos: Pos(x: x, y: 2))",
            ),
        ),
        (
            "enum in struct",
            program(
                &format!("{SHAPE}struct Box {{\n    s: Shape\n    n: int\n}}\n"),
                "Box",
                "Box(s: Shape::rect(width: x, height: 2), n: 3)",
            ),
            program(
                &format!("{SHAPE_SWAPPED}struct Box {{\n    s: Shape\n    n: int\n}}\n"),
                "Box",
                "Box(s: Shape::rect(width: x, height: 2), n: 3)",
            ),
        ),
        (
            "Option<Pos>",
            program(POS, "Option<Pos>", "some(Pos(x: x, y: 2))"),
            program(POS_SWAPPED, "Option<Pos>", "some(Pos(x: x, y: 2))"),
        ),
        (
            "Result<Pos, int>",
            program(POS, "Result<Pos, int>", "ok(Pos(x: x, y: 2))"),
            program(POS_SWAPPED, "Result<Pos, int>", "ok(Pos(x: x, y: 2))"),
        ),
    ];
    let mut applied = Vec::new();
    for (label, old_source, new_source) in &rows {
        let (old, new) = (compile(old_source), compile(new_source));
        let scratch = Scratch::new("apply-positional");
        provision_and_put(scratch.store(), &old);
        let before = snapshot(scratch.store());
        let error = match apply(scratch.store(), prepare(old.clone()), prepare(new), None) {
            Ok(_) => {
                applied.push(*label);
                continue;
            }
            Err(error) => error,
        };
        assert_eq!(
            error.code(),
            Code::StoreApplyUnsupported,
            "{label}: {error}"
        );
        assert!(
            matches!(
                error,
                ApplyError::Unsupported(UnsupportedChange::StoredValue)
            ),
            "{label}: {error:?}"
        );
        assert_eq!(snapshot(scratch.store()), before, "{label}");
        let head =
            LogicalHead::decode(&std::fs::read(scratch.store().join(HEAD_FILE)).expect("Head"))
                .expect("decode Head");
        assert_eq!(head.binding, active_binding(&old), "{label}");
    }
    assert!(
        applied.is_empty(),
        "apply published a program that reinterprets stored cells: {applied:?}"
    );

    let top_level = |first: &str, second: &str| {
        program("", "int", "x")
            .replace(
                "    required at: int\n",
                &format!("    required {first}: int\n    required {second}: int\n"),
            )
            .replace("Marker(at: x)", "Marker(x: x, y: 2)")
    };
    let stored = stored_struct(POS);
    for (label, old_source, new_source) in [
        (
            "top-level field swap",
            top_level("x", "y"),
            top_level("y", "x"),
        ),
        (
            "code-only edit",
            stored.clone(),
            stored.replace("y: 2", "y: 3"),
        ),
    ] {
        let (old, new) = (compile(&old_source), compile(&new_source));
        let scratch = Scratch::new("apply-positional");
        provision_and_put(scratch.store(), &old);
        apply(scratch.store(), prepare(old), prepare(new), None)
            .unwrap_or_else(|error| panic!("{label} must apply: {error}"));
    }
}
