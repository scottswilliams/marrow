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
    CollectionShape, EnumShape, ExportSignature, FieldShape, ImageType, Interface, InterfaceError,
    InterfaceId, RecordShape, RootShape, TransferType, VariantShape,
};
use marrow_verify::{RetShape, SealedCollectionType, VerifiedImage};

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
        RetShape::Identity { root, optional } => ImageType::Identity { root, optional },
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
    let collections: Vec<CollectionShape> = image
        .collections()
        .iter()
        .map(|collection| match *collection {
            SealedCollectionType::List { elem } => CollectionShape::List { elem },
            SealedCollectionType::Map { key, value } => CollectionShape::Map { key, value },
        })
        .collect();
    let roots: Vec<RootShape> = image
        .roots()
        .iter()
        .map(|root| RootShape {
            name: root.name().to_string(),
            keys: root.keys().to_vec(),
        })
        .collect();
    Interface::build(exports, &records, &enums, &collections, &roots)
}

fn interface_id(source: &str) -> InterfaceId {
    interface_of(&compile_verify(source))
        .expect("interface reconstructs")
        .interface_id()
}

const TWO_EXPORTS: &str = r#"struct Point {
    x: int
    y: int
}

pub fn add(a: int, b: int): int {
    return a + b
}

pub fn shift(p: Point, dx: int): Point {
    return Point(x: p.x + dx, y: p.y)
}
"#;

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
    let edited = r#"struct Point {
    x: int
    y: int
}

pub fn add(a: int, b: int): int {
    return b + a
}

pub fn shift(p: Point, dx: int): Point {
    const nx = p.x + dx
    return Point(x: nx, y: p.y)
}
"#;

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
    let widened = r#"struct Point {
    x: int
    y: int
}

pub fn add(a: int, b: string): int {
    return a
}

pub fn shift(p: Point, dx: int): Point {
    return Point(x: p.x + dx, y: p.y)
}
"#;
    assert_ne!(base, interface_id(widened));
}

/// Renaming a record field used in a signature moves the `InterfaceId` — a record's
/// field names are part of the observable transfer shape.
#[test]
fn a_record_field_rename_moves_the_interface_id() {
    let base = interface_id(TWO_EXPORTS);
    let renamed = r#"struct Point {
    x: int
    z: int
}

pub fn add(a: int, b: int): int {
    return a + b
}

pub fn shift(p: Point, dz: int): Point {
    return Point(x: p.x, z: p.z + dz)
}
"#;
    assert_ne!(base, interface_id(renamed));
}

/// A signature reaching a collection projects into the transfer graph through the
/// production path: the earned `List<int>` carrier resolves to `TransferType::List`.
#[test]
fn a_collection_returning_export_projects() {
    let source = r#"pub fn items(): List<int> {
    var xs: List<int> = List()
    return xs
}
"#;
    let image = compile_verify(source);
    let interface = interface_of(&image).expect("a collection return projects");
    let items = interface
        .descriptors()
        .iter()
        .find(|d| matches!(d.ret(), TransferType::List(_)))
        .expect("the items export returns a list");
    assert!(matches!(
        items.ret(),
        TransferType::List(inner) if matches!(**inner, TransferType::Scalar(marrow_image::Scalar::Int))
    ));
}
