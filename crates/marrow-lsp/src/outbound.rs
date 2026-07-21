//! The closed outbound-frame owner and its one fallible concrete serialization seam.
//!
//! [`Outbound`] is a closed enum of everything the server sends: results, errors, and
//! the two server-initiated notifications (`publishDiagnostics`, `showMessage`). Each
//! variant is serialized once, through [`encode`], into a bounded immutable frame body.
//! There is no public generic `T: Serialize` surface, no `serde_json::Value`, no
//! `json!`, and no `to_value`/`to_string`/`to_vec`: the seam serializes a concrete
//! private envelope with `serde_json::to_writer` into a size-bounded sink.

use std::io::{self, Write};

use lsp_types::{InitializeResult, Location, PublishDiagnosticsParams, TextEdit};
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

use crate::capacities::MAX_OUTBOUND_FRAME_BYTES;
use crate::protocol::RequestId;

/// A window/showMessage severity. Only `ERROR` is used by H00a (a background capture
/// failure); the type is closed to what the server sends.
#[derive(Clone, Copy)]
pub enum MessageType {
    /// `MessageType.Error` (1).
    Error,
}

impl MessageType {
    fn code(self) -> i32 {
        match self {
            MessageType::Error => 1,
        }
    }
}

/// A closed outbound message. The coordinator constructs exactly one per acquired
/// outbound credit.
pub enum Outbound {
    /// An `initialize` result.
    Initialize {
        /// The request id.
        id: RequestId,
        /// The initialize result payload.
        result: Box<InitializeResult>,
    },
    /// A hover result (or null).
    Hover {
        /// The request id.
        id: RequestId,
        /// The hover payload, or `None` for a null result.
        result: Option<lsp_types::Hover>,
    },
    /// A definition result (or null).
    Definition {
        /// The request id.
        id: RequestId,
        /// The location, or `None` for a null result.
        result: Option<Location>,
    },
    /// A formatting result (edits, or null).
    Formatting {
        /// The request id.
        id: RequestId,
        /// The edits, or `None` for a null result.
        result: Option<Vec<TextEdit>>,
    },
    /// A null result (a `shutdown` acknowledgement).
    Null {
        /// The request id.
        id: RequestId,
    },
    /// A JSON-RPC error. A null id is the null-id protocol-error reply.
    Error {
        /// The request id, or `None` for a null-id reply.
        id: Option<RequestId>,
        /// The JSON-RPC error code.
        code: i32,
        /// The error message.
        message: String,
    },
    /// A `textDocument/publishDiagnostics` notification.
    PublishDiagnostics(Box<PublishDiagnosticsParams>),
    /// A `window/showMessage` notification.
    ShowMessage {
        /// The message type.
        typ: MessageType,
        /// The message body.
        message: String,
    },
}

/// Why encoding an outbound frame failed. Both are internal-error class before handoff:
/// no bytes are emitted.
#[derive(Debug)]
pub enum EncodeError {
    /// Serialization failed (a payload could not be encoded).
    Serialize,
    /// The encoded body exceeded [`MAX_OUTBOUND_FRAME_BYTES`].
    TooLarge,
}

/// Serialize one outbound message into a bounded immutable frame body. On any failure
/// no partial bytes escape: the returned error carries nothing and the caller emits
/// zero bytes.
pub fn encode(outbound: &Outbound) -> Result<Vec<u8>, EncodeError> {
    let mut sink = BoundedWriter::new(MAX_OUTBOUND_FRAME_BYTES);
    let result = match outbound {
        Outbound::Initialize { id, result } => write_result(&mut sink, id, result.as_ref()),
        Outbound::Hover { id, result } => write_result(&mut sink, id, result),
        Outbound::Definition { id, result } => write_result(&mut sink, id, result),
        Outbound::Formatting { id, result } => write_result(&mut sink, id, result),
        Outbound::Null { id } => write_result(&mut sink, id, &NullResult),
        Outbound::Error { id, code, message } => write_error(&mut sink, id.as_ref(), *code, message),
        Outbound::PublishDiagnostics(params) => {
            write_notification(&mut sink, "textDocument/publishDiagnostics", params.as_ref())
        }
        Outbound::ShowMessage { typ, message } => write_notification(
            &mut sink,
            "window/showMessage",
            &ShowMessageParams {
                typ: typ.code(),
                message,
            },
        ),
    };
    match result {
        Ok(()) => Ok(sink.into_inner()),
        Err(WriteError::TooLarge) => Err(EncodeError::TooLarge),
        Err(WriteError::Serialize) => Err(EncodeError::Serialize),
    }
}

/// The wire form of a JSON-RPC id: an integer or string, serialized transparently.
struct WireId<'a>(&'a RequestId);

impl Serialize for WireId<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            RequestId::Integer(value) => serializer.serialize_i32(*value),
            RequestId::Text(text) => serializer.serialize_str(text),
        }
    }
}

/// A concrete result envelope: `{jsonrpc, id, result}`. `P` is a concrete payload at
/// each call site — never a public generic surface.
struct ResultEnvelope<'a, P: Serialize> {
    id: &'a RequestId,
    result: &'a P,
}

impl<P: Serialize> Serialize for ResultEnvelope<'_, P> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("Response", 3)?;
        envelope.serialize_field("jsonrpc", "2.0")?;
        envelope.serialize_field("id", &WireId(self.id))?;
        envelope.serialize_field("result", self.result)?;
        envelope.end()
    }
}

/// The serialized `null` result.
struct NullResult;

impl Serialize for NullResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_none()
    }
}

/// A concrete error envelope: `{jsonrpc, id, error:{code,message}}`.
struct ErrorEnvelope<'a> {
    id: Option<&'a RequestId>,
    code: i32,
    message: &'a str,
}

impl Serialize for ErrorEnvelope<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("ErrorResponse", 3)?;
        envelope.serialize_field("jsonrpc", "2.0")?;
        match self.id {
            Some(id) => envelope.serialize_field("id", &WireId(id))?,
            None => envelope.serialize_field("id", &Option::<i32>::None)?,
        }
        envelope.serialize_field(
            "error",
            &WireError {
                code: self.code,
                message: self.message,
            },
        )?;
        envelope.end()
    }
}

#[derive(Serialize)]
struct WireError<'a> {
    code: i32,
    message: &'a str,
}

/// A concrete notification envelope: `{jsonrpc, method, params}`.
struct NotificationEnvelope<'a, P: Serialize> {
    method: &'a str,
    params: &'a P,
}

impl<P: Serialize> Serialize for NotificationEnvelope<'_, P> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("Notification", 3)?;
        envelope.serialize_field("jsonrpc", "2.0")?;
        envelope.serialize_field("method", self.method)?;
        envelope.serialize_field("params", self.params)?;
        envelope.end()
    }
}

#[derive(Serialize)]
struct ShowMessageParams<'a> {
    #[serde(rename = "type")]
    typ: i32,
    message: &'a str,
}

fn write_result<P: Serialize>(
    sink: &mut BoundedWriter,
    id: &RequestId,
    result: &P,
) -> Result<(), WriteError> {
    serde_json::to_writer(sink.by_ref(), &ResultEnvelope { id, result }).map_err(map_serde)?;
    sink.check()
}

fn write_error(
    sink: &mut BoundedWriter,
    id: Option<&RequestId>,
    code: i32,
    message: &str,
) -> Result<(), WriteError> {
    serde_json::to_writer(sink.by_ref(), &ErrorEnvelope { id, code, message }).map_err(map_serde)?;
    sink.check()
}

fn write_notification<P: Serialize>(
    sink: &mut BoundedWriter,
    method: &str,
    params: &P,
) -> Result<(), WriteError> {
    serde_json::to_writer(sink.by_ref(), &NotificationEnvelope { method, params })
        .map_err(map_serde)?;
    sink.check()
}

fn map_serde(error: serde_json::Error) -> WriteError {
    // A bounded-sink overflow surfaces as an io error through to_writer; a genuine
    // serialization defect is a data error. Distinguish so the too-large case is not
    // misreported as an internal serialize defect.
    if error.is_io() {
        WriteError::TooLarge
    } else {
        WriteError::Serialize
    }
}

enum WriteError {
    TooLarge,
    Serialize,
}

/// An `io::Write` sink that accepts at most `limit` bytes before failing, so an encoded
/// frame can never exceed the outbound bound. Overflow bytes are never retained.
struct BoundedWriter {
    buffer: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl BoundedWriter {
    fn new(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            limit,
            overflowed: false,
        }
    }

    fn check(&self) -> Result<(), WriteError> {
        if self.overflowed {
            Err(WriteError::TooLarge)
        } else {
            Ok(())
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.buffer
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buffer.len() + data.len() > self.limit {
            self.overflowed = true;
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        self.buffer.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(outbound: &Outbound) -> String {
        String::from_utf8(encode(outbound).unwrap()).unwrap()
    }

    #[test]
    fn null_result_encodes_id_and_null() {
        let text = body(&Outbound::Null {
            id: RequestId::Integer(4),
        });
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":4,"result":null}"#);
    }

    #[test]
    fn error_with_integer_id() {
        let text = body(&Outbound::Error {
            id: Some(RequestId::Integer(7)),
            code: -32601,
            message: "method not found".to_owned(),
        });
        assert_eq!(
            text,
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"method not found"}}"#
        );
    }

    #[test]
    fn error_with_null_id() {
        let text = body(&Outbound::Error {
            id: None,
            code: -32700,
            message: "parse error".to_owned(),
        });
        assert_eq!(
            text,
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}"#
        );
    }

    #[test]
    fn string_id_is_serialized_as_string() {
        let text = body(&Outbound::Null {
            id: RequestId::Text("abc".to_owned()),
        });
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":"abc","result":null}"#);
    }

    #[test]
    fn show_message_notification() {
        let text = body(&Outbound::ShowMessage {
            typ: MessageType::Error,
            message: "project.source_path: broken".to_owned(),
        });
        assert_eq!(
            text,
            r#"{"jsonrpc":"2.0","method":"window/showMessage","params":{"type":1,"message":"project.source_path: broken"}}"#
        );
    }

    #[test]
    fn hover_null_result() {
        let text = body(&Outbound::Hover {
            id: RequestId::Integer(1),
            result: None,
        });
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":1,"result":null}"#);
    }

    #[test]
    fn oversized_body_is_too_large_with_no_partial_bytes() {
        let huge = "x".repeat(MAX_OUTBOUND_FRAME_BYTES + 1);
        let error = encode(&Outbound::Error {
            id: None,
            code: -32603,
            message: huge,
        });
        assert!(matches!(error, Err(EncodeError::TooLarge)));
    }
}
