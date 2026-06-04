use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};

use marrow_check::CheckedProgram;
use marrow_store::key::SavedKey;
use serde_json::{Value, json};

use crate::cmd_data::get::{DataQuery, DataQuerySegment, resolve_data_query};

use super::codec::{
    decode_base64_field, decode_key, decode_query_path, encode_key, encode_query_path,
};
use super::{ProtocolError, bad_request};

pub(super) struct CursorState {
    key: RandomState,
}

impl CursorState {
    pub(super) fn new() -> Self {
        Self {
            key: RandomState::new(),
        }
    }

    pub(super) fn encode_cursor(&self, scope: &str, path: &[DataQuerySegment]) -> String {
        let path = encode_query_path(path);
        marrow_run::base64::encode(
            json!({ "v": 2, "scope": scope, "path": path, "sig": self.signature(scope, &path) })
                .to_string()
                .as_bytes(),
        )
    }

    pub(super) fn decode_cursor(
        &self,
        program: &CheckedProgram,
        value: &Value,
        prefix: &DataQuery,
    ) -> Result<DataQuery, ProtocolError> {
        let cursor = decode_base64_field(value, "cursor")?;
        let cursor =
            String::from_utf8(cursor).map_err(|_| bad_request("`cursor` is not a checked path"))?;
        let cursor: Value = serde_json::from_str(&cursor)
            .map_err(|_| bad_request("`cursor` is not a debug_data_walk cursor"))?;
        let object = cursor
            .as_object()
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_walk cursor"))?;
        if object.get("v").and_then(Value::as_u64) != Some(2) {
            return Err(bad_request("`cursor` is not a debug_data_walk cursor"));
        }
        let path = object
            .get("path")
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_walk cursor"))?;
        let scope = object
            .get("scope")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_walk cursor"))?;
        let sig = object
            .get("sig")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_walk cursor"))?;
        if sig != self.signature(scope, path) {
            return Err(bad_request("`cursor` is not a debug_data_walk cursor"));
        }
        if scope != prefix.path {
            return Err(bad_request("`cursor` is outside the requested path"));
        }
        let segments = decode_query_path(path)?;
        let query =
            resolve_data_query(program, &segments).map_err(|message| bad_request(&message))?;
        if !query_under_prefix(&query, prefix) {
            return Err(bad_request("`cursor` is outside the requested path"));
        }
        Ok(query)
    }

    /// Encode an opaque resume cursor for `debug_data_children` paging: the parent
    /// path scope plus the last child key this page returned. Sent back on the same
    /// connection with the same `path`, it resumes the child scan after that key.
    pub(super) fn encode_children_cursor(&self, scope: &str, after: &SavedKey) -> String {
        let key = encode_key(after);
        marrow_run::base64::encode(
            json!({ "v": 1, "scope": scope, "after": key, "sig": self.children_signature(scope, &key) })
                .to_string()
                .as_bytes(),
        )
    }

    /// Decode a `debug_data_children` resume cursor, validating its signature and
    /// that its scope matches the request `path`. Returns the child key the next
    /// page resumes after.
    pub(super) fn decode_children_cursor(
        &self,
        value: &Value,
        scope: &str,
    ) -> Result<SavedKey, ProtocolError> {
        let cursor = decode_base64_field(value, "cursor")?;
        let cursor = String::from_utf8(cursor)
            .map_err(|_| bad_request("`cursor` is not a debug_data_children cursor"))?;
        let cursor: Value = serde_json::from_str(&cursor)
            .map_err(|_| bad_request("`cursor` is not a debug_data_children cursor"))?;
        let object = cursor
            .as_object()
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_children cursor"))?;
        if object.get("v").and_then(Value::as_u64) != Some(1) {
            return Err(bad_request("`cursor` is not a debug_data_children cursor"));
        }
        let after = object
            .get("after")
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_children cursor"))?;
        let cursor_scope = object
            .get("scope")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_children cursor"))?;
        let sig = object
            .get("sig")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request("`cursor` is not a debug_data_children cursor"))?;
        if sig != self.children_signature(cursor_scope, after) {
            return Err(bad_request("`cursor` is not a debug_data_children cursor"));
        }
        if cursor_scope != scope {
            return Err(bad_request("`cursor` is outside the requested path"));
        }
        decode_key(after)
    }

    fn signature(&self, scope: &str, path: &Value) -> String {
        let mut hasher = self.key.build_hasher();
        "marrow:debug_data_walk:cursor:v1".hash(&mut hasher);
        scope.hash(&mut hasher);
        path.to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn children_signature(&self, scope: &str, after: &Value) -> String {
        let mut hasher = self.key.build_hasher();
        "marrow:debug_data_children:cursor:v1".hash(&mut hasher);
        scope.hash(&mut hasher);
        after.to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

pub(super) fn query_under_prefix(query: &DataQuery, prefix: &DataQuery) -> bool {
    if query.store != prefix.store || query.identity_arity != prefix.identity_arity {
        return false;
    }
    if !query.identity.starts_with(&prefix.identity) {
        return false;
    }
    if prefix.identity.len() < prefix.identity_arity {
        return true;
    }
    query.identity.len() == prefix.identity_arity && query.data_path.starts_with(&prefix.data_path)
}
