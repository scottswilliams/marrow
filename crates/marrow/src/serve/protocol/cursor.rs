use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};

use marrow_check::CheckedProgram;
use serde_json::{Value, json};

use crate::cmd_data::get::{DataQuery, DataQuerySegment, resolve_data_query};

use super::codec::{decode_base64_field, decode_query_path, encode_query_path};
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

    fn signature(&self, scope: &str, path: &Value) -> String {
        let mut hasher = self.key.build_hasher();
        "marrow:debug_data_walk:cursor:v1".hash(&mut hasher);
        scope.hash(&mut hasher);
        path.to_string().hash(&mut hasher);
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
