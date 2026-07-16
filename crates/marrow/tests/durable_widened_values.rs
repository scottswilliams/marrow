//! E03w slice E: widened durable field values, stored and read end to end.
//!
//! A durable field's stored value is drawn from the closed storable set — a scalar, a
//! dense `struct` (product), or a closed `enum`/`Option`/`Result` (sum). The durable
//! value codec frames a composite inline in the one field-leaf cell, so a widened field
//! is executable, not parked. These tests drive the whole production path — capture ->
//! compile -> verify -> attach -> VM — against one persistent ephemeral attachment,
//! storing and reading back a record-typed field, an enum-typed field, and an
//! `Option`-typed field, including the sparse `Option[string]` three-state read
//! (absent cell vs present-`none` vs present-`some`) and a widened value used in an
//! expression after read.

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0\n\
     id product Account d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0\n\
     id root accounts b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0\n\
     id key accounts.id c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0\n\
     id field Account.id e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0\n\
     id field Account.kind e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1\n\
     id field Account.owner e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2\n\
     id field Account.note e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3\n\
     id sum Access 50505050505050505050505050505050\n\
     id member Access.reader 51515151515151515151515151515151\n\
     id member Access.writer 52525252525252525252525252525252\n\
     id member Access.admin 53535353535353535353535353535353\n\
     id sum Option[string] 60606060606060606060606060606060\n\
     id member Option[string].none 61616161616161616161616161616161\n\
     id member Option[string].some 62626262626262626262626262626262\n\
     high-water 0\n\
     end\n";

// `Account` stores a required enum (`kind`), a required dense struct (`owner`), and a
// sparse `Option[string]` (`note`). The sparse `note` gives the cell-presence axis on
// top of the in-band `Option` value — the three-state fixture.
const SOURCE: &str = "resource Account\n\
     \x20   required id: int\n\
     \x20   required kind: Access\n\
     \x20   required owner: Name\n\
     \x20   note: Option[string]\n\
     \n\
     struct Name\n\
     \x20   first: string\n\
     \x20   last: string\n\
     \n\
     enum Access\n\
     \x20   reader\n\
     \x20   writer\n\
     \x20   admin\n\
     \n\
     store ^accounts(id: int): Account\n\
     \n\
     fn ada(): Name\n\
     \x20   return Name(first: \"Ada\", last: \"Lovelace\")\n\
     \n\
     fn wrapSome(s: string): Option[string]\n\
     \x20   return some(s)\n\
     \n\
     fn wrapNone(): Option[string]\n\
     \x20   return none\n\
     \n\
     pub fn createReader(id: int)\n\
     \x20   transaction\n\
     \x20       ^accounts(id) = Account(id: id, kind: Access::reader, owner: ada())\n\
     \n\
     pub fn createAdmin(id: int)\n\
     \x20   transaction\n\
     \x20       ^accounts(id) = Account(id: id, kind: Access::admin, owner: ada())\n\
     \n\
     pub fn readKind(id: int): Access?\n\
     \x20   return ^accounts(id).kind\n\
     \n\
     pub fn isAdmin(id: int): bool\n\
     \x20   if const k = ^accounts(id).kind\n\
     \x20       return k == Access::admin\n\
     \x20   return false\n\
     \n\
     pub fn readOwner(id: int): Name?\n\
     \x20   return ^accounts(id).owner\n\
     \n\
     pub fn createNoteSome(id: int, s: string)\n\
     \x20   transaction\n\
     \x20       ^accounts(id) = Account(id: id, kind: Access::reader, owner: ada(), note: wrapSome(s))\n\
     \n\
     pub fn createNoteNone(id: int)\n\
     \x20   transaction\n\
     \x20       ^accounts(id) = Account(id: id, kind: Access::reader, owner: ada(), note: wrapNone())\n\
     \n\
     pub fn readNote(id: int): Option[string]?\n\
     \x20   return ^accounts(id).note\n";

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

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => attachment,
        Ephemeral::Parked => panic!("a widened-field store is executable, not parked"),
        Ephemeral::Failed(code) => panic!("attach failed: {code}"),
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

/// Unwrap the present value of a field read (`Optional(Some(v))`), panicking on an
/// absent cell — the read of a set field always finds the cell.
fn present(value: Option<Value>) -> Value {
    match value {
        Some(Value::Optional(Some(inner))) => *inner,
        other => panic!("expected a present field read, got {other:?}"),
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn id(n: i64) -> Vec<Value> {
    vec![Value::Int(n)]
}

#[test]
fn a_required_enum_field_round_trips_and_drives_an_expression() {
    let image = compile_verify(SOURCE);
    let mut store = attach(&image);
    run(&image, &mut store, "createReader", id(1));

    // The entry's required enum reads back as `Access::reader` (variant 0, empty payload).
    match present(run(&image, &mut store, "readKind", id(1))) {
        Value::Enum(_, variant, payload) => {
            assert_eq!(variant, 0, "reader is variant 0");
            assert!(payload.is_empty(), "reader carries no payload");
        }
        other => panic!("not an enum: {other:?}"),
    }
    // A widened value used in an expression after read: `if const` binds the read enum
    // and `==` compares it — `reader` is not `admin`.
    assert_eq!(
        run(&image, &mut store, "isAdmin", id(1)),
        Some(Value::Bool(false))
    );

    // A whole-entry replace with `admin` (variant 2) round-trips a different variant, and
    // the same read-and-compare now observes it.
    run(&image, &mut store, "createAdmin", id(1));
    match present(run(&image, &mut store, "readKind", id(1))) {
        Value::Enum(_, variant, _) => assert_eq!(variant, 2, "admin is variant 2"),
        other => panic!("not an enum: {other:?}"),
    }
    assert_eq!(
        run(&image, &mut store, "isAdmin", id(1)),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_record_field_round_trips_with_its_dense_leaves() {
    let image = compile_verify(SOURCE);
    let mut store = attach(&image);
    run(&image, &mut store, "createReader", id(2));

    // The dense struct reads back with both leaves present, in declaration order.
    match present(run(&image, &mut store, "readOwner", id(2))) {
        Value::Record(_, slots) => {
            assert_eq!(slots.len(), 2);
            assert_eq!(slots[0], Some(text("Ada")));
            assert_eq!(slots[1], Some(text("Lovelace")));
        }
        other => panic!("not a record: {other:?}"),
    }
}

#[test]
fn a_sparse_option_field_reads_three_distinct_states() {
    let image = compile_verify(SOURCE);
    let mut store = attach(&image);
    run(&image, &mut store, "createReader", id(3));

    // State 1 — the cell is absent (the sparse field was never set): read yields `none`
    // at the presence axis (`Optional(None)`), not an in-band value.
    assert_eq!(
        run(&image, &mut store, "readNote", id(3)),
        Some(Value::Optional(None))
    );

    // State 2 — present `none`: the cell holds the `Option` value `none` (variant 0).
    run(&image, &mut store, "createNoteNone", id(3));
    match present(run(&image, &mut store, "readNote", id(3))) {
        Value::Enum(_, variant, payload) => {
            assert_eq!(variant, 0, "none is variant 0");
            assert!(payload.is_empty());
        }
        other => panic!("present-none is not an enum: {other:?}"),
    }

    // State 3 — present `some("hi")`: the cell holds `some` (variant 1) with the payload.
    run(
        &image,
        &mut store,
        "createNoteSome",
        vec![Value::Int(3), text("hi")],
    );
    match present(run(&image, &mut store, "readNote", id(3))) {
        Value::Enum(_, variant, payload) => {
            assert_eq!(variant, 1, "some is variant 1");
            assert_eq!(payload.as_ref(), [text("hi")]);
        }
        other => panic!("present-some is not an enum: {other:?}"),
    }
}
