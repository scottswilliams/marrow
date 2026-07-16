//! Request dispatch: one [`ClientMessage`] to one [`ServerMessage`].
//!
//! A request names an export by identity and carries its JSON arguments. Dispatch
//! rejects an unknown export, a durable export (storeless only on this beta line),
//! and an argument set that does not match the verified signature; otherwise it
//! decodes the arguments, runs the export on the VM, and encodes the result. The
//! four outcomes map onto the closed response grammar: a value, a source-mapped
//! fault, or a typed reject.

use marrow_codes::Code;
use marrow_local_wire::{ClientMessage, Json, ServerMessage, Span};

use crate::descriptor::Service;
use crate::transfer;

impl Service {
    /// Produce the response to a client message. A `Hello` after the handshake is a
    /// protocol error, not a second handshake.
    pub fn handle(&self, message: ClientMessage) -> ServerMessage {
        match message {
            ClientMessage::Hello { .. } => reject(Code::RunnerHandshake),
            ClientMessage::Request { export, args } => self.handle_request(export.bytes(), &args),
        }
    }

    fn handle_request(&self, export: &[u8; 32], args: &[Json]) -> ServerMessage {
        let Some(served) = self.lookup(export) else {
            return reject(Code::RunnerUnknownExport);
        };
        if served.is_durable() {
            return reject(Code::RunnerDurableUnsupported);
        }
        let function = self.image.function(served.func());
        if function.params().len() != args.len() {
            return reject(Code::RunnerArgMismatch);
        }
        let mut values = Vec::with_capacity(args.len());
        for (ty, json) in function.params().iter().zip(args) {
            match transfer::decode_arg(&self.image, ty, json) {
                Some(value) => values.push(value),
                None => return reject(Code::RunnerArgMismatch),
            }
        }
        match marrow_vm::run(&self.image, served.func(), values) {
            Ok(None) => ServerMessage::Value { data: Json::Null },
            Ok(Some(value)) => match transfer::encode_value(&self.image, &value) {
                Some(data) => ServerMessage::Value { data },
                // Unreachable for a served export: its return shape is transferable,
                // so its value encodes. Fail closed rather than emit a partial reply.
                None => reject(Code::RunnerReplyEncode),
            },
            Err(fault) => ServerMessage::Fault {
                code: fault.code().to_string(),
                span: Span {
                    line: fault.line(),
                    column: fault.column(),
                },
            },
        }
    }
}

fn reject(code: Code) -> ServerMessage {
    ServerMessage::Reject {
        code: code.as_str().to_string(),
    }
}
