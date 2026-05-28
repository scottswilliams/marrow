//! The `marrow serve` request/response protocol.
//!
//! Each request and reply is one JSON object on its own line (newline-delimited).
//! This module is the transport-free core: [`handle_request`] turns one request
//! value into one reply value against a [`Backend`], so it is unit-tested without
//! sockets. A reply is `{"id": …, "ok": …}` on success or
//! `{"id": …, "error": {"code": …, "message": …}}` on failure, echoing the
//! request's `id`.
//!
//! This is a read-only tooling surface: it never writes managed data. Slice A
//! serves `saved_roots`; path-addressed reads (`saved_get`, `saved_children`,
//! `saved_walk`) are later slices.

use marrow_store::backend::Backend;
use serde_json::{Value, json};

/// A request was malformed — not an object, or missing a string `op`.
pub const PROTOCOL_MALFORMED: &str = "protocol.malformed";
/// A request named an operation the server does not support.
pub const PROTOCOL_UNKNOWN_OP: &str = "protocol.unknown_op";

/// A protocol-level failure (a malformed or unsupported request). A storage
/// failure is surfaced separately, carrying the store's own `store.*` code.
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
    use marrow_store::path::{PathSegment, SavedKey, encode_path};

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
}
