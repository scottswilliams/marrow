//! The `marrow serve` request/response protocol.
//!
//! Each request and reply is one JSON object on its own line (newline-delimited).
//! This module is the transport-free core: [`handle_request`] turns one request
//! value into one reply value against a [`Backend`], so it is unit-tested without
//! sockets. A reply is `{"id": …, "ok": …}` on success or
//! `{"id": …, "error": {"code": …, "message": …}}` on failure, echoing the
//! request's `id`.
//!
//! This is a read-only tooling surface: it never writes managed data. It serves
//! `saved_roots` and the path-addressed reads `saved_get`, `saved_children`, and
//! `saved_walk`.

use marrow_run::base64;
use marrow_store::backend::Backend;
use marrow_store::backend::Presence;
use marrow_store::path::{ChildSegment, PathSegment, SavedKey, encode_path};
use serde_json::{Value, json};

/// A request was malformed — not an object, or missing a string `op`.
pub const PROTOCOL_MALFORMED: &str = "protocol.malformed";
/// A request named an operation the server does not support.
pub const PROTOCOL_UNKNOWN_OP: &str = "protocol.unknown_op";
/// A request named a known operation but its arguments were malformed — a bad
/// path segment, an unknown key type, or a missing `path`.
pub const PROTOCOL_BAD_REQUEST: &str = "protocol.bad_request";

/// A protocol-level failure (a malformed or unsupported request). A storage
/// failure is surfaced separately, carrying the store's own `store.*` code.
#[derive(Debug)]
struct ProtocolError {
    code: &'static str,
    message: String,
}

/// Handle one request, returning its reply. Never fails: every error — protocol
/// or storage — becomes an `error` reply that echoes the request's `id`.
pub fn handle_request(store: &dyn Backend, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    match dispatch(store, request) {
        Ok(result) => json!({ "id": id, "ok": result }),
        Err(error) => json!({
            "id": id,
            "error": { "code": error.code, "message": error.message },
        }),
    }
}

/// Route a request to its operation handler by the `op` field.
fn dispatch(store: &dyn Backend, request: &Value) -> Result<Value, ProtocolError> {
    let op = request
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError {
            code: PROTOCOL_MALFORMED,
            message: "request is missing a string `op`".to_string(),
        })?;
    match op {
        "saved_roots" => op_saved_roots(store),
        "saved_get" => op_saved_get(store, request),
        "saved_children" => op_saved_children(store, request),
        "saved_walk" => op_saved_walk(store, request),
        other => Err(ProtocolError {
            code: PROTOCOL_UNKNOWN_OP,
            message: format!("unknown operation `{other}`"),
        }),
    }
}

/// `saved_roots` → the project's saved root names, in store order.
fn op_saved_roots(store: &dyn Backend) -> Result<Value, ProtocolError> {
    let roots = store.roots().map_err(store_error)?;
    Ok(json!({ "roots": roots }))
}

/// `saved_get` → the four-state presence at a saved path plus its stored value as
/// base64 (`null` when no value is stored there). The bytes are the store's raw
/// canonical encoding; the client decodes them with the field's schema type.
fn op_saved_get(store: &dyn Backend, request: &Value) -> Result<Value, ProtocolError> {
    let path = encode_path(&request_path(request)?);
    let presence = store.presence(&path).map_err(store_error)?;
    let value = store.read(&path).map_err(store_error)?;
    Ok(json!({
        "presence": presence_name(presence),
        "value": value.map(|bytes| base64::encode(&bytes)),
    }))
}

/// `saved_children` → the distinct immediate children directly below a saved path,
/// in Marrow order: each is a `{"key": …}` (a record/index key) or `{"name": …}`
/// (a field, layer, or index name).
fn op_saved_children(store: &dyn Backend, request: &Value) -> Result<Value, ProtocolError> {
    let path = encode_path(&request_path(request)?);
    let children = store.child_keys(&path).map_err(store_error)?;
    let children: Vec<Value> = children.iter().map(encode_child).collect();
    Ok(json!({ "children": children }))
}

/// The largest `saved_walk` page the server returns, so an unbounded request
/// cannot force a huge scan. A client pages by walking deeper subtrees.
const MAX_WALK: usize = 10_000;

/// `saved_walk` → up to `limit` `(path, value)` entries in the subtree at a saved
/// path, in Marrow order, plus whether the page was truncated. Each entry's path
/// and value are base64 (the path bytes are opaque to the client in v1). The
/// `limit` is required and clamped to [`MAX_WALK`].
fn op_saved_walk(store: &dyn Backend, request: &Value) -> Result<Value, ProtocolError> {
    let path = encode_path(&request_path(request)?);
    let limit = request
        .get("limit")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad_request("`saved_walk` requires an integer `limit`"))?;
    let limit = limit.min(MAX_WALK as u64) as usize;
    let page = store.scan(&path, limit).map_err(store_error)?;
    let entries: Vec<Value> = page
        .entries
        .iter()
        .map(
            |(path, value)| json!({ "path": base64::encode(path), "value": base64::encode(value) }),
        )
        .collect();
    Ok(json!({ "entries": entries, "truncated": page.truncated }))
}

/// The decoded `path` of a request, or a `protocol.bad_request` error.
fn request_path(request: &Value) -> Result<Vec<PathSegment>, ProtocolError> {
    let path = request
        .get("path")
        .ok_or_else(|| bad_request("request is missing `path`"))?;
    decode_path(path)
}

/// The protocol name for a [`Presence`] state.
fn presence_name(presence: Presence) -> &'static str {
    match presence {
        Presence::Absent => "absent",
        Presence::ValueOnly => "value_only",
        Presence::ChildrenOnly => "children_only",
        Presence::ValueAndChildren => "value_and_children",
    }
}

/// Decode a `path` value — a JSON array of segment objects — into path segments.
fn decode_path(value: &Value) -> Result<Vec<PathSegment>, ProtocolError> {
    value
        .as_array()
        .ok_or_else(|| bad_request("`path` must be an array of segments"))?
        .iter()
        .map(decode_segment)
        .collect()
}

/// Decode one path segment: a one-field object tagged by its kind, e.g.
/// `{"root":"books"}`, `{"key":{"int":1}}`, `{"layer":"versions"}`.
fn decode_segment(value: &Value) -> Result<PathSegment, ProtocolError> {
    let (kind, inner) = one_field(value, "a path segment")?;
    let segment = match kind.as_str() {
        "root" => PathSegment::Root(segment_name(inner, "root")?),
        "key" => PathSegment::RecordKey(decode_key(inner)?),
        "field" => PathSegment::Field(segment_name(inner, "field")?),
        "layer" => PathSegment::ChildLayer(segment_name(inner, "layer")?),
        "index" => PathSegment::Index(segment_name(inner, "index")?),
        "index_key" => PathSegment::IndexKey(decode_key(inner)?),
        other => return Err(bad_request(&format!("unknown path segment `{other}`"))),
    };
    Ok(segment)
}

/// A path segment's string name (for `root`/`field`/`layer`/`index`).
fn segment_name(value: &Value, kind: &str) -> Result<String, ProtocolError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| bad_request(&format!("`{kind}` must name a string")))
}

/// Decode a key value — a one-field object tagged by its type — into a [`SavedKey`].
/// The accepted tags are the [`SavedKey::wire_tag`] of each key kind, so they
/// stay in lockstep with [`encode_key`] and the shared scalar-name table. Wide
/// integer keys (`duration`, `instant`) are strings because JSON numbers cannot
/// hold an `i128`; `int` and `date` are JSON numbers; `bytes` is base64.
fn decode_key(value: &Value) -> Result<SavedKey, ProtocolError> {
    let (tag, inner) = one_field(value, "a key")?;
    let tag = tag.as_str();
    let key = if tag == SavedKey::Int(0).wire_tag() {
        SavedKey::Int(
            inner
                .as_i64()
                .ok_or_else(|| bad_request("`int` key must be an integer"))?,
        )
    } else if tag == SavedKey::Bool(false).wire_tag() {
        SavedKey::Bool(
            inner
                .as_bool()
                .ok_or_else(|| bad_request("`bool` key must be a boolean"))?,
        )
    } else if tag == SavedKey::Str(String::new()).wire_tag() {
        SavedKey::Str(segment_name(inner, "str")?)
    } else if tag == SavedKey::Date(0).wire_tag() {
        let days = inner
            .as_i64()
            .ok_or_else(|| bad_request("`date` key must be an integer"))?;
        SavedKey::Date(i32::try_from(days).map_err(|_| bad_request("`date` key is out of range"))?)
    } else if tag == SavedKey::Duration(0).wire_tag() {
        SavedKey::Duration(parse_i128(inner, "duration")?)
    } else if tag == SavedKey::Instant(0).wire_tag() {
        SavedKey::Instant(parse_i128(inner, "instant")?)
    } else if tag == SavedKey::Bytes(Vec::new()).wire_tag() {
        SavedKey::Bytes(decode_base64_field(inner, "bytes")?)
    } else {
        return Err(bad_request(&format!("unknown key type `{tag}`")));
    };
    Ok(key)
}

/// Encode a [`SavedKey`] back to its one-field JSON form (the inverse of
/// [`decode_key`]), used for `saved_children` output. The tag comes from
/// [`SavedKey::wire_tag`] (sourced from the shared scalar-name table) in the same
/// match as the payload, so tag and value cannot disagree.
fn encode_key(key: &SavedKey) -> Value {
    let tag = key.wire_tag();
    let payload = match key {
        SavedKey::Int(value) => json!(value),
        SavedKey::Bool(value) => json!(value),
        SavedKey::Str(value) => json!(value),
        SavedKey::Date(value) => json!(value),
        SavedKey::Duration(value) => json!(value.to_string()),
        SavedKey::Instant(value) => json!(value.to_string()),
        SavedKey::Bytes(value) => json!(base64::encode(value)),
    };
    json!({ tag: payload })
}

/// Encode a child of a saved path: a key value or a member name.
fn encode_child(child: &ChildSegment) -> Value {
    match child {
        ChildSegment::Key(key) => json!({ "key": encode_key(key) }),
        ChildSegment::Name(name) => json!({ "name": name }),
    }
}

/// The single `(tag, value)` of a one-field object, or a `protocol.bad_request`.
fn one_field<'a>(value: &'a Value, what: &str) -> Result<(&'a String, &'a Value), ProtocolError> {
    let object = value
        .as_object()
        .ok_or_else(|| bad_request(&format!("{what} must be a one-field object")))?;
    if object.len() != 1 {
        return Err(bad_request(&format!("{what} must have exactly one tag")));
    }
    Ok(object.iter().next().expect("exactly one field"))
}

/// Parse a wide integer carried as a decimal string (JSON numbers cannot hold an
/// `i128`).
fn parse_i128(value: &Value, kind: &str) -> Result<i128, ProtocolError> {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| bad_request(&format!("`{kind}` key must be an integer in a string")))
}

/// Decode a base64 string field, or a `protocol.bad_request`.
fn decode_base64_field(value: &Value, kind: &str) -> Result<Vec<u8>, ProtocolError> {
    let text = value
        .as_str()
        .ok_or_else(|| bad_request(&format!("`{kind}` must be a base64 string")))?;
    base64::decode(text).ok_or_else(|| bad_request(&format!("`{kind}` is not valid base64")))
}

/// Build a `protocol.bad_request` error.
fn bad_request(message: &str) -> ProtocolError {
    ProtocolError {
        code: PROTOCOL_BAD_REQUEST,
        message: message.to_string(),
    }
}

/// Carry a storage failure through with its own stable `store.*` code.
fn store_error(error: marrow_store::StoreError) -> ProtocolError {
    ProtocolError {
        code: error.code(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marrow_store::mem::MemStore;

    /// A store holding one record under `^books`, for root listing.
    fn store_with_a_book() -> MemStore {
        let mut store = MemStore::new();
        let path = encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::Field("title".into()),
        ]);
        store.write(&path, b"Mort".to_vec());
        store
    }

    #[test]
    fn saved_roots_lists_the_roots_and_echoes_the_id() {
        let store = store_with_a_book();
        let reply = handle_request(&store, &json!({ "id": 7, "op": "saved_roots" }));
        assert_eq!(reply["id"], json!(7));
        assert_eq!(reply["ok"]["roots"], json!(["books"]));
    }

    #[test]
    fn an_empty_store_lists_no_roots() {
        let store = MemStore::new();
        let reply = handle_request(&store, &json!({ "id": 1, "op": "saved_roots" }));
        assert_eq!(reply["ok"]["roots"], json!([]));
    }

    #[test]
    fn an_unknown_op_is_a_protocol_error() {
        let store = MemStore::new();
        let reply = handle_request(&store, &json!({ "id": 1, "op": "frobnicate" }));
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_UNKNOWN_OP));
    }

    #[test]
    fn a_request_without_an_op_is_malformed_and_echoes_a_null_id() {
        let store = MemStore::new();
        let reply = handle_request(&store, &json!({ "what": true }));
        assert_eq!(reply["id"], Value::Null);
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_MALFORMED));
    }

    #[test]
    fn saved_get_returns_presence_and_the_base64_value() {
        let store = store_with_a_book();
        let reply = handle_request(
            &store,
            &json!({
                "id": 1, "op": "saved_get",
                "path": [{"root": "books"}, {"key": {"int": 1}}, {"field": "title"}],
            }),
        );
        // A leaf field holds a value and no children, and "Mort" base64-encodes to
        // "TW9ydA==".
        assert_eq!(reply["ok"]["presence"], json!("value_only"));
        assert_eq!(reply["ok"]["value"], json!("TW9ydA=="));
    }

    #[test]
    fn saved_get_of_an_absent_path_has_no_value() {
        let store = store_with_a_book();
        let reply = handle_request(
            &store,
            &json!({
                "op": "saved_get",
                "path": [{"root": "books"}, {"key": {"int": 2}}, {"field": "title"}],
            }),
        );
        assert_eq!(reply["ok"]["presence"], json!("absent"));
        assert_eq!(reply["ok"]["value"], Value::Null);
    }

    #[test]
    fn saved_children_lists_record_keys_then_field_names() {
        let store = store_with_a_book();
        let under_root = handle_request(
            &store,
            &json!({ "op": "saved_children", "path": [{"root": "books"}] }),
        );
        assert_eq!(under_root["ok"]["children"], json!([{"key": {"int": 1}}]));
        let under_record = handle_request(
            &store,
            &json!({ "op": "saved_children", "path": [{"root": "books"}, {"key": {"int": 1}}] }),
        );
        assert_eq!(under_record["ok"]["children"], json!([{"name": "title"}]));
    }

    #[test]
    fn a_bad_path_segment_is_a_bad_request() {
        let store = MemStore::new();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_get", "path": [{"frob": "x"}] }),
        );
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    }

    #[test]
    fn a_saved_get_without_a_path_is_a_bad_request() {
        let store = MemStore::new();
        let reply = handle_request(&store, &json!({ "op": "saved_get" }));
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    }

    #[test]
    fn keys_of_every_type_round_trip_through_the_codec() {
        // Wide integers (duration/instant) carry as strings; bytes as base64.
        for key in [
            SavedKey::Int(7),
            SavedKey::Bool(true),
            SavedKey::Str("x".into()),
            SavedKey::Date(19_000),
            SavedKey::Duration(123_000_000_000),
            SavedKey::Instant(-5),
            SavedKey::Bytes(vec![0, 1, 2, 255]),
        ] {
            assert_eq!(decode_key(&encode_key(&key)).expect("decode"), key);
        }
    }

    #[test]
    fn base64_round_trips_arbitrary_bytes() {
        for bytes in [
            Vec::new(),
            vec![0u8],
            vec![1, 2],
            vec![1, 2, 3],
            b"Mort".to_vec(),
            vec![0, 255, 128, 64, 32],
        ] {
            assert_eq!(base64::decode(&base64::encode(&bytes)), Some(bytes));
        }
    }

    /// The serve protocol decodes base64 through the one canonical codec, so it
    /// rejects exactly the unpadded and over-padded inputs the runtime rejects —
    /// no second, laxer dialect on the serve surface.
    #[test]
    fn serve_base64_decode_rejects_non_canonical_padding() {
        // These were accepted by the old padding-trimming serve decoder but
        // rejected by the runtime; now both agree they are invalid.
        for text in ["Zm8", "Zg", "Zm9vYg", "Zg===="] {
            assert!(
                decode_base64_field(&json!(text), "key").is_err(),
                "non-canonical base64 {text:?} must be rejected"
            );
            // The shared codec backs the rejection.
            assert_eq!(base64::decode(text), None, "{text:?}");
        }
        // The canonical, fully-padded forms decode.
        assert_eq!(
            decode_base64_field(&json!("Zm8="), "key").expect("padded"),
            b"fo".to_vec()
        );
        assert_eq!(
            decode_base64_field(&json!("Zm9vYg=="), "key").expect("padded"),
            b"foob".to_vec()
        );
    }

    /// A store holding two book titles, for paging.
    fn store_with_two_books() -> MemStore {
        let mut store = MemStore::new();
        for (id, title) in [(1, "Mort"), (2, "Sourcery")] {
            let path = encode_path(&[
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(id)),
                PathSegment::Field("title".into()),
            ]);
            store.write(&path, title.as_bytes().to_vec());
        }
        store
    }

    #[test]
    fn saved_walk_truncates_at_the_limit() {
        let store = store_with_two_books();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_walk", "path": [{"root": "books"}], "limit": 1 }),
        );
        assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 1);
        assert_eq!(reply["ok"]["truncated"], json!(true));
    }

    #[test]
    fn saved_walk_returns_the_whole_subtree_under_a_generous_limit() {
        let store = store_with_two_books();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_walk", "path": [{"root": "books"}], "limit": 100 }),
        );
        assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 2);
        assert_eq!(reply["ok"]["truncated"], json!(false));
    }

    #[test]
    fn saved_walk_without_a_limit_is_a_bad_request() {
        let store = MemStore::new();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_walk", "path": [{"root": "books"}] }),
        );
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    }

    #[test]
    fn an_unknown_key_type_is_a_bad_request() {
        let store = MemStore::new();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_get", "path": [{"root": "books"}, {"key": {"frob": 1}}] }),
        );
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    }

    #[test]
    fn a_bytes_key_with_invalid_base64_is_a_bad_request() {
        let store = MemStore::new();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_get", "path": [{"root": "books"}, {"key": {"bytes": "!!!"}}] }),
        );
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    }

    #[test]
    fn a_wide_integer_key_that_is_not_an_integer_is_a_bad_request() {
        let store = MemStore::new();
        let reply = handle_request(
            &store,
            &json!({ "op": "saved_get", "path": [{"root": "books"}, {"key": {"duration": "notanint"}}] }),
        );
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    }
}
