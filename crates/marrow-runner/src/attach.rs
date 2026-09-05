//! The native attached session: the runner side of the persistent terminal path.
//!
//! Where the storeless [`Service`] serves an image's storeless exports over the channel, an
//! attached session serves the durable exports of the image the privileged lifecycle actor
//! ([`marrow_lifecycle::attach`]) admitted against a persistent store — the
//! [`NativeAttachment`] that pairs exactly that image with exactly that store. Each request
//! opens one durable session bounded by `demand ∩ ceiling ∩ grant`. A mutating export
//! commits its own `transaction` region to the store; a read-only export observes a coherent
//! view; a committed write is durable across a restart. The attachment keeps the store's
//! single-owner lock for the session's whole life, so no second process can bind the same
//! store concurrently.
//!
//! The CLI never opens the store: `marrow run … --store` spawns this attached session and
//! speaks the wire protocol to it, so the lifecycle state lives only behind this crate's
//! privileged boundary.
//!
//! Unlike the storeless [`Service`](crate::Service), the attached session pins the exact
//! **image identity** ([`VerifiedImage::image_id`](marrow_verify::VerifiedImage::image_id))
//! as its handshake identity rather than the transfer-graph interface identity. The terminal
//! shares the exact image bytes it spawned the runner with, so it verifies that identity
//! directly — a stronger binding than interface shape, and one that works for any program,
//! including one with a non-transferable export (an entry-identity return, a collection) that
//! has no whole-program wire interface. The per-call transfer codec still governs each
//! argument and return value, so a call to a non-transferable export fails closed at encode
//! time rather than being served partially.
//!
//! The service takes only the attachment the lifecycle actor returned; no image travels
//! beside it, so a foreign image cannot be served against an admitted store.
//!
//! ```compile_fail
//! fn foreign_pair(
//!     image: marrow_verify::VerifiedImage,
//!     attachment: marrow_lifecycle::NativeAttachment,
//! ) -> marrow_runner::AttachedService {
//!     marrow_runner::AttachedService::new(image, attachment)
//! }
//! ```

use marrow_codes::Code;
use marrow_lifecycle::NativeAttachment;
use marrow_local_wire::{ClientMessage, Json, ServerMessage};
use marrow_vm::{DurableExecutionFault, DurableRun, IncompleteDisposition};

use crate::channel::Handler;
use crate::dispatch;

/// A live attached session: the admitted image paired with the open persistent store, holding
/// the store's single-owner lock. Built once at attach; each request opens its own durable
/// session against the store. Retired (`None`) once an invocation's durable outcome is
/// unknown, after which the channel closes.
pub struct AttachedService {
    attachment: Option<NativeAttachment>,
    close_after_response: bool,
}

impl AttachedService {
    /// Serve the attachment the lifecycle actor returned.
    pub fn new(attachment: NativeAttachment) -> Self {
        Self {
            attachment: Some(attachment),
            close_after_response: false,
        }
    }
}

impl Handler for AttachedService {
    /// Serve one request against the attached store. `Hello` after the handshake and
    /// `Provision` (a separate one-shot command, never a mid-session operation) are protocol
    /// rejects; a `Request` dispatches to the image's export against a fresh durable session.
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

impl AttachedService {
    fn handle_request(&mut self, export_id: &[u8; 32], args: &[Json]) -> ServerMessage {
        // A retired session has already asked the channel to close; a request that still
        // arrives is outside the protocol.
        let Some(attachment) = self.attachment.as_mut() else {
            return dispatch::reject(Code::RunnerHandshake);
        };
        let decoded = match dispatch::decode_request(attachment.image(), export_id, args) {
            Ok(decoded) => decoded,
            Err(reject) => return reject,
        };
        // A storeless export needs no session; a durable one runs against the native store
        // through the same attachment seam the ephemeral session uses.
        if let dispatch::Route::Storeless = decoded.route {
            return dispatch::run_storeless(attachment.image(), decoded.export, decoded.values);
        }
        let run = marrow_vm::run_export(attachment, decoded.export, decoded.values);
        match run {
            Some(DurableRun::Ran(Err(DurableExecutionFault::Incomplete(incomplete)))) => {
                match incomplete.into_disposition() {
                    IncompleteDisposition::Classified { fault, durable } => {
                        if durable == marrow_vm::DurableCommitState::Unknown {
                            self.attachment.take();
                            self.close_after_response = true;
                        }
                        dispatch::incomplete_message(&fault, durable)
                    }
                    IncompleteDisposition::Pending { fault, recovery } => {
                        let attachment = self
                            .attachment
                            .take()
                            .expect("pending recovery owns the live attachment");
                        let (durable, recovered) = attachment.resolve_recovery(recovery);
                        self.attachment = recovered;
                        self.close_after_response =
                            durable == marrow_vm::DurableCommitState::Unknown;
                        dispatch::incomplete_message(&fault, durable)
                    }
                }
            }
            run => match dispatch::project_durable_run(attachment.image(), run) {
                dispatch::RunProjection::Reply(response) => response,
                dispatch::RunProjection::RetireAfter(response) => {
                    self.attachment.take();
                    self.close_after_response = true;
                    response
                }
            },
        }
    }
}
