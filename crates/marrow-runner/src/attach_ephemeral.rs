//! The ephemeral-memory attached session: the runner side of an isolated in-memory durable run.
//!
//! Where the native attached session ([`crate::AttachedService`]) binds an image to a persistent
//! store, this session binds the image to a fresh process-local in-memory store — the same
//! [`EphemeralAttachment`] the source-test runner uses in process, here served over the wire.
//! Each request opens one durable session against that store; a committed `transaction` region
//! is observable by a later request *in this same session*, and the whole store is discarded
//! when the process exits. There is no persistence, no store lock, and no admission: the store
//! never survives the runner, so a committed write is durable only within the live session.
//!
//! The attachment is minted by [`Self::mint`], which the runner calls **after** the handshake
//! completes — an unauthenticated peer never causes the in-memory store to open (the
//! `hello`-before-attachment ordering the channel enforces by constructing the handler only once
//! a client has proven the launch nonce).
//!
//! Like the native session, the handshake identity is the exact **image identity**
//! ([`VerifiedImage::image_id`]) — the client recomputes it independently from the bytes it
//! spawned the runner with — and the per-call transfer codec governs each argument and return
//! value, so a call to a non-transferable export fails closed at encode time.
//!
//! [`EphemeralAttachment`]: marrow_kernel::durable::EphemeralAttachment

use marrow_codes::Code;
use marrow_local_wire::{ClientMessage, Id32, Json, ServerMessage};
use marrow_verify::VerifiedImage;
use marrow_vm::Ephemeral;

use crate::channel::Handler;
use crate::dispatch;

/// A live ephemeral-memory attached session: the served image and its process-local in-memory
/// attachment, held across every request of one session and discarded with the process.
pub struct AttachedEphemeralService {
    image: VerifiedImage,
    attachment: Ephemeral,
    close_after_response: bool,
}

impl AttachedEphemeralService {
    /// Mint the in-memory attachment for `image`. Called by the runner only after the client's
    /// handshake succeeds, so the store never opens for an unauthenticated peer. A durable image
    /// yields a ready attachment; a storeless or not-yet-executable image yields a session that
    /// rejects every durable request typed while still running the image's storeless exports.
    pub fn mint(image: VerifiedImage) -> Self {
        let attachment = marrow_vm::mint_ephemeral(&image);
        Self {
            image,
            attachment,
            close_after_response: false,
        }
    }

    /// The handshake identity the runner proves back: the exact image identity, which the client
    /// independently recomputes from the bytes it spawned the runner with.
    pub fn identity(&self) -> Id32 {
        Id32::from_bytes(self.image.image_id().0)
    }
}

impl Handler for AttachedEphemeralService {
    /// Serve one request against the in-memory attachment. `Hello` after the handshake and
    /// `Provision` (an ephemeral store is never provisioned) are protocol rejects; a `Request`
    /// dispatches to the image's export against a session on the held attachment.
    fn handle(&mut self, message: ClientMessage) -> ServerMessage {
        match message {
            ClientMessage::Hello { .. } => dispatch::reject(Code::RunnerHandshake),
            ClientMessage::Provision { .. } => dispatch::reject(Code::RunnerHandshake),
            ClientMessage::Request { export, args } => self.handle_request(export.bytes(), &args),
        }
    }

    fn close_after_response(&self) -> bool {
        self.close_after_response
    }
}

impl AttachedEphemeralService {
    fn handle_request(&mut self, export_id: &[u8; 32], args: &[Json]) -> ServerMessage {
        // Split the disjoint borrows: the image is read while the attachment opens a session.
        let Self {
            image,
            attachment,
            close_after_response,
        } = self;
        let (export, values) = match dispatch::decode_request(image, export_id, args) {
            Ok(decoded) => decoded,
            Err(reject) => return reject,
        };
        // A storeless export needs no session; a durable one runs against the in-memory store
        // through the same session machinery the native attachment uses.
        if export.demand().is_empty() {
            return dispatch::run_storeless(image, export, values);
        }
        let projection = match attachment {
            Ephemeral::Ready(store) => dispatch::project_durable_run(
                image,
                marrow_vm::run_export(image, store, export, values),
            ),
            // A durable request against an image whose shape is not yet executable, or whose
            // attachment could not be minted, is a typed reject — never a partial reply.
            Ephemeral::Parked => {
                dispatch::RunProjection::Reply(dispatch::reject(Code::RunnerDurableUnsupported))
            }
            Ephemeral::Failed(code) => dispatch::RunProjection::Reply(ServerMessage::Reject {
                code: code.to_string(),
            }),
        };
        match projection {
            dispatch::RunProjection::Reply(response) => response,
            dispatch::RunProjection::RetireAfter(response) => {
                *close_after_response = true;
                response
            }
        }
    }
}
