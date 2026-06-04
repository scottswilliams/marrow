//! The `marrow serve` request/response protocol.
//!
//! Each request and reply is one JSON object on its own line. This module is
//! the transport-free core: [`ProtocolSession`] turns request values into reply
//! values against checked tree-cell data.

mod codec;
mod cursor;
mod data;
mod walk;

use marrow_check::CheckedProgram;
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};

/// A request was malformed: not an object, or missing a string `op`.
pub const PROTOCOL_MALFORMED: &str = "protocol.malformed";
/// A request named an operation the server does not support.
pub const PROTOCOL_UNKNOWN_OP: &str = "protocol.unknown_op";
/// A known operation received malformed arguments.
pub const PROTOCOL_BAD_REQUEST: &str = "protocol.bad_request";
/// The store has evolved past the schema this serve binary was checked against, so
/// a data op cannot render its data under the stale schema.
pub const PROTOCOL_STALE_EPOCH: &str = "protocol.stale_epoch";

#[derive(Debug)]
pub(super) struct ProtocolError {
    code: &'static str,
    message: String,
}

pub(super) struct ProtocolSession {
    cursors: cursor::CursorState,
    /// The store has evolved past the schema this serve binary was checked against,
    /// fixed once per connection from the pinned snapshot. While set, every data op
    /// refuses rather than rendering evolved data under the stale schema.
    stale_epoch: bool,
}

impl ProtocolSession {
    pub(super) fn new(stale_epoch: bool) -> Self {
        Self {
            cursors: cursor::CursorState::new(),
            stale_epoch,
        }
    }

    pub(super) fn handle_request(
        &self,
        program: &CheckedProgram,
        store: &TreeStore,
        request: &Value,
    ) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        match self.dispatch(program, store, request) {
            Ok(result) => json!({ "id": id, "ok": result }),
            Err(error) => json!({
                "id": id,
                "error": { "code": error.code, "message": error.message },
            }),
        }
    }

    fn dispatch(
        &self,
        program: &CheckedProgram,
        store: &TreeStore,
        request: &Value,
    ) -> Result<Value, ProtocolError> {
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| ProtocolError {
                code: PROTOCOL_MALFORMED,
                message: "request is missing a string `op`".to_string(),
            })?;
        let is_data_op = matches!(
            op,
            "debug_data_roots" | "debug_data_get" | "debug_data_children" | "debug_data_walk"
        );
        if is_data_op && self.stale_epoch {
            return Err(ProtocolError {
                code: PROTOCOL_STALE_EPOCH,
                message:
                    "the store has evolved past the schema this server was checked against; restart \
                     `marrow serve` to read the evolved data"
                        .to_string(),
            });
        }
        match op {
            "debug_data_roots" => data::op_debug_data_roots(program, store),
            "debug_data_get" => data::op_debug_data_get(program, store, request),
            "debug_data_children" => {
                data::op_debug_data_children(program, store, request, &self.cursors)
            }
            "debug_data_walk" => walk::op_debug_data_walk(program, store, request, &self.cursors),
            other => Err(ProtocolError {
                code: PROTOCOL_UNKNOWN_OP,
                message: format!("unknown operation `{other}`"),
            }),
        }
    }
}

pub(super) fn bad_request(message: &str) -> ProtocolError {
    ProtocolError {
        code: PROTOCOL_BAD_REQUEST,
        message: message.to_string(),
    }
}

pub(super) fn store_error(error: marrow_store::StoreError) -> ProtocolError {
    ProtocolError {
        code: error.code(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
