//! The `marrow serve` request/response protocol.
//!
//! Each request and reply is one JSON object on its own line. This module is
//! the debug/admin adapter: [`ProtocolSession`] turns request values into reply
//! values over shared checked tooling facts.

mod codec;
mod cursor;
mod data;
mod walk;

use marrow_check::CheckedProgram;
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};

/// A request was malformed: not an object, or missing a string `op`.
pub(crate) const PROTOCOL_MALFORMED: &str = "protocol.malformed";
/// A request named an operation the server does not support.
pub(crate) const PROTOCOL_UNKNOWN_OP: &str = "protocol.unknown_op";
/// A known operation received malformed arguments.
pub(crate) const PROTOCOL_BAD_REQUEST: &str = "protocol.bad_request";
/// The store has evolved past the schema this serve binary was checked against, so
/// a data op cannot render its data under the stale schema.
pub(crate) const PROTOCOL_STALE_EPOCH: &str = "protocol.stale_epoch";

/// Error types derive `Debug` by convention; production reports the error through
/// its wire envelope (`code`/`message`), never its `Debug` shape.
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
        let name = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| ProtocolError {
                code: PROTOCOL_MALFORMED,
                message: "request is missing a string `op`".to_string(),
            })?;
        let op = Op::parse(name);
        // A data op reads saved data, so the stale-epoch gate and the dispatcher both
        // derive from the parsed op rather than re-matching the operation names.
        if op.reads_data() && self.stale_epoch {
            return Err(ProtocolError {
                code: PROTOCOL_STALE_EPOCH,
                message:
                    "the store has evolved past the schema this server was checked against; restart \
                     `marrow serve` to read the evolved data"
                        .to_string(),
            });
        }
        match op {
            Op::DataRoots => data::op_debug_data_roots(program, store),
            Op::DataGet => data::op_debug_data_get(program, store, request),
            Op::DataChildren => {
                data::op_debug_data_children(program, store, request, &self.cursors)
            }
            Op::DataWalk => walk::op_debug_data_walk(program, store, request, &self.cursors),
            Op::Other(other) => Err(ProtocolError {
                code: PROTOCOL_UNKNOWN_OP,
                message: format!("unknown operation `{other}`"),
            }),
        }
    }
}

/// A parsed protocol operation. The data ops read saved data and so are gated by the
/// stale-epoch check; `Other` carries the unrecognized name for its error reply. The
/// one source of truth for which operations exist and which read data.
enum Op<'a> {
    DataRoots,
    DataGet,
    DataChildren,
    DataWalk,
    Other(&'a str),
}

impl<'a> Op<'a> {
    fn parse(name: &'a str) -> Self {
        match name {
            "debug_data_roots" => Self::DataRoots,
            "debug_data_get" => Self::DataGet,
            "debug_data_children" => Self::DataChildren,
            "debug_data_walk" => Self::DataWalk,
            other => Self::Other(other),
        }
    }

    fn reads_data(&self) -> bool {
        matches!(
            self,
            Self::DataRoots | Self::DataGet | Self::DataChildren | Self::DataWalk
        )
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

/// Surface a shared tooling failure on the wire. A malformed query is a
/// client-facing bad request; a store fault keeps the store code so a corrupt
/// store is never disguised as a client error.
pub(super) fn tooling_error(error: marrow_check::tooling::ToolingError) -> ProtocolError {
    match error {
        marrow_check::tooling::ToolingError::Query(error) => bad_request(&error.to_string()),
        marrow_check::tooling::ToolingError::Store(error) => store_error(error),
    }
}

#[cfg(test)]
mod tests;
