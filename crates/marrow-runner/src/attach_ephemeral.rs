//! The ephemeral-memory attached session: the runner side of an isolated in-memory durable run.
//!
//! Where the native attached session ([`crate::AttachedService`]) serves an image the lifecycle
//! admitted against a persistent store, this session serves the image over a fresh
//! process-local in-memory store the lifecycle minted from the image's own projection — the
//! same [`MemoryAttachment`] the source-test runner uses in process, here served over the wire.
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
//! ([`VerifiedImage::image_id`](marrow_verify::VerifiedImage::image_id)) — the client recomputes
//! it independently from the bytes it spawned the runner with — and the per-call transfer codec
//! governs each argument and return value, so a call to a non-transferable export fails closed
//! at encode time.
//!
//! [`MemoryAttachment`]: marrow_lifecycle::MemoryAttachment

use marrow_codes::Code;
use marrow_lifecycle::{EphemeralOutcome, PreparedImage, mint_ephemeral};
use marrow_local_wire::{ClientMessage, Id32, Json, ServerMessage};

use crate::channel::Handler;
use crate::dispatch;

/// A live ephemeral-memory attached session: the mint outcome, which owns the served image in
/// every arm, held across every request of one session and discarded with the process.
pub struct AttachedEphemeralService {
    outcome: EphemeralOutcome,
    close_after_response: bool,
}

impl AttachedEphemeralService {
    /// Mint the in-memory attachment for the prepared image. Called by the runner only after
    /// the client's handshake succeeds, so the store never opens for an unauthenticated peer. A
    /// durable image yields a ready attachment; a storeless or not-yet-executable image yields
    /// a session that rejects every durable request typed while still running the image's
    /// storeless exports.
    pub fn mint(prepared: PreparedImage) -> Self {
        Self {
            outcome: mint_ephemeral(prepared),
            close_after_response: false,
        }
    }

    /// The handshake identity the runner proves back: the exact image identity, which the client
    /// independently recomputes from the bytes it spawned the runner with.
    pub fn identity(&self) -> Id32 {
        Id32::from_bytes(self.outcome.image().image_id().0)
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
        let decoded = match dispatch::decode_request(self.outcome.image(), export_id, args) {
            Ok(decoded) => decoded,
            Err(reject) => return reject,
        };
        // A storeless export needs no session, so a parked or failed mint still serves it; a
        // durable one runs against the in-memory store through the same attachment seam the
        // native session uses.
        if !decoded.durable {
            return dispatch::run_storeless(self.outcome.image(), decoded.export, decoded.values);
        }
        let projection = match &mut self.outcome {
            EphemeralOutcome::Ready(attachment) => {
                let run = marrow_vm::run_export(attachment, decoded.export, decoded.values);
                dispatch::project_durable_run(attachment.image(), run)
            }
            // A durable request against an image whose shape is not yet executable, or whose
            // attachment could not be minted, is a typed reject — never a partial reply.
            EphemeralOutcome::Parked(_) => {
                dispatch::RunProjection::Reply(dispatch::reject(Code::RunnerDurableUnsupported))
            }
            EphemeralOutcome::Failed { cause, .. } => {
                dispatch::RunProjection::Reply(ServerMessage::Reject {
                    code: cause.to_string(),
                })
            }
        };
        match projection {
            dispatch::RunProjection::Reply(response) => response,
            dispatch::RunProjection::RetireAfter(response) => {
                self.close_after_response = true;
                response
            }
        }
    }
}
