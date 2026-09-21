//! The closed outbound-frame owner and its one fallible concrete serialization seam.
//!
//! [`Outbound`] is a closed enum of everything the server sends: results, errors, and
//! the two server-initiated notifications (`publishDiagnostics`, `showMessage`). Each
//! variant is serialized once, through [`encode`], into a bounded immutable frame body.
//! There is no public generic `T: Serialize` surface, no `serde_json::Value`, no
//! `json!`, and no `to_value`/`to_string`/`to_vec`: the seam serializes a concrete
//! private envelope with `serde_json::to_writer` into a size-bounded sink.

use std::io::{self, Write};

use lsp_types::{
    CompletionResponse, DocumentSymbolResponse, Hover, InitializeResult, Location,
    PublishDiagnosticsParams, SignatureHelp, TextEdit,
};
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

use crate::capacities::MAX_OUTBOUND_FRAME_BYTES;
use crate::protocol::RequestId;

/// A window/showMessage severity. Background capture failures and whole-analysis
/// stops use `ERROR`; the type is closed to what the server sends.
#[derive(Clone, Copy)]
pub(crate) enum MessageType {
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

/// A closed outbound message. The coordinator hands off exactly one per outbound credit.
pub(crate) enum Outbound {
    /// A response carrying a result.
    Result {
        /// The request id.
        id: RequestId,
        /// The result payload.
        result: ResponseResult,
    },
    /// A JSON-RPC error. A null id is the null-id protocol-error reply.
    Error {
        /// The request id, or `None` for a null-id reply.
        id: Option<RequestId>,
        /// The refusal, which names its wire code and message.
        code: ErrorCode,
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

/// The `result` member of one response: the standard payload of the request's method,
/// or `null`. Untagged, so a variant serializes as its payload alone.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ResponseResult {
    /// The `initialize` result.
    Initialize(Box<InitializeResult>),
    /// A hover, or null.
    Hover(Option<Hover>),
    /// A definition location, or null.
    Definition(Option<Location>),
    /// Formatting edits, or null.
    Formatting(Option<Vec<TextEdit>>),
    /// A complete completion candidate list, or null.
    Completion(Option<CompletionResponse>),
    /// A signature-help payload, or null.
    SignatureHelp(Option<SignatureHelp>),
    /// A declaration outline, or null.
    DocumentSymbol(Option<DocumentSymbolResponse>),
    /// The `shutdown` acknowledgement.
    Null,
}

/// A refusal the server answers with. Each variant is one JSON-RPC error whose wire
/// code and message are spelled here alone, so a reply's code and message cannot
/// disagree and a reason is compared as a variant, never as prose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ErrorCode {
    /// The message was not valid JSON.
    ParseError,
    /// The message was not a well-formed request object.
    InvalidRequest,
    /// A batch (top-level array) is not accepted.
    BatchUnsupported,
    /// The id is still live for an earlier request.
    DuplicateRequestId,
    /// The request is not admissible in the current lifecycle phase.
    InvalidInPhase,
    /// A second `initialize`.
    InitializeRepeated,
    /// An unknown request method.
    MethodNotFound,
    /// The `initialize` parameters did not decode.
    MalformedInitializeParams,
    /// The workspace root is malformed or is not exactly one folder.
    MalformedWorkspaceRoot,
    /// `initialize` named no workspace root, so no document can be resolved.
    NoWorkspaceRoot,
    /// A semantic request's parameters did not decode, or named a document outside
    /// the selected root.
    MalformedParams,
    /// An internal server error.
    InternalError,
    /// A request arrived before the server was initialized.
    ServerNotInitialized,
    /// The request's admitted revision or document version is no longer current.
    ContentModified,
    /// An open document's last edit was refused by overlay admission.
    CaptureUnavailable,
    /// The analysis exhausted a resource bound; recoverable.
    AnalysisResourceLimit,
}

impl ErrorCode {
    /// The JSON-RPC or LSP wire code.
    fn code(self) -> i32 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest
            | Self::BatchUnsupported
            | Self::DuplicateRequestId
            | Self::InvalidInPhase
            | Self::InitializeRepeated => -32600,
            Self::MethodNotFound => -32601,
            Self::MalformedInitializeParams
            | Self::MalformedWorkspaceRoot
            | Self::NoWorkspaceRoot
            | Self::MalformedParams => -32602,
            Self::InternalError => -32603,
            Self::ServerNotInitialized => -32002,
            Self::ContentModified => -32801,
            Self::CaptureUnavailable | Self::AnalysisResourceLimit => -32803,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::ParseError => "parse error",
            Self::InvalidRequest => "invalid request",
            Self::BatchUnsupported => "batch requests are not supported",
            Self::DuplicateRequestId => "duplicate request id",
            Self::InvalidInPhase => "invalid request in current state",
            Self::InitializeRepeated => "initialize already handled",
            Self::MethodNotFound => "method not found",
            Self::MalformedInitializeParams => "malformed initialize params",
            Self::MalformedWorkspaceRoot => "malformed workspace root",
            Self::NoWorkspaceRoot => "no selected root",
            Self::MalformedParams => "malformed params",
            Self::InternalError => "internal error",
            Self::ServerNotInitialized => "server not initialized",
            Self::ContentModified => "content modified",
            Self::CaptureUnavailable => "project capture unavailable",
            Self::AnalysisResourceLimit => "analysis resource limit",
        }
    }
}

/// Why encoding an outbound frame failed. Both are internal-error class before handoff:
/// no bytes are emitted.
#[derive(Debug)]
pub(crate) enum EncodeError {
    /// Serialization failed (a payload could not be encoded).
    Serialize,
    /// The encoded body exceeded [`MAX_OUTBOUND_FRAME_BYTES`].
    TooLarge,
}

/// Serialize one outbound message into a bounded immutable frame body. On any failure
/// no partial bytes escape: the returned error carries nothing and the caller emits
/// zero bytes.
pub(crate) fn encode(outbound: &Outbound) -> Result<Vec<u8>, EncodeError> {
    let mut sink = BoundedWriter::new(MAX_OUTBOUND_FRAME_BYTES);
    let written = match outbound {
        Outbound::Result { id, result } => {
            serde_json::to_writer(sink.by_ref(), &ResultEnvelope { id, result })
        }
        Outbound::Error { id, code } => serde_json::to_writer(
            sink.by_ref(),
            &ErrorEnvelope {
                id: id.as_ref(),
                code: *code,
            },
        ),
        Outbound::PublishDiagnostics(params) => serde_json::to_writer(
            sink.by_ref(),
            &NotificationEnvelope {
                method: "textDocument/publishDiagnostics",
                params: params.as_ref(),
            },
        ),
        Outbound::ShowMessage { typ, message } => serde_json::to_writer(
            sink.by_ref(),
            &NotificationEnvelope {
                method: "window/showMessage",
                params: &ShowMessageParams {
                    typ: typ.code(),
                    message,
                },
            },
        ),
    };
    match written {
        Ok(()) => Ok(sink.into_inner()),
        // The bounded sink's overflow surfaces as an io error through `to_writer`; a
        // genuine serialization defect is a data error.
        Err(error) if error.is_io() => Err(EncodeError::TooLarge),
        Err(_) => Err(EncodeError::Serialize),
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

/// A concrete result envelope: `{jsonrpc, id, result}`.
struct ResultEnvelope<'a> {
    id: &'a RequestId,
    result: &'a ResponseResult,
}

impl Serialize for ResultEnvelope<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("Response", 3)?;
        envelope.serialize_field("jsonrpc", "2.0")?;
        envelope.serialize_field("id", &WireId(self.id))?;
        envelope.serialize_field("result", self.result)?;
        envelope.end()
    }
}

/// A concrete error envelope: `{jsonrpc, id, error:{code,message}}`.
struct ErrorEnvelope<'a> {
    id: Option<&'a RequestId>,
    code: ErrorCode,
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
                code: self.code.code(),
                message: self.code.message(),
            },
        )?;
        envelope.end()
    }
}

#[derive(Serialize)]
struct WireError {
    code: i32,
    message: &'static str,
}

/// A concrete notification envelope: `{jsonrpc, method, params}`. `P` is a concrete
/// payload at each call site — never a public generic surface.
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

/// An `io::Write` sink that accepts at most `limit` bytes before failing, so an encoded
/// frame can never exceed the outbound bound. Overflow bytes are never retained.
struct BoundedWriter {
    buffer: Vec<u8>,
    limit: usize,
}

impl BoundedWriter {
    fn new(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.buffer
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buffer.len() + data.len() > self.limit {
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
        let text = body(&Outbound::Result {
            id: RequestId::Integer(4),
            result: ResponseResult::Null,
        });
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":4,"result":null}"#);
    }

    #[test]
    fn error_with_integer_id() {
        let text = body(&Outbound::Error {
            id: Some(RequestId::Integer(7)),
            code: ErrorCode::MethodNotFound,
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
            code: ErrorCode::ParseError,
        });
        assert_eq!(
            text,
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}"#
        );
    }

    #[test]
    fn string_id_is_serialized_as_string() {
        let text = body(&Outbound::Result {
            id: RequestId::Text("abc".to_owned()),
            result: ResponseResult::Null,
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
        let text = body(&Outbound::Result {
            id: RequestId::Integer(1),
            result: ResponseResult::Hover(None),
        });
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":1,"result":null}"#);
    }

    #[test]
    fn oversized_body_is_too_large_with_no_partial_bytes() {
        let huge = "x".repeat(MAX_OUTBOUND_FRAME_BYTES + 1);
        let error = encode(&Outbound::ShowMessage {
            typ: MessageType::Error,
            message: huge,
        });
        assert!(matches!(error, Err(EncodeError::TooLarge)));
    }
}
