//! Storeless request dispatch over a launched image.
//!
//! A request resolves through the shared export classifier; a storeless export runs on
//! the VM with no session, and a durable export is a typed reject because this service
//! opens no store. Successful values are encoded while borrowed; a completed frame or an
//! encoding error leaves this owner.

use marrow_codes::Code;
use marrow_local_wire::{ClientMessage, EncodedFrame, Json, WireError};

use crate::channel::Handler;
use crate::descriptor::Service;
use crate::dispatch;

impl Handler for Service {
    /// Produce a complete response while borrowed results are live. A `Hello`
    /// after the handshake is a protocol error, not a second handshake.
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
}

impl Service {
    fn handle_request(
        &self,
        export: &[u8; 32],
        args: &[Json],
        turn: u32,
    ) -> Result<EncodedFrame, WireError> {
        let image = &self.image;
        let resolved = match dispatch::resolve_export(image, export) {
            Ok(resolved) => resolved,
            Err(reject) => return reject.encode_frame(turn),
        };
        // The route is refused before the arguments are examined, so a durable export is
        // unsupported here whatever its arguments look like.
        if let dispatch::Route::Durable = resolved.route {
            return dispatch::reject(Code::RunnerDurableUnsupported).encode_frame(turn);
        }
        match dispatch::decode_args(image, resolved.function, args) {
            Ok(values) => dispatch::run_storeless(image, resolved.export, values, turn),
            Err(reject) => reject.encode_frame(turn),
        }
    }
}
