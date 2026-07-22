//! A handler that serves nothing but one typed refusal.

use marrow_local_wire::{ClientMessage, ServerMessage};

use crate::channel::Handler;

/// A [`Handler`](crate::Handler) that answers every request with one typed
/// [`ServerMessage::Reject`] carrying a fixed code.
///
/// The native `attach` path serves this — after binding the channel and completing the
/// handshake, but *without opening the store* — when the lifecycle actor refuses the presented
/// image before any engine call (a demand-exceeds-ceiling authority refusal). The terminal then
/// receives a typed [`CallOutcome::Reject`](crate::CallOutcome::Reject) it renders as an ordinary
/// run outcome, rather than a spawn/descriptor failure. Binding a socket and proving the
/// handshake open no store, so a refusal served this way makes zero engine calls; the full
/// source-vocabulary refusal sentence is written to the runner's stderr, the byte-log pipe the
/// trusted main owns.
pub struct RefusalService {
    code: String,
}

impl RefusalService {
    /// A refusal service that rejects every request with `code`.
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl Handler for RefusalService {
    fn handle(&mut self, _message: ClientMessage) -> ServerMessage {
        ServerMessage::Reject {
            code: self.code.clone(),
        }
    }
}
