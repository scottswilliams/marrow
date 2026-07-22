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
//! Every request and call reply carries one exact u32 turn. A serial client assigns a
//! turn once and the runner channel echoes it on that request's sole response, so a
//! delayed response from an earlier turn cannot settle a later call.

use crate::error::WireError;
use crate::id::Id32;
use crate::json::{self, Json};
use crate::{frame, span::Span};

/// What is known about durable state after an invocation stopped without
/// completing. This is independent of the source-mapped fault and carries no
/// recovery witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableState {
    KnownOld,
    KnownNew,
    Unknown,
}

impl DurableState {
    fn as_str(self) -> &'static str {
        match self {
            Self::KnownOld => "known_old",
            Self::KnownNew => "known_new",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "known_old" => Some(Self::KnownOld),
            "known_new" => Some(Self::KnownNew),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// A message from the caller to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// Open the session, proving the launch nonce the supervisor issued.
    Hello { nonce: Id32 },
    /// Invoke `export` with positional `args`, each an already-encoded transfer
    /// value. The runner decodes them against the export's verified signature.
    Request { export: Id32, args: Vec<Json> },
    /// Provision a fresh persistent store for the launched image at the `store`
    /// destination, gated by `approval` — the token of the exact rendered provision
    /// report the owner accepted. The runner rebuilds the report for its image and
    /// destination and refuses a token that does not match, so a store is never
    /// provisioned without an auditable acceptance.
    Provision { store: String, approval: String },
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
    /// The invocation stopped without returning. Its durable state is reported
    /// independently of the source-mapped fault; no recovery witness crosses
    /// the wire.
    Incomplete {
        code: String,
        durable: DurableState,
        span: Span,
    },
    /// The request could not be admitted or run (an unknown export, an argument
    /// mismatch, or a durable export the stock runner will not execute). The
    /// `code` is the runner's typed reason.
    Reject { code: String },
    /// A store was provisioned: `instance` is the fresh store instance identity
    /// (lowercase hex). The receipt of a completed provision.
    Provisioned { instance: String },
}

impl ClientMessage {
    /// Encode this message as a full frame. A request uses turn zero; serial clients that
    /// keep a session open use [`Self::encode_with_turn`] to assign a distinct turn.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        self.encode_with_turn(0)
    }

    /// Encode this message as a full frame, assigning `turn` to a request. Handshake and
    /// provisioning messages have no call turn and therefore ignore the argument.
    pub fn encode_with_turn(&self, turn: u32) -> Result<Vec<u8>, WireError> {
        frame::assemble(json::encode(&self.to_json(turn)).as_bytes())
    }

    /// Decode a client message from a frame body (`version ‖ json`), discarding a request's
    /// turn. The runner channel uses [`Self::decode_with_turn`] so it can echo that turn on the
    /// exact response.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        Self::decode_with_turn(body).map(|(message, _)| message)
    }

    /// Decode a client message and its call turn. `Some(turn)` is returned exactly for a
    /// request; handshake and provisioning messages return `None`.
    pub fn decode_with_turn(body: &[u8]) -> Result<(Self, Option<u32>), WireError> {
        let value = json::parse_strict(frame::body_json(body)?)?;
        let object = Fields::new(&value)?;
        match object.kind()? {
            "hello" => {
                object.exact(&["kind", "nonce"])?;
                Ok((
                    ClientMessage::Hello {
                        nonce: object.id("nonce")?,
                    },
                    None,
                ))
            }
            "request" => {
                object.exact(&["args", "export", "kind", "turn"])?;
                Ok((
                    ClientMessage::Request {
                        export: object.id("export")?,
                        args: object.array("args")?.to_vec(),
                    },
                    Some(object.u32("turn")?),
                ))
            }
            "provision" => {
                object.exact(&["approval", "kind", "store"])?;
                Ok((
                    ClientMessage::Provision {
                        store: object.string("store")?,
                        approval: object.string("approval")?,
                    },
                    None,
                ))
            }
            _ => Err(WireError::Malformed),
        }
    }

    fn to_json(&self, turn: u32) -> Json {
        match self {
            ClientMessage::Hello { nonce } => object(vec![
                ("kind", Json::Str("hello".to_string())),
                ("nonce", Json::Str(nonce.to_hex())),
            ]),
            ClientMessage::Request { export, args } => object(vec![
                ("kind", Json::Str("request".to_string())),
                ("export", Json::Str(export.to_hex())),
                ("args", Json::Array(args.clone())),
                ("turn", Json::Int(i64::from(turn))),
            ]),
            ClientMessage::Provision { store, approval } => object(vec![
                ("kind", Json::Str("provision".to_string())),
                ("store", Json::Str(store.clone())),
                ("approval", Json::Str(approval.clone())),
            ]),
        }
    }
}

impl ServerMessage {
    /// Encode this message as a full frame. A call reply uses turn zero; a runner channel uses
    /// [`Self::encode_with_turn`] to echo the request's distinct turn.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        self.encode_with_turn(0)
    }

    /// Encode this message as a full frame, assigning `turn` to a call reply. `Ready` and
    /// `Provisioned` are not call replies and therefore carry no turn.
    pub fn encode_with_turn(&self, turn: u32) -> Result<Vec<u8>, WireError> {
        frame::assemble(json::encode(&self.to_json(turn)).as_bytes())
    }

    /// Decode a server message from a frame body (`version ‖ json`), discarding a reply's
    /// correlation turn. Supervisors that keep a session open use [`Self::decode_with_turn`].
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        Self::decode_with_turn(body).map(|(message, _)| message)
    }

    /// Decode a server message and its call turn. `Some(turn)` is returned exactly for call
    /// replies; the handshake and provisioning receipt return `None`.
    pub fn decode_with_turn(body: &[u8]) -> Result<(Self, Option<u32>), WireError> {
        let value = json::parse_strict(frame::body_json(body)?)?;
        let object = Fields::new(&value)?;
        match object.kind()? {
            "ready" => {
                object.exact(&["interface", "kind", "session"])?;
                Ok((
                    ServerMessage::Ready {
                        session: object.id("session")?,
                        interface: object.id("interface")?,
                    },
                    None,
                ))
            }
            "value" => {
                object.exact(&["data", "kind", "turn"])?;
                Ok((
                    ServerMessage::Value {
                        data: object.get("data")?.clone(),
                    },
                    Some(object.u32("turn")?),
                ))
            }
            "fault" => {
                object.exact(&["code", "kind", "span", "turn"])?;
                Ok((
                    ServerMessage::Fault {
                        code: object.code("code")?,
                        span: object.span("span")?,
                    },
                    Some(object.u32("turn")?),
                ))
            }
            "incomplete" => {
                object.exact(&["code", "durable", "kind", "span", "turn"])?;
                Ok((
                    ServerMessage::Incomplete {
                        code: object.code("code")?,
                        durable: object.durable_state("durable")?,
                        span: object.span("span")?,
                    },
                    Some(object.u32("turn")?),
                ))
            }
            "reject" => {
                object.exact(&["code", "kind", "turn"])?;
                Ok((
                    ServerMessage::Reject {
                        code: object.code("code")?,
                    },
                    Some(object.u32("turn")?),
                ))
            }
            "provisioned" => {
                object.exact(&["instance", "kind"])?;
                Ok((
                    ServerMessage::Provisioned {
                        instance: object.string("instance")?,
                    },
                    None,
                ))
            }
            _ => Err(WireError::Malformed),
        }
    }

    fn to_json(&self, turn: u32) -> Json {
        match self {
            ServerMessage::Ready { session, interface } => object(vec![
                ("kind", Json::Str("ready".to_string())),
                ("session", Json::Str(session.to_hex())),
                ("interface", Json::Str(interface.to_hex())),
            ]),
            ServerMessage::Value { data } => object(vec![
                ("kind", Json::Str("value".to_string())),
                ("data", data.clone()),
                ("turn", Json::Int(i64::from(turn))),
            ]),
            ServerMessage::Fault { code, span } => object(vec![
                ("kind", Json::Str("fault".to_string())),
                ("code", Json::Str(code.clone())),
                ("span", span.to_json()),
                ("turn", Json::Int(i64::from(turn))),
            ]),
            ServerMessage::Incomplete {
                code,
                durable,
                span,
            } => object(vec![
                ("kind", Json::Str("incomplete".to_string())),
                ("code", Json::Str(code.clone())),
                ("durable", Json::Str(durable.as_str().to_string())),
                ("span", span.to_json()),
                ("turn", Json::Int(i64::from(turn))),
            ]),
            ServerMessage::Reject { code } => object(vec![
                ("kind", Json::Str("reject".to_string())),
                ("code", Json::Str(code.clone())),
                ("turn", Json::Int(i64::from(turn))),
            ]),
            ServerMessage::Provisioned { instance } => object(vec![
                ("kind", Json::Str("provisioned".to_string())),
                ("instance", Json::Str(instance.clone())),
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

    /// An arbitrary JSON string field (a destination path or a report/instance token). The
    /// wire carries it opaquely; the runner interprets it against its image and filesystem.
    fn string(&self, key: &str) -> Result<String, WireError> {
        match self.get(key)? {
            Json::Str(s) => Ok(s.clone()),
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

    fn durable_state(&self, key: &str) -> Result<DurableState, WireError> {
        match self.get(key)? {
            Json::Str(value) => DurableState::parse(value).ok_or(WireError::Malformed),
            _ => Err(WireError::Malformed),
        }
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
    use super::{ClientMessage, DurableState, ServerMessage};
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
            r#"{"args":[1,true],"export":"1111111111111111111111111111111111111111111111111111111111111111","kind":"request","turn":0}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Ready {
                    session: Id32::from_bytes([0x11; 32]),
                    interface: Id32::from_bytes([0x22; 32]),
                }
                .encode()
                .unwrap()
            ),
            r#"{"interface":"2222222222222222222222222222222222222222222222222222222222222222","kind":"ready","session":"1111111111111111111111111111111111111111111111111111111111111111"}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Value {
                    data: Json::Int(42)
                }
                .encode()
                .unwrap()
            ),
            r#"{"data":42,"kind":"value","turn":0}"#
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
            r#"{"code":"run.overflow","kind":"fault","span":{"column":2,"line":7},"turn":0}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Incomplete {
                    code: "run.commit".to_string(),
                    durable: DurableState::KnownNew,
                    span: Span { line: 9, column: 4 },
                }
                .encode()
                .unwrap()
            ),
            r#"{"code":"run.commit","durable":"known_new","kind":"incomplete","span":{"column":4,"line":9},"turn":0}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Reject {
                    code: "runner.unknown_export".to_string()
                }
                .encode()
                .unwrap()
            ),
            r#"{"code":"runner.unknown_export","kind":"reject","turn":0}"#
        );
        assert_eq!(
            json_of(
                &ClientMessage::Provision {
                    store: "/data/notes".to_string(),
                    approval: "0123456789abcdef".to_string(),
                }
                .encode()
                .unwrap()
            ),
            r#"{"approval":"0123456789abcdef","kind":"provision","store":"/data/notes"}"#
        );
        assert_eq!(
            json_of(
                &ServerMessage::Provisioned {
                    instance: "00112233445566778899aabbccddeeff".to_string(),
                }
                .encode()
                .unwrap()
            ),
            r#"{"instance":"00112233445566778899aabbccddeeff","kind":"provisioned"}"#
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
        client_round_trip(ClientMessage::Provision {
            store: "/data/notes".to_string(),
            approval: "0123456789abcdef".to_string(),
        });
        server_round_trip(ServerMessage::Provisioned {
            instance: "00112233445566778899aabbccddeeff".to_string(),
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
        for durable in [
            DurableState::KnownOld,
            DurableState::KnownNew,
            DurableState::Unknown,
        ] {
            server_round_trip(ServerMessage::Incomplete {
                code: "run.commit".to_string(),
                durable,
                span: Span { line: 2, column: 3 },
            });
        }
        server_round_trip(ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string(),
        });
    }

    /// A call turn is an exact u32 in both directions. The maximum value round-trips; negative,
    /// overflowing, non-integer, missing, and extra spellings are refused by the authoritative
    /// message owner.
    #[test]
    fn call_turns_are_exact_u32_values() {
        let request = ClientMessage::Request {
            export: Id32::from_bytes([3; 32]),
            args: vec![],
        };
        let frame = request.encode_with_turn(u32::MAX).expect("encode request");
        let (decoded, turn) = ClientMessage::decode_with_turn(&frame[4..]).expect("decode request");
        assert_eq!(decoded, request);
        assert_eq!(turn, Some(u32::MAX));

        let reply = ServerMessage::Value { data: Json::Null };
        let frame = reply.encode_with_turn(u32::MAX).expect("encode reply");
        let (decoded, turn) = ServerMessage::decode_with_turn(&frame[4..]).expect("decode reply");
        assert_eq!(decoded, reply);
        assert_eq!(turn, Some(u32::MAX));

        for bad in [
            Json::Int(-1),
            Json::Int(i64::from(u32::MAX) + 1),
            Json::Str("0".into()),
        ] {
            let encoded = json::encode(&super::object(vec![
                ("kind", Json::Str("value".to_string())),
                ("data", Json::Null),
                ("turn", bad),
            ]));
            let body = [&[crate::PROTOCOL_VERSION], encoded.as_bytes()].concat();
            assert!(ServerMessage::decode_with_turn(&body).is_err());
        }

        for pairs in [
            vec![
                ("kind", Json::Str("value".to_string())),
                ("data", Json::Null),
            ],
            vec![
                ("kind", Json::Str("value".to_string())),
                ("data", Json::Null),
                ("turn", Json::Int(0)),
                ("extra", Json::Null),
            ],
        ] {
            let encoded = json::encode(&super::object(pairs));
            let body = [&[crate::PROTOCOL_VERSION], encoded.as_bytes()].concat();
            assert!(ServerMessage::decode_with_turn(&body).is_err());
        }
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
        // A provision request is a client message; a provisioned receipt is a server
        // message. Neither decodes in the other direction.
        let provision = ClientMessage::Provision {
            store: "/data/x".to_string(),
            approval: "abc".to_string(),
        }
        .encode()
        .unwrap();
        assert!(ServerMessage::decode(&provision[4..]).is_err());
        let provisioned = ServerMessage::Provisioned {
            instance: "00".to_string(),
        }
        .encode()
        .unwrap();
        assert!(ClientMessage::decode(&provisioned[4..]).is_err());
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

        let bad_durable = json::encode(&super::object(vec![
            ("kind", Json::Str("incomplete".to_string())),
            ("code", Json::Str("run.commit".to_string())),
            ("durable", Json::Str("maybe_new".to_string())),
            (
                "span",
                super::object(vec![("line", Json::Int(1)), ("column", Json::Int(1))]),
            ),
        ]));
        let body = [&[crate::PROTOCOL_VERSION], bad_durable.as_bytes()].concat();
        assert!(
            ServerMessage::decode(&body).is_err(),
            "the durable-state vocabulary is closed",
        );
    }
}
