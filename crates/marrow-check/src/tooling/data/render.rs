use std::collections::HashMap;

use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::{DataPathSegment, decode_tree_enum_member};
use marrow_store::value::{SavedValue, decode_value, encode_value};

use super::{DataQuery, DataQuerySegment};
use crate::{CheckedProgram, EnumId, StoreLeafKind, identity_leaf_key_mismatch};

const UNDECLARED_MEMBER: &str = "<undeclared member>";

pub fn render_query_segments(segments: &[DataQuerySegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            DataQuerySegment::Root(name) => {
                text.push('^');
                text.push_str(name);
            }
            DataQuerySegment::Field(name) | DataQuerySegment::Layer(name) => {
                text.push('.');
                text.push_str(name);
            }
            DataQuerySegment::Key(key) => {
                push_key(&mut text, key);
            }
        }
    }
    text
}

pub(crate) fn push_member(path: &mut String, name: &str) -> usize {
    let prior_len = path.len();
    path.push('.');
    path.push_str(name);
    prior_len
}

/// Append a stored data path under an already-rendered root and identity,
/// resolving each member's catalog id to its declared source name.
pub(crate) fn render_data_path(
    text: &mut String,
    path: &[DataPathSegment],
    names: &HashMap<String, String>,
) {
    for segment in path {
        match segment {
            DataPathSegment::Member(member) => {
                let name = names
                    .get(member.as_str())
                    .map(String::as_str)
                    .unwrap_or(UNDECLARED_MEMBER);
                push_member(text, name);
            }
            DataPathSegment::Key(key) => {
                push_key(text, key);
            }
        }
    }
}

pub(crate) fn push_key(path: &mut String, key: &SavedKey) -> usize {
    let prior_len = path.len();
    path.push('(');
    path.push_str(&render_key(key));
    path.push(')');
    prior_len
}

pub fn render_data_value(program: &CheckedProgram, leaf: &StoreLeafKind, bytes: &[u8]) -> String {
    match leaf {
        StoreLeafKind::Scalar(ty) => {
            render_scalar_value(*ty, bytes).unwrap_or_else(|| render_hex_value(bytes))
        }
        StoreLeafKind::Identity { store_root, arity } => {
            render_identity_value(program, store_root, *arity, bytes)
                .unwrap_or_else(|| render_hex_value(bytes))
        }
        StoreLeafKind::Enum { enum_id } => {
            render_enum_value(program, *enum_id, bytes).unwrap_or_else(|| render_hex_value(bytes))
        }
    }
}

pub fn render_data_query_value(
    program: &CheckedProgram,
    query: &DataQuery,
    bytes: &[u8],
) -> String {
    match query.leaf() {
        Some(leaf) => render_data_value(program, leaf, bytes),
        None => render_hex_value(bytes),
    }
}

fn render_scalar_value(ty: marrow_store::value::ScalarType, bytes: &[u8]) -> Option<String> {
    match decode_value(bytes, ty)? {
        SavedValue::Str(value) => Some(format!("{value:?}")),
        SavedValue::Bytes(value) => Some(render_hex_value(&value)),
        SavedValue::Bool(value) => Some(value.to_string()),
        value => render_encoded_scalar(value),
    }
}

fn render_encoded_scalar(value: SavedValue) -> Option<String> {
    String::from_utf8(encode_value(&value).ok()?).ok()
}

fn render_identity_value(
    program: &CheckedProgram,
    store_root: &str,
    arity: usize,
    bytes: &[u8],
) -> Option<String> {
    let keys = decode_identity_payload_arity(bytes, arity)?;
    if identity_leaf_key_mismatch(program, store_root, &keys).is_some() {
        return None;
    }
    let mut segments = Vec::with_capacity(1 + keys.len());
    segments.push(DataQuerySegment::Root(store_root.to_string()));
    segments.extend(keys.into_iter().map(DataQuerySegment::Key));
    Some(render_query_segments(&segments))
}

fn render_enum_value(program: &CheckedProgram, enum_id: EnumId, bytes: &[u8]) -> Option<String> {
    let stored = decode_tree_enum_member(bytes).ok()?;
    let enum_fact = program.facts.enum_(enum_id)?;
    if enum_fact.catalog_id.as_deref() != Some(stored.enum_id().as_str()) {
        return None;
    }
    let member = program.facts.enum_members().iter().find(|member| {
        member.enum_id == enum_id
            && member.catalog_id.as_deref() == Some(stored.member_id().as_str())
    })?;
    if !program.facts.enum_member_is_selectable(member.id) {
        return None;
    }
    program
        .facts
        .enum_member_catalog_path(&program.modules, member.id)
}

fn render_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Bytes(value) => render_hex_value(value),
        SavedKey::Date(value) => render_key_temporal(SavedValue::Date(*value)),
        SavedKey::Instant(value) => render_key_temporal(SavedValue::Instant(*value)),
        SavedKey::Duration(value) => render_key_temporal(SavedValue::Duration(*value)),
    }
}

fn render_hex_value(bytes: &[u8]) -> String {
    let mut text = String::from("0x");
    push_hex(&mut text, bytes);
    text
}

fn render_key_temporal(value: SavedValue) -> String {
    String::from_utf8(encode_value(&value).expect("temporal key values encode"))
        .expect("temporal key encodings are ascii")
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    use std::fmt::Write;

    for byte in bytes {
        write!(out, "{byte:02x}").expect("write hex");
    }
}
