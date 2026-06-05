use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};

use marrow_check::CheckedProgram;
use marrow_check::tooling::{
    DataQuery, DataQuerySegment, data_query_under_prefix, resolve_data_query,
};
use marrow_store::key::SavedKey;
use serde_json::{Value, json};

use super::codec::{
    decode_base64_field, decode_key, decode_query_path, encode_key, encode_query_path,
};
use super::{ProtocolError, bad_request, tooling_error};

pub(super) struct CursorState {
    key: RandomState,
}

/// The per-operation parameters of a signed cursor envelope: the version it was
/// stamped with, the inner payload field name, the scope it must match, the
/// operation label used in the malformed-cursor message, and the signature
/// function that reproduces the stamp.
struct SignedEnvelope<'a> {
    version: u64,
    field: &'a str,
    scope: &'a str,
    label: &'a str,
    signature: &'a dyn Fn(&str, &Value) -> String,
}

impl SignedEnvelope<'_> {
    fn malformed(&self) -> ProtocolError {
        bad_request(&format!("`cursor` is not a {} cursor", self.label))
    }
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
        let path = self.decode_signed_envelope(
            value,
            &SignedEnvelope {
                version: 2,
                field: "path",
                scope: prefix.path(),
                label: "debug_data_walk",
                signature: &|scope, field| self.signature(scope, field),
            },
        )?;
        let segments = decode_query_path(&path)?;
        let query = resolve_data_query(program, &segments).map_err(tooling_error)?;
        if !data_query_under_prefix(&query, prefix) {
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
        let after = self.decode_signed_envelope(
            value,
            &SignedEnvelope {
                version: 1,
                field: "after",
                scope,
                label: "debug_data_children",
                signature: &|scope, field| self.children_signature(scope, field),
            },
        )?;
        decode_key(&after)
    }

    /// Validate a signed cursor envelope and return its inner field.
    ///
    /// Both cursor flavors share one base64 -> utf8 -> json -> object ->
    /// version -> field -> scope -> signature contract; only the version, the
    /// inner field name, and the signature seed differ. A structural or
    /// signature failure is reported uniformly so a client cannot tell a
    /// malformed envelope from a forged signature: revealing which check failed
    /// would help an attacker forge a valid token. A scope mismatch is a
    /// distinct, safe-to-report condition (the cursor was issued for another
    /// path), so it keeps its own message.
    fn decode_signed_envelope(
        &self,
        value: &Value,
        envelope: &SignedEnvelope<'_>,
    ) -> Result<Value, ProtocolError> {
        let cursor = decode_base64_field(value, "cursor")?;
        let cursor = String::from_utf8(cursor).map_err(|_| envelope.malformed())?;
        let cursor: Value = serde_json::from_str(&cursor).map_err(|_| envelope.malformed())?;
        let object = cursor.as_object().ok_or_else(|| envelope.malformed())?;
        if object.get("v").and_then(Value::as_u64) != Some(envelope.version) {
            return Err(envelope.malformed());
        }
        let field = object
            .get(envelope.field)
            .ok_or_else(|| envelope.malformed())?;
        let scope = object
            .get("scope")
            .and_then(Value::as_str)
            .ok_or_else(|| envelope.malformed())?;
        let sig = object
            .get("sig")
            .and_then(Value::as_str)
            .ok_or_else(|| envelope.malformed())?;
        if sig != (envelope.signature)(scope, field) {
            return Err(envelope.malformed());
        }
        if scope != envelope.scope {
            return Err(bad_request("`cursor` is outside the requested path"));
        }
        Ok(field.clone())
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
