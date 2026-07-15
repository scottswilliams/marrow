//! G00a: the host-neutral wire interface descriptor and `InterfaceId`, observed
//! through the full production path (capture -> compile -> verify -> reconstruct).
//!
//! The interface is reconstructed from the verified image alone — export ids,
//! function parameter/return types, the record/enum tables, and each export's
//! `DemandSetId` — so nothing about the interface is serialized into the image and
//! the `InterfaceId` derives from verified facts (like `DemandSetId` derives at
//! verify). A body-only edit that changes no signature and no demand leaves the
//! `InterfaceId` unchanged; any signature change moves it.

use marrow_image::{
    EnumShape, ExportSignature, FieldShape, ImageType, Interface, InterfaceError, InterfaceId,
    RecordShape, VariantShape,
};
use marrow_verify::{RetShape, VerifiedImage};

/// Compile and verify one `src/main.mw` through the production path.
fn compile_verify(source: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        None,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

/// Map a function's decoded return shape to the bare-or-optional `ImageType` the
/// interface builder consumes. A one-to-one projection.
fn ret_to_image(ret: RetShape) -> ImageType {
    match ret {
        RetShape::Unit => ImageType::Unit,
        RetShape::Scalar { scalar, optional } => ImageType::Scalar { scalar, optional },
        RetShape::Record { idx, optional } => ImageType::Record { idx, optional },
        RetShape::Enum { idx, optional } => ImageType::Enum { idx, optional },
        RetShape::Collection { idx, optional } => ImageType::Collection { idx, optional },
    }
}

/// Reconstruct the wire interface from a verified image, using only its public
/// accessors. This is the thin projection both real callers (the terminal and the
/// generated TypeScript client) build the descriptor set through; the identity,
/// transfer-graph law, and canonical encoding live in `marrow-image`.
fn interface_of(image: &VerifiedImage) -> Result<Interface, InterfaceError> {
    let records: Vec<RecordShape> = image
        .record_types()
        .iter()
        .map(|record| RecordShape {
            fields: record
                .fields()
                .iter()
                .map(|field| FieldShape {
                    name: field.name.to_string(),
                    ty: field.ty,
                    required: field.required,
                })
                .collect(),
        })
        .collect();
    let enums: Vec<EnumShape> = image
        .enums()
        .iter()
        .map(|enum_type| EnumShape {
            variants: enum_type
                .variants()
                .iter()
                .map(|variant| VariantShape {
                    name: variant.name.to_string(),
                    category: variant.category,
                    payload: variant.payload.clone(),
                })
                .collect(),
        })
        .collect();
    let exports: Vec<ExportSignature> = image
        .exports()
        .iter()
        .map(|export| {
            let function = image.function(export.function());
            ExportSignature {
                id: export.id(),
                params: function.params().to_vec(),
                ret: ret_to_image(function.ret()),
                demand_id: export.demand_id(),
            }
        })
        .collect();
    Interface::build(exports, &records, &enums)
}

fn interface_id(source: &str) -> InterfaceId {
    interface_of(&compile_verify(source))
        .expect("interface reconstructs")
        .interface_id()
}

const TWO_EXPORTS: &str = "struct Point\n\
    \x20   x: int\n\
    \x20   y: int\n\
    \n\
    pub fn add(a: int, b: int): int\n\
    \x20   return a + b\n\
    \n\
    pub fn shift(p: Point, dx: int): Point\n\
    \x20   return Point(x: p.x + dx, y: p.y)\n";

/// The two-export storeless fixture reconstructs a two-descriptor interface, and its
/// `InterfaceId` is deterministic across recompiles.
#[test]
fn two_export_interface_reconstructs_deterministically() {
    let image = compile_verify(TWO_EXPORTS);
    let interface = interface_of(&image).expect("interface reconstructs");
    assert_eq!(interface.descriptors().len(), 2);
    // Determinism: an independent recompile yields the same identity.
    assert_eq!(interface.interface_id(), interface_id(TWO_EXPORTS));
}

/// A body-only edit — the observable signatures and demands are unchanged — leaves
/// the `InterfaceId` fixed, even though the image bytes (and `ImageId`) change.
#[test]
fn a_body_edit_keeps_the_interface_id() {
    let base = interface_id(TWO_EXPORTS);

    // `add` returns `b + a` and `shift` uses a local; neither signature nor demand
    // changes, so the interface identity must not move.
    let edited = "struct Point\n\
        \x20   x: int\n\
        \x20   y: int\n\
        \n\
        pub fn add(a: int, b: int): int\n\
        \x20   return b + a\n\
        \n\
        pub fn shift(p: Point, dx: int): Point\n\
        \x20   const nx = p.x + dx\n\
        \x20   return Point(x: nx, y: p.y)\n";

    let base_image = compile_verify(TWO_EXPORTS);
    let edited_image = compile_verify(edited);
    // The bytes genuinely differ (this is a real body edit, not a no-op).
    assert_ne!(base_image.image_id(), edited_image.image_id());
    assert_eq!(base, interface_id(edited));
}

/// A parameter-type change moves the `InterfaceId`.
#[test]
fn a_parameter_type_change_moves_the_interface_id() {
    let base = interface_id(TWO_EXPORTS);
    let widened = "struct Point\n\
        \x20   x: int\n\
        \x20   y: int\n\
        \n\
        pub fn add(a: int, b: string): int\n\
        \x20   return a\n\
        \n\
        pub fn shift(p: Point, dx: int): Point\n\
        \x20   return Point(x: p.x + dx, y: p.y)\n";
    assert_ne!(base, interface_id(widened));
}

/// Renaming a record field used in a signature moves the `InterfaceId` — a record's
/// field names are part of the observable transfer shape.
#[test]
fn a_record_field_rename_moves_the_interface_id() {
    let base = interface_id(TWO_EXPORTS);
    let renamed = "struct Point\n\
        \x20   x: int\n\
        \x20   z: int\n\
        \n\
        pub fn add(a: int, b: int): int\n\
        \x20   return a + b\n\
        \n\
        pub fn shift(p: Point, dz: int): Point\n\
        \x20   return Point(x: p.x, z: p.z + dz)\n";
    assert_ne!(base, interface_id(renamed));
}

/// A signature reaching a collection is rejected with a typed exclusion, observed
/// through the production path: collections are not yet in the transfer graph.
#[test]
fn a_collection_returning_export_is_excluded() {
    let source = "pub fn items(): List[int]\n\
        \x20   var xs: List[int] = List()\n\
        \x20   return xs\n";
    let image = compile_verify(source);
    let error = interface_of(&image).expect_err("a collection return is excluded");
    assert!(matches!(error, InterfaceError::TransferTypeExcluded { .. }));
}
