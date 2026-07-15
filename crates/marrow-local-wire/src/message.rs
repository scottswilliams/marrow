//! The closed handshake / request / response / fault grammar.
//!
//! Two message sets cross the wire, one per direction. A [`ClientMessage`] is sent
//! by the caller (the generated client or a terminal) to the runner; a
//! [`ServerMessage`] is the runner's reply. Each message is a canonical JSON object
//! tagged by a `"kind"` field, carried in a versioned length-prefixed frame. The
//! sets are disjoint, so a message decoded in the wrong direction — a client that
//! receives a request, a runner that receives a response — is rejected as malformed
//! rather than silently confused.
//!
//! The grammar is closed: there is no free-form envelope, no streaming or partial
//! reply, and no replay/cancellation message. A mutating call whose reply is lost is
//! classified through [`crate::loss`], never resent.

use crate::error::WireError;
use crate::id::Id32;
use crate::json::{self, Json};
use crate::{frame, span::Span};

/// A message from the caller to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// Open the session, proving the launch nonce the supervisor issued.
    Hello { nonce: Id32 },
    /// Invoke `export` with positional `args`, each an already-encoded transfer
    /// value. The runner decodes them against the export's verified signature.
    Request { export: Id32, args: Vec<Json> },
}

/// A message from the runner to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// The handshake succeeded: the runner proves its session token and pins the
    /// interface identity the launched image serves.
    Ready { session: Id32, interface: Id32 },
    /// A successful call result (JSON `null` for a unit return).
    Value { data: Json },
    /// A source-mapped runtime fault raised while running the export.
    Fault { code: String, span: Span },
    /// The request could not be admitted or run (an unknown export, an argument
    /// mismatch, or a durable export the stock runner will not execute). The
    /// `code` is the runner's typed reason.
    Reject { code: String },
}

impl ClientMessage {
    /// Encode this message as a full frame.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        frame::assemble(json::encode(&self.to_json()).as_bytes())
    }

    /// Decode a client message from a frame body (`version ‖ json`).
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let value = json::parse_strict(frame::body_json(body)?)?;
        let object = Fields::new(&value)?;
        match object.kind()? {
            "hello" => {
                object.exact(&["kind", "nonce"])?;
                Ok(ClientMessage::Hello {
                    nonce: object.id("nonce")?,
                })
            }
            "request" => {
                object.exact(&["args", "export", "kind"])?;
                Ok(ClientMessage::Request {
                    export: object.id("export")?,
                    args: object.array("args")?.to_vec(),
                })
            }
            _ => Err(WireError::Malformed),
        }
    }

    fn to_json(&self) -> Json {
        match self {
            ClientMessage::Hello { nonce } => object(vec![
                ("kind", Json::Str("hello".to_string())),
                ("nonce", Json::Str(nonce.to_hex())),
            ]),
            ClientMessage::Request { export, args } => object(vec![
                ("kind", Json::Str("request".to_string())),
                ("export", Json::Str(export.to_hex())),
                ("args", Json::Array(args.clone())),
            ]),
        }
    }
}

impl ServerMessage {
    /// Encode this message as a full frame.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        frame::assemble(json::encode(&self.to_json()).as_bytes())
    }

    /// Decode a server message from a frame body (`version ‖ json`).
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let value = json::parse_strict(frame::body_json(body)?)?;
        let object = Fields::new(&value)?;
        match object.kind()? {
            "ready" => {
                object.exact(&["interface", "kind", "session"])?;
                Ok(ServerMessage::Ready {
                    session: object.id("session")?,
                    interface: object.id("interface")?,
                })
            }
            "value" => {
                object.exact(&["data", "kind"])?;
                Ok(ServerMessage::Value {
                    data: object.get("data")?.clone(),
                })
            }
            "fault" => {
                object.exact(&["code", "kind", "span"])?;
                Ok(ServerMessage::Fault {
                    code: object.code("code")?,
                    span: object.span("span")?,
                })
            }
            "reject" => {
                object.exact(&["code", "kind"])?;
                Ok(ServerMessage::Reject {
                    code: object.code("code")?,
                })
            }
            _ => Err(WireError::Malformed),
        }
    }

    fn to_json(&self) -> Json {
        match self {
            ServerMessage::Ready { session, interface } => object(vec![
                ("kind", Json::Str("ready".to_string())),
                ("session", Json::Str(session.to_hex())),
                ("interface", Json::Str(interface.to_hex())),
            ]),
            ServerMessage::Value { data } => object(vec![
                ("kind", Json::Str("value".to_string())),
                ("data", data.clone()),
            ]),
            ServerMessage::Fault { code, span } => object(vec![
                ("kind", Json::Str("fault".to_string())),
                ("code", Json::Str(code.clone())),
                ("span", span.to_json()),
            ]),
            ServerMessage::Reject { code } => object(vec![
                ("kind", Json::Str("reject".to_string())),
                ("code", Json::Str(code.clone())),
            ]),
        }
    }
}

fn object(pairs: Vec<(&str, Json)>) -> Json {
    Json::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// A decode-side view over a message object's fields, resolving each by key with
/// typed access. Every miss is a typed [`WireError`].
struct Fields<'a> {
    pairs: &'a [(String, Json)],
}

impl<'a> Fields<'a> {
    fn new(value: &'a Json) -> Result<Self, WireError> {
        match value {
            Json::Object(pairs) => Ok(Fields { pairs }),
            _ => Err(WireError::Malformed),
        }
    }

    fn get(&self, key: &str) -> Result<&'a Json, WireError> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .ok_or(WireError::Malformed)
    }

    fn kind(&self) -> Result<&'a str, WireError> {
        match self.get("kind")? {
            Json::Str(s) => Ok(s.as_str()),
            _ => Err(WireError::Malformed),
        }
    }

    /// The object's keys must be exactly `expected` — no missing and no extra
    /// field. The canonical parse already guarantees uniqueness and sort order.
    fn exact(&self, expected: &[&str]) -> Result<(), WireError> {
        if self.pairs.len() != expected.len() {
            return Err(WireError::Malformed);
        }
        for key in expected {
            if !self.pairs.iter().any(|(k, _)| k == key) {
                return Err(WireError::Malformed);
            }
        }
        Ok(())
    }

    fn id(&self, key: &str) -> Result<Id32, WireError> {
        match self.get(key)? {
            Json::Str(s) => Id32::from_hex(s).ok_or(WireError::Malformed),
            _ => Err(WireError::Malformed),
        }
    }

    fn array(&self, key: &str) -> Result<&'a [Json], WireError> {
        match self.get(key)? {
            Json::Array(items) => Ok(items),
            _ => Err(WireError::Malformed),
        }
    }

    /// A dotted diagnostic-code string: non-empty and lowercase-dotted ASCII. The
    /// wire carries it opaquely; the runner produces it from a typed code.
    fn code(&self, key: &str) -> Result<String, WireError> {
        match self.get(key)? {
            Json::Str(s)
                if !s.is_empty()
                    && s.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'
                    }) =>
            {
                Ok(s.clone())
            }
            _ => Err(WireError::Malformed),
        }
    }

    fn span(&self, key: &str) -> Result<Span, WireError> {
        let span = Fields::new(self.get(key)?)?;
        span.exact(&["column", "line"])?;
        Ok(Span {
            line: span.u32("line")?,
            column: span.u32("column")?,
        })
    }

    fn u32(&self, key: &str) -> Result<u32, WireError> {
        match self.get(key)? {
            Json::Int(n) if *n >= 0 && *n <= i64::from(u32::MAX) => Ok(*n as u32),
            _ => Err(WireError::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientMessage, ServerMessage};
    use crate::id::Id32;
    use crate::json::{self, Json};
    use crate::span::Span;

    fn json_of(frame: &[u8]) -> String {
        // Skip the 4-byte length prefix and the version byte.
        String::from_utf8(frame[5..].to_vec()).expect("utf8 json")
    }

    /// Frozen canonical spellings for each message kind.
    #[test]
    fn message_json_is_frozen() {
        assert_eq!(
            json_of(
                &ClientMessage::Hello {
                    nonce: Id32::from_bytes([0; 32])
                }
                .encode()
                .unwrap()
            ),
            r#"{"kind":"hello","nonce":"0000000000000000000000000000000000000000000000000000000000000000"}"#
        );
        assert_eq!(
            json_of(
                &ClientMessage::Request {
                    export: Id32::from_bytes([0x11; 32]),
                    args: vec![Json::Int(1), Json::Bool(true)],
                }
                .encode()
                .unwrap()
            ),
            r#"{"args":[1,true],"export":"1111111111111111111111111111111111111111111111111111111111111111","kind":"request"}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Value {
                    data: Json::Int(42)
                }
                .encode()
                .unwrap()
            ),
            r#"{"data":42,"kind":"value"}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Fault {
                    code: "run.overflow".to_string(),
                    span: Span { line: 7, column: 2 },
                }
                .encode()
                .unwrap()
            ),
            r#"{"code":"run.overflow","kind":"fault","span":{"column":2,"line":7}}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Reject {
                    code: "runner.unknown_export".to_string()
                }
                .encode()
                .unwrap()
            ),
            r#"{"code":"runner.unknown_export","kind":"reject"}"#
        );
    }

    fn client_round_trip(msg: ClientMessage) {
        let frame = msg.encode().expect("encode");
        let body = &frame[4..];
        assert_eq!(ClientMessage::decode(body), Ok(msg));
    }

    fn server_round_trip(msg: ServerMessage) {
        let frame = msg.encode().expect("encode");
        let body = &frame[4..];
        assert_eq!(ServerMessage::decode(body), Ok(msg));
    }

    #[test]
    fn every_message_round_trips() {
        client_round_trip(ClientMessage::Hello {
            nonce: Id32::from_bytes([9; 32]),
        });
        client_round_trip(ClientMessage::Request {
            export: Id32::from_bytes([3; 32]),
            args: vec![Json::Null, Json::Str("x".to_string())],
        });
        server_round_trip(ServerMessage::Ready {
            session: Id32::from_bytes([1; 32]),
            interface: Id32::from_bytes([2; 32]),
        });
        server_round_trip(ServerMessage::Value {
            data: Json::Array(vec![Json::Int(-1)]),
        });
        server_round_trip(ServerMessage::Fault {
            code: "run.budget".to_string(),
            span: Span { line: 1, column: 1 },
        });
        server_round_trip(ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string(),
        });
    }

    /// A message decoded in the wrong direction is rejected, never confused.
    #[test]
    fn directions_do_not_cross() {
        let hello = ClientMessage::Hello {
            nonce: Id32::from_bytes([0; 32]),
        }
        .encode()
        .unwrap();
        assert!(ServerMessage::decode(&hello[4..]).is_err());
        let value = ServerMessage::Value { data: Json::Null }.encode().unwrap();
        assert!(ClientMessage::decode(&value[4..]).is_err());
    }

    /// An extra or missing field is malformed even when the JSON is canonical.
    #[test]
    fn field_sets_are_exact() {
        let extra = json::encode(&super::object(vec![
            ("kind", Json::Str("value".to_string())),
            ("data", Json::Int(1)),
            ("extra", Json::Int(2)),
        ]));
        let body = [&[crate::PROTOCOL_VERSION], extra.as_bytes()].concat();
        assert!(ServerMessage::decode(&body).is_err());
    }
}
