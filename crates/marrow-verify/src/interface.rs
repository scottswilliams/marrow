//! The one projection from a [`VerifiedImage`] to its wire [`Interface`].
//!
//! The identity, the transfer-graph law, and the canonical encoding live in
//! `marrow-image`; this feeds it the verified image's export, record, enum,
//! collection, and root facts. Every consumer — the runner's service, the terminal,
//! and the generated client — reconstructs the interface through this one function.

use marrow_image::{
    CollectionShape, EnumShape, ExportSignature, FieldShape, Interface, InterfaceError,
    RecordShape, RootShape, VariantShape,
};

use crate::sealed::{SealedCollectionType, VerifiedImage};

/// Reconstruct the wire interface from a verified image using only its sealed tables.
pub fn interface_of(image: &VerifiedImage) -> Result<Interface, InterfaceError> {
    let records: Vec<RecordShape> = image
        .record_types()
        .iter()
        .map(|record| RecordShape {
            fields: record
                .fields()
                .iter()
                .map(|field| FieldShape {
                    name: field.name().to_string(),
                    ty: field.ty(),
                    required: field.required(),
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
                    name: variant.name().to_string(),
                    category: variant.category(),
                    payload: variant.payload().to_vec(),
                })
                .collect(),
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
    let exports: Vec<ExportSignature> = image
        .exports()
        .iter()
        .map(|export| {
            let function = image
                .function(export.function())
                .expect("verified export function")
                .body();
            ExportSignature {
                id: export.id(),
                params: function.params().to_vec(),
                ret: function.ret(),
                demand_id: export.demand_id(),
            }
        })
        .collect();
    Interface::build(exports, &records, &enums, &collections, &roots)
}
