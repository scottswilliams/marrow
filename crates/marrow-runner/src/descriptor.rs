//! The served program: its verified image, its wire interface identity, and the
//! export dispatch table.
//!
//! [`interface_of`] is the one production projection from a [`VerifiedImage`] to its
//! [`Interface`] — the same reconstruction both real callers build the descriptor
//! set through (the terminal and, at the next slice, the generated client). A
//! [`Service`] pairs that image with its [`InterfaceId`] and a lookup from an
//! export's 32-byte identity to its function index and durable status, so a request
//! dispatches on a verified id alone.

use marrow_image::{
    EnumShape, ExportSignature, FieldShape, ImageType, Interface, InterfaceError, RecordShape,
    VariantShape,
};
use marrow_local_wire::Id32;
use marrow_verify::{FunctionIndex, RetShape, VerifiedImage};

/// Reconstruct the wire interface from a verified image using only its public
/// accessors. The identity, transfer-graph law, and canonical encoding live in
/// `marrow-image`; this is the thin projection that feeds it the image's export,
/// record, and enum facts.
pub fn interface_of(image: &VerifiedImage) -> Result<Interface, InterfaceError> {
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

/// Map a function's return shape to the bare-or-optional [`ImageType`] the interface
/// builder consumes.
pub(crate) fn ret_to_image(ret: RetShape) -> ImageType {
    match ret {
        RetShape::Unit => ImageType::Unit,
        RetShape::Scalar { scalar, optional } => ImageType::Scalar { scalar, optional },
        RetShape::Record { idx, optional } => ImageType::Record { idx, optional },
        RetShape::Enum { idx, optional } => ImageType::Enum { idx, optional },
        RetShape::Collection { idx, optional } => ImageType::Collection { idx, optional },
        RetShape::Identity { root, optional } => ImageType::Identity { root, optional },
    }
}

/// One dispatchable export: its stable identity, its function index, and whether its
/// verified demand is durable (which the stock runner will not execute).
pub(crate) struct ServedExport {
    id: [u8; 32],
    func: FunctionIndex,
    durable: bool,
}

/// The program a runner serves: a verified image, its wire [`Id32`] interface
/// identity, and the export dispatch table. Built once at launch; an image whose
/// interface cannot be reconstructed (a signature reaches a collection, excluded
/// until G00b) is not servable.
pub struct Service {
    pub(crate) image: VerifiedImage,
    interface_id: Id32,
    exports: Vec<ServedExport>,
}

impl Service {
    /// Build the service from a verified image, or report why its interface could
    /// not be reconstructed.
    pub fn build(image: VerifiedImage) -> Result<Service, InterfaceError> {
        let interface = interface_of(&image)?;
        let interface_id = Id32::from_bytes(*interface.interface_id().bytes());
        let exports = image
            .exports()
            .iter()
            .map(|export| ServedExport {
                id: *export.id().bytes(),
                func: export.function(),
                durable: !export.demand().is_empty(),
            })
            .collect();
        Ok(Service {
            image,
            interface_id,
            exports,
        })
    }

    /// The wire identity of the served interface, proven back to the client in the
    /// handshake.
    pub fn interface_id(&self) -> Id32 {
        self.interface_id
    }

    /// The verified image this service serves. The attached session reads it to dispatch a
    /// durable export against the native store.
    pub(crate) fn image(&self) -> &VerifiedImage {
        &self.image
    }

    /// The dispatch entry for an export identity, if the image carries it.
    pub(crate) fn lookup(&self, id: &[u8; 32]) -> Option<&ServedExport> {
        self.exports.iter().find(|export| &export.id == id)
    }
}

impl ServedExport {
    pub(crate) fn func(&self) -> FunctionIndex {
        self.func
    }
    pub(crate) fn is_durable(&self) -> bool {
        self.durable
    }
}
