//! The private closed JSON-RPC 2.0 envelope and its side-effect-free inbound decoder.
//!
//! [`decode`] turns one framed message body into exactly one closed [`Inbound`]
//! outcome. It reads and mutates no coordinator state: it returns only the bounded
//! recovered-id candidate a protocol error may reply with, and the coordinator alone
//! consults its ledger. The decoder rejects every JSON array as `NoBatch` under the
//! explicit LSP-3.18 profile, drops a well-formed unsolicited response, and recovers a
//! single bounded integer or string id from an otherwise-invalid request object.
//!
//! There is no generic `T: Serialize`/`Deserialize` payload surface, no
//! `serde_json::Value`, and no `json!`: a hand-written [`serde::de::Visitor`] over the
//! envelope map detects duplicate recognized fields, skips unknown members within the
//! decoded-value bounds, and captures method-specific parameters as an owned
//! [`RawValue`] for the method router to decode.

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;

use crate::capacities::MAX_REQUEST_ID_STRING_BYTES;

/// A JSON-RPC request/response id the ledger can carry: a bounded 32-bit integer or a
/// bounded UTF-8 string. Integer `1` and string `"1"` are deliberately distinct. A
/// null, fractional, out-of-range, or over-length id never becomes a `RequestId`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RequestId {
    /// A bounded integer id.
    Integer(i32),
    /// A bounded UTF-8 string id.
    Text(String),
}

/// A raw id token observed in the envelope before it is admitted as a [`RequestId`].
/// Only [`IdToken::Integer`] and [`IdToken::Text`] within bounds recover to an id;
/// every other token (null, fractional, out-of-range, over-length, wrong shape) is
/// unrecoverable.
enum IdToken {
    Absent,
    Null,
    Integer(i64),
    Text(String),
    Unrecoverable,
}

impl IdToken {
    /// Recover a bounded [`RequestId`], or `None` for an absent, null, fractional,
    /// out-of-range, over-length, or wrong-shaped id.
    fn recover(&self) -> Option<RequestId> {
        match self {
            IdToken::Integer(value) => i32::try_from(*value).ok().map(RequestId::Integer),
            IdToken::Text(text) if text.len() <= MAX_REQUEST_ID_STRING_BYTES => {
                Some(RequestId::Text(text.clone()))
            }
            _ => None,
        }
    }
}

/// The closed outcome of decoding one message body. The decoder produces exactly one.
pub enum Inbound {
    /// A well-formed request: a bounded id, a method, and owned method parameters.
    Request {
        /// The recovered request id.
        id: RequestId,
        /// The method name.
        method: String,
        /// The raw method parameters, or `None` when the member is absent.
        params: Option<Box<RawValue>>,
    },
    /// A well-formed notification: a method and owned parameters, no id.
    Notification {
        /// The method name.
        method: String,
        /// The raw method parameters, or `None` when the member is absent.
        params: Option<Box<RawValue>>,
    },
    /// A structurally valid response object (an id plus exactly one of result/error,
    /// no method). The server issues no unsolicited requests, so it is dropped.
    UnsolicitedResponse,
    /// The message is not a routable request or notification.
    Reject(Reject),
}

/// Why a message was rejected, and the bounded id a reply may carry.
pub enum Reject {
    /// Invalid UTF-8/JSON, malformed array syntax, or trailing/concatenated JSON.
    /// Replied to as `-32700` with a null id.
    ParseError,
    /// A structurally invalid request. Replied to as `-32600`.
    InvalidRequest {
        /// The single uniquely recoverable bounded id, or `None` for a null-id reply.
        recovered_id: Option<RequestId>,
        /// Why the request is structurally invalid.
        reason: InvalidReason,
    },
}

/// Why a structurally invalid request is invalid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InvalidReason {
    /// A scalar/null top level, or an object that is neither a request, a
    /// notification, nor a valid response.
    Structural,
    /// A JSON array at the top level, rejected wholesale under the LSP-3.18 profile
    /// after bounded syntax validation and with zero semantic element dispatch.
    NoBatch,
}

/// Decode one framed message body into exactly one closed [`Inbound`] outcome. Reads
/// and mutates no ledger state.
pub fn decode(bytes: &[u8]) -> Inbound {
    match serde_json::from_slice::<TopLevel>(bytes) {
        Ok(TopLevel::Object(envelope)) => envelope.classify(),
        Ok(TopLevel::Array) => Inbound::Reject(Reject::InvalidRequest {
            recovered_id: None,
            reason: InvalidReason::NoBatch,
        }),
        Ok(TopLevel::Scalar) => Inbound::Reject(Reject::InvalidRequest {
            recovered_id: None,
            reason: InvalidReason::Structural,
        }),
        // Invalid UTF-8/JSON, malformed array syntax, and trailing/concatenated JSON
        // all surface as a deserialize error and are one `ParseError`. `from_slice`
        // rejects trailing non-whitespace, so concatenated JSON never decodes as a
        // valid first message.
        Err(_) => Inbound::Reject(Reject::ParseError),
    }
}

/// The three top-level JSON shapes the decoder distinguishes. An array is consumed in
/// full (bounded syntax validation) with no element dispatch, then rejected as
/// `NoBatch`.
enum TopLevel {
    Object(Envelope),
    Array,
    Scalar,
}

impl<'de> serde::Deserialize<'de> for TopLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TopLevelVisitor)
    }
}

struct TopLevelVisitor;

impl<'de> Visitor<'de> for TopLevelVisitor {
    type Value = TopLevel;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON-RPC message object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        Ok(TopLevel::Object(Envelope::from_map(map)?))
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        // Validate the complete array syntactically with zero semantic dispatch: every
        // element is consumed as `IgnoredAny`. A malformed element surfaces as a
        // deserialize error (one `ParseError`); a syntactically valid array is `NoBatch`.
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(TopLevel::Array)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(TopLevel::Scalar)
    }
}

/// The decoded envelope map. `params`, `result`, and `error` presence is recorded;
/// duplicate recognized fields are a hard decode error (invalid message).
struct Envelope {
    id: IdToken,
    method: Option<String>,
    params: Option<Box<RawValue>>,
    has_result: bool,
    has_error: bool,
}

impl Envelope {
    fn from_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut id: Option<IdToken> = None;
        let mut method: Option<String> = None;
        let mut params: Option<Box<RawValue>> = None;
        let mut has_result = false;
        let mut has_error = false;
        // Track duplicate recognized fields: a second occurrence is invalid.
        let (mut saw_jsonrpc, mut saw_params) = (false, false);

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "jsonrpc" => {
                    if saw_jsonrpc {
                        return Err(de::Error::custom("duplicate jsonrpc"));
                    }
                    saw_jsonrpc = true;
                    let _: IgnoredAny = map.next_value()?;
                }
                "id" => {
                    if id.is_some() {
                        return Err(de::Error::custom("duplicate id"));
                    }
                    id = Some(map.next_value::<IdToken>()?);
                }
                "method" => {
                    if method.is_some() {
                        return Err(de::Error::custom("duplicate method"));
                    }
                    method = Some(map.next_value::<String>()?);
                }
                "params" => {
                    if saw_params {
                        return Err(de::Error::custom("duplicate params"));
                    }
                    saw_params = true;
                    params = Some(map.next_value::<Box<RawValue>>()?);
                }
                "result" => {
                    if has_result {
                        return Err(de::Error::custom("duplicate result"));
                    }
                    has_result = true;
                    let _: IgnoredAny = map.next_value()?;
                }
                "error" => {
                    if has_error {
                        return Err(de::Error::custom("duplicate error"));
                    }
                    has_error = true;
                    let _: IgnoredAny = map.next_value()?;
                }
                // Unknown members are skipped within the decoded-value bounds.
                _ => {
                    let _: IgnoredAny = map.next_value()?;
                }
            }
        }

        Ok(Self {
            id: id.unwrap_or(IdToken::Absent),
            method,
            params,
            has_result,
            has_error,
        })
    }

    /// Classify the envelope into one closed [`Inbound`] outcome.
    fn classify(self) -> Inbound {
        match self.method {
            Some(method) => match &self.id {
                // A method plus a recoverable id is a request; a method with no id is
                // a notification.
                IdToken::Absent => Inbound::Notification {
                    method,
                    params: self.params,
                },
                token => match token.recover() {
                    Some(id) => Inbound::Request {
                        id,
                        method,
                        params: self.params,
                    },
                    // A method with a present-but-invalid id (null, fractional,
                    // over-range, over-length) is an invalid request; no id recovers.
                    None => Inbound::Reject(Reject::InvalidRequest {
                        recovered_id: None,
                        reason: InvalidReason::Structural,
                    }),
                },
            },
            None => {
                // No method. A structurally valid response object — an id plus exactly
                // one of result/error — is a dropped unsolicited response.
                let has_id = !matches!(self.id, IdToken::Absent);
                if has_id && (self.has_result ^ self.has_error) {
                    Inbound::UnsolicitedResponse
                } else {
                    // Any other object is an invalid request; recover its single
                    // bounded id if it has one.
                    Inbound::Reject(Reject::InvalidRequest {
                        recovered_id: self.id.recover(),
                        reason: InvalidReason::Structural,
                    })
                }
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for IdToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(IdTokenVisitor)
    }
}

struct IdTokenVisitor;

impl<'de> Visitor<'de> for IdTokenVisitor {
    type Value = IdToken;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON-RPC id")
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(IdToken::Integer(value))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(i64::try_from(value).map_or(IdToken::Unrecoverable, IdToken::Integer))
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        // A fractional id never enters the ledger.
        Ok(IdToken::Unrecoverable)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(IdToken::Text(value.to_owned()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(IdToken::Text(value))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(IdToken::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(IdToken::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(IdToken::Unrecoverable)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(IdToken::Unrecoverable)
    }
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(IdToken::Unrecoverable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params_str(params: &Option<Box<RawValue>>) -> Option<&str> {
        params.as_ref().map(|raw| raw.get())
    }

    #[test]
    fn valid_request_recovers_integer_id_and_method() {
        let Inbound::Request { id, method, params } =
            decode(br#"{"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{"x":1}}"#)
        else {
            panic!("expected request");
        };
        assert_eq!(id, RequestId::Integer(7));
        assert_eq!(method, "textDocument/hover");
        assert_eq!(params_str(&params), Some(r#"{"x":1}"#));
    }

    #[test]
    fn integer_and_string_ids_are_distinct() {
        assert_ne!(RequestId::Integer(1), RequestId::Text("1".to_owned()));
        let Inbound::Request { id, .. } = decode(br#"{"id":"1","method":"m"}"#) else {
            panic!();
        };
        assert_eq!(id, RequestId::Text("1".to_owned()));
    }

    #[test]
    fn notification_has_no_id() {
        let Inbound::Notification { method, .. } = decode(br#"{"method":"initialized","params":{}}"#)
        else {
            panic!("expected notification");
        };
        assert_eq!(method, "initialized");
    }

    #[test]
    fn well_formed_response_is_dropped() {
        assert!(matches!(
            decode(br#"{"jsonrpc":"2.0","id":1,"result":null}"#),
            Inbound::UnsolicitedResponse
        ));
        assert!(matches!(
            decode(br#"{"id":2,"error":{"code":-1,"message":"x"}}"#),
            Inbound::UnsolicitedResponse
        ));
    }

    #[test]
    fn response_with_both_result_and_error_is_invalid_not_dropped() {
        // id + result + error is not a valid response; it recovers its id.
        assert!(matches!(
            decode(br#"{"id":9,"result":null,"error":{}}"#),
            Inbound::Reject(Reject::InvalidRequest {
                recovered_id: Some(RequestId::Integer(9)),
                reason: InvalidReason::Structural
            })
        ));
    }

    #[test]
    fn parse_error_on_invalid_json() {
        assert!(matches!(
            decode(b"{ not json"),
            Inbound::Reject(Reject::ParseError)
        ));
    }

    #[test]
    fn parse_error_on_invalid_utf8() {
        assert!(matches!(
            decode(&[0xff, 0xfe, 0x00]),
            Inbound::Reject(Reject::ParseError)
        ));
    }

    #[test]
    fn parse_error_on_trailing_json() {
        assert!(matches!(
            decode(br#"{"method":"m"}{"method":"n"}"#),
            Inbound::Reject(Reject::ParseError)
        ));
    }

    #[test]
    fn every_array_is_no_batch() {
        for body in [b"[]".as_slice(), b"[1]", br#"[{"method":"m"}]"#, b"[1,2,3]"] {
            assert!(
                matches!(
                    decode(body),
                    Inbound::Reject(Reject::InvalidRequest {
                        recovered_id: None,
                        reason: InvalidReason::NoBatch
                    })
                ),
                "array {body:?} should be NoBatch"
            );
        }
    }

    #[test]
    fn malformed_array_syntax_is_parse_error_not_no_batch() {
        assert!(matches!(
            decode(b"[1,2,"),
            Inbound::Reject(Reject::ParseError)
        ));
    }

    #[test]
    fn scalar_and_null_top_level_are_structural() {
        for body in [b"5".as_slice(), b"true", b"\"x\"", b"null"] {
            assert!(
                matches!(
                    decode(body),
                    Inbound::Reject(Reject::InvalidRequest {
                        recovered_id: None,
                        reason: InvalidReason::Structural
                    })
                ),
                "scalar {body:?} should be structural"
            );
        }
    }

    #[test]
    fn invalid_object_recovers_single_id() {
        // No method, no valid response shape, but a recoverable id.
        assert!(matches!(
            decode(br#"{"id":42,"foo":"bar"}"#),
            Inbound::Reject(Reject::InvalidRequest {
                recovered_id: Some(RequestId::Integer(42)),
                reason: InvalidReason::Structural
            })
        ));
    }

    #[test]
    fn null_fractional_and_overrange_ids_do_not_recover() {
        for body in [
            br#"{"id":null,"foo":1}"#.as_slice(),
            br#"{"id":1.5,"foo":1}"#,
            br#"{"id":9999999999,"foo":1}"#,
        ] {
            assert!(
                matches!(
                    decode(body),
                    Inbound::Reject(Reject::InvalidRequest {
                        recovered_id: None,
                        ..
                    })
                ),
                "id in {body:?} must not recover"
            );
        }
    }

    #[test]
    fn method_with_null_id_is_invalid_request_no_recovery() {
        assert!(matches!(
            decode(br#"{"id":null,"method":"m"}"#),
            Inbound::Reject(Reject::InvalidRequest {
                recovered_id: None,
                reason: InvalidReason::Structural
            })
        ));
    }

    #[test]
    fn overlong_string_id_does_not_recover() {
        let long = "x".repeat(MAX_REQUEST_ID_STRING_BYTES + 1);
        let body = format!(r#"{{"id":"{long}","foo":1}}"#);
        assert!(matches!(
            decode(body.as_bytes()),
            Inbound::Reject(Reject::InvalidRequest {
                recovered_id: None,
                ..
            })
        ));
    }

    #[test]
    fn duplicate_recognized_field_is_invalid() {
        // Two `method` members: invalid message (decode error -> ParseError).
        assert!(matches!(
            decode(br#"{"method":"a","method":"b"}"#),
            Inbound::Reject(Reject::ParseError)
        ));
        assert!(matches!(
            decode(br#"{"id":1,"id":2,"method":"m"}"#),
            Inbound::Reject(Reject::ParseError)
        ));
    }

    #[test]
    fn unknown_members_are_skipped() {
        let Inbound::Request { id, method, .. } =
            decode(br#"{"extra":{"deep":[1,2]},"id":3,"method":"m","more":true}"#)
        else {
            panic!("expected request");
        };
        assert_eq!(id, RequestId::Integer(3));
        assert_eq!(method, "m");
    }

    #[test]
    fn request_with_absent_params_is_none() {
        let Inbound::Request { params, .. } = decode(br#"{"id":1,"method":"shutdown"}"#) else {
            panic!();
        };
        assert!(params.is_none());
    }
}
