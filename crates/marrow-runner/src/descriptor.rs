//! The served program: its verified image and its wire interface identity.
//!
//! A [`Service`] pairs a verified image with its [`Id32`] interface identity, so
//! the handshake proves the served interface back to the client. Export resolution is
//! `dispatch::decode_request`, shared with the attached session; the image-to-interface
//! projection is `marrow_verify::interface_of`.

use marrow_image::InterfaceError;
use marrow_local_wire::Id32;
use marrow_verify::{VerifiedImage, interface_of};

/// The program a storeless runner serves: the verified image and its wire [`Id32`]
/// interface identity, built once at launch from the closed transfer graph, including
/// finite lists, ordered maps, and entry identities.
pub struct Service {
    pub(crate) image: VerifiedImage,
    interface_id: Id32,
}

impl Service {
    /// Build the service from a verified image, or report why its interface could
    /// not be reconstructed.
    pub fn build(image: VerifiedImage) -> Result<Service, InterfaceError> {
        let interface = interface_of(&image)?;
        let interface_id = Id32::from_bytes(*interface.interface_id().bytes());
        Ok(Service {
            image,
            interface_id,
        })
    }

    /// The wire identity of the served interface, proven back to the client in the
    /// handshake.
    pub fn interface_id(&self) -> Id32 {
        self.interface_id
    }
}
