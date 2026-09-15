//! The served program: its verified image, its wire interface identity, and the
//! export dispatch table.
//!
//! A [`Service`] pairs a verified image with its [`InterfaceId`] and a lookup from an
//! export's 32-byte identity to its function index and durable status, so a request
//! dispatches on a verified id alone. The image-to-interface projection is
//! `marrow_verify::interface_of`.

use marrow_image::InterfaceError;
use marrow_lifecycle::{PreparedImage, prepare};
use marrow_local_wire::Id32;
use marrow_verify::{FunctionIndex, VerifiedImage, interface_of};

/// One dispatchable export: its stable identity, its function index, and whether its
/// verified demand is durable (which the stock runner will not execute).
pub(crate) struct ServedExport {
    id: [u8; 32],
    func: FunctionIndex,
    durable: bool,
}

/// The program a runner serves: the prepared verified image, its wire [`Id32`] interface
/// identity, and the export dispatch table. Built once at launch from the closed
/// transfer graph, including finite lists, ordered maps, and entry identities. The
/// preparation is the sole owner of the image; a `Provision` request borrows it.
pub struct Service {
    pub(crate) image: PreparedImage,
    interface_id: Id32,
    exports: Vec<ServedExport>,
}

impl Service {
    /// Build the service from a verified image, or report why its interface could
    /// not be reconstructed. Prepares the image once, so a later provision derives nothing.
    pub fn build(image: VerifiedImage) -> Result<Service, InterfaceError> {
        let interface = interface_of(&image)?;
        let interface_id = Id32::from_bytes(*interface.interface_id().bytes());
        let exports = image
            .exports()
            .iter()
            .map(|export| ServedExport {
                id: *export.id().bytes(),
                func: export.function(),
                durable: !image
                    .function(export.function())
                    .expect("verified export function")
                    .demand()
                    .is_empty(),
            })
            .collect();
        Ok(Service {
            image: prepare(image),
            interface_id,
            exports,
        })
    }

    /// The wire identity of the served interface, proven back to the client in the
    /// handshake.
    pub fn interface_id(&self) -> Id32 {
        self.interface_id
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
