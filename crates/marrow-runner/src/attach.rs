//! The native attached session: the runner side of the persistent terminal path.
//!
//! Where the storeless [`Service`] serves an image's storeless exports over the channel, an
//! attached session binds a verified image to a persistent store through the privileged
//! lifecycle actor ([`marrow_lifecycle::attach`]) and serves the image's *durable* exports
//! against that store — each request opening exactly one durable session bounded by
//! `demand ∩ ceiling ∩ grant`. A mutating export commits its own `transaction` region to the
//! store; a read-only export observes a coherent view; a committed write is durable across a
//! restart. The held [`OpenStore`] keeps the store's single-owner lock for the session's
//! whole life, so no second process can bind the same store concurrently.
//!
//! The CLI never opens the store: `marrow run … --store` spawns this attached session and
//! speaks the wire protocol to it, so the lifecycle state lives only behind this crate's
//! privileged boundary.
//!
//! Unlike the storeless [`Service`](crate::Service), the attached session pins the exact
//! **image identity** ([`VerifiedImage::image_id`]) as its handshake identity rather than the
//! transfer-graph interface identity. The terminal shares the exact image bytes it spawned
//! the runner with, so it verifies that identity directly — a stronger binding than interface
//! shape, and one that works for any program, including one with a non-transferable export
//! (an entry-identity return, a collection) that has no whole-program wire interface. The
//! per-call transfer codec still governs each argument and return value, so a call to a
//! non-transferable export fails closed at encode time rather than being served partially.

use marrow_codes::Code;
use marrow_lifecycle::OpenStore;
use marrow_local_wire::{ClientMessage, Id32, Json, ServerMessage};
use marrow_verify::VerifiedImage;
use marrow_vm::{DurableExecutionFault, DurableRun, IncompleteDisposition};

use crate::channel::Handler;
use crate::dispatch;

/// A live attached session: the served program image and the open persistent store, holding
/// the store's single-owner lock. Built once at attach; each request opens its own durable
/// session against the store.
pub struct AttachedService {
    image: VerifiedImage,
    open: Option<OpenStore>,
    close_after_response: bool,
}

impl AttachedService {
    /// Bind `image` to the already-open `store`.
    pub fn new(image: VerifiedImage, open: OpenStore) -> Self {
        Self {
            image,
            open: Some(open),
            close_after_response: false,
        }
    }

    /// The handshake identity the runner proves back: the exact image identity, which the
    /// terminal independently recomputes from the bytes it spawned the runner with.
    pub fn identity(&self) -> Id32 {
        Id32::from_bytes(self.image.image_id().0)
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
        let (export, values) = match dispatch::decode_request(&self.image, export_id, args) {
            Ok(decoded) => decoded,
            Err(reject) => return reject,
        };
        // A storeless export needs no session; a durable one runs against the native store
        // through the same session machinery the ephemeral attachment uses.
        if export.demand().is_empty() {
            return dispatch::run_storeless(&self.image, export, values);
        }
        let run = {
            let open = self
                .open
                .as_mut()
                .expect("a live attached session retains its store owner");
            marrow_vm::run_export(&self.image, open, export, values)
        };
        match run {
            DurableRun::Ran(Err(DurableExecutionFault::Incomplete(incomplete))) => match incomplete
                .into_disposition()
            {
                IncompleteDisposition::Classified { fault, durable } => {
                    if durable == marrow_vm::DurableCommitState::Unknown {
                        self.open.take();
                        self.close_after_response = true;
                    }
                    dispatch::incomplete_message(&fault, durable)
                }
                IncompleteDisposition::Pending { fault, recovery } => {
                    let open = self
                        .open
                        .take()
                        .expect("pending recovery owns the live attached store");
                    let (durable, recovered_open) = open.resolve_recovery(recovery);
                    self.open = recovered_open;
                    self.close_after_response = durable == marrow_vm::DurableCommitState::Unknown;
                    dispatch::incomplete_message(&fault, durable)
                }
            },
            run => match dispatch::project_durable_run(&self.image, run) {
                dispatch::RunProjection::Reply(response) => response,
                dispatch::RunProjection::RetireAfter(response) => {
                    self.open.take();
                    self.close_after_response = true;
                    response
                }
            },
        }
    }
}
