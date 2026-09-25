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
//! directly. Argument decoding and returned-value encoding still use that image's
//! verified transfer types, including collections and entry identities. Outbound
//! framing finishes while the returned value and its image remain borrowed.
//!
//! The service takes only the attachment the lifecycle actor returned — no image travels
//! beside it, so a foreign image cannot be served against an admitted store:
//!
//! ```compile_fail
//! fn foreign_pair(
//!     image: marrow_verify::VerifiedImage,
//!     attachment: marrow_lifecycle::NativeAttachment,
//! ) -> marrow_runner::AttachedService {
//!     marrow_runner::AttachedService::new(image, attachment)
//! }
//! ```

use marrow_codes::{Code, DurableCommitState};
use marrow_lifecycle::NativeAttachment;
use marrow_local_wire::{ClientMessage, EncodedFrame, Json, WireError};
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
    /// Serve one request against the attached store. `Hello` after the handshake is a
    /// protocol reject; a `Request` dispatches to the image's export against a fresh durable
    /// session.
    fn handle(
        &mut self,
        message: ClientMessage,
        turn: Option<u32>,
    ) -> Result<EncodedFrame, WireError> {
        let turn = turn.unwrap_or(0);
        match message {
            ClientMessage::Hello { .. } => {
                dispatch::reject(Code::RunnerHandshake).encode_frame(turn)
            }
            ClientMessage::Request { export, args } => {
                self.handle_request(export.bytes(), &args, turn)
            }
        }
    }

    fn close_after_response(&self) -> bool {
        self.close_after_response
    }
}

impl AttachedService {
    fn handle_request(
        &mut self,
        export_id: &[u8; 32],
        args: &[Json],
        turn: u32,
    ) -> Result<EncodedFrame, WireError> {
        // A retired session has already asked the channel to close; a request that still
        // arrives is outside the protocol.
        let Some(attachment) = self.attachment.as_mut() else {
            return dispatch::reject(Code::RunnerHandshake).encode_frame(turn);
        };
        let decoded = match dispatch::decode_request(attachment.image(), export_id, args) {
            Ok(decoded) => decoded,
            Err(reject) => return reject.encode_frame(turn),
        };
        if let dispatch::Route::Storeless = decoded.route {
            return dispatch::run_storeless(
                attachment.image(),
                decoded.export,
                decoded.values,
                turn,
            );
        }
        let Some(run) = marrow_vm::run_export(attachment, decoded.export, decoded.values) else {
            return dispatch::reject(Code::RunnerUnknownExport).encode_frame(turn);
        };
        match run {
            DurableRun::Ran(Ok(value)) => {
                dispatch::value_frame(attachment.image(), value.as_ref(), turn)
            }
            DurableRun::Ran(Err(DurableExecutionFault::Runtime(fault))) => {
                dispatch::fault_message(&fault).encode_frame(turn)
            }
            DurableRun::Ran(Err(DurableExecutionFault::Incomplete(incomplete))) => {
                let (fault, durable) = self.classify(incomplete.into_disposition());
                if durable == DurableCommitState::Unknown {
                    self.attachment.take();
                    self.close_after_response = true;
                }
                dispatch::incomplete_message(&fault, durable).encode_frame(turn)
            }
            DurableRun::Parked => {
                dispatch::reject(Code::RunnerDurableUnsupported).encode_frame(turn)
            }
            DurableRun::Failed(code) => dispatch::reject(code).encode_frame(turn),
        }
    }

    /// The one durable-state classification for an incomplete invocation. A pending commit
    /// recovery is resolved by the attachment itself, which alone may consume that fact; an
    /// attachment the resolution could not keep is retired here.
    fn classify(
        &mut self,
        disposition: IncompleteDisposition,
    ) -> (marrow_vm::RuntimeFault, DurableCommitState) {
        match disposition {
            IncompleteDisposition::Classified { fault, durable } => (fault, durable),
            IncompleteDisposition::Pending { fault, recovery } => {
                let attachment = self
                    .attachment
                    .take()
                    .expect("pending recovery owns the live attachment");
                let (durable, recovered) = attachment.resolve_recovery(recovery);
                self.attachment = recovered;
                (fault, durable)
            }
        }
    }
}
