//! Canonical value→text rendering: the single owner of the E03w text forms for every
//! runtime value. The VM renders interpolation holes and `string(...)` through
//! [`value_text`]; the CLI's `run` output delegates to the same owner, so one
//! function renders each canonical form and no display path forks. The renderer is
//! total over [`Value`] — an enum payload or a returned value can be any shape,
//! including a record, list, map, or optional — so it never faults on a value the
//! verifier admitted.

use std::fmt::Write;

use marrow_kernel::codec::key::KeyScalar;
use marrow_verify::{SealedEnumType, SealedRecordType};

use crate::Value;

/// `0x`-prefixed lowercase hex, the canonical `bytes` rendering.
pub fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The canonical `YYYY-MM-DD` text of a date. A validated date always formats; a raw
/// day outside the supported range (only reachable from a hand-built value) falls back
/// to its integer so rendering never fails.
pub fn date_text(days: i32) -> String {
    marrow_temporal::format_date(days).unwrap_or_else(|| days.to_string())
}

/// The canonical UTC text of an instant, with the same out-of-range fallback.
pub fn instant_text(nanos: i128) -> String {
    marrow_temporal::format_instant(nanos).unwrap_or_else(|| nanos.to_string())
}

/// The canonical scalar text of a map or identity key column.
pub fn key_text(key: &KeyScalar) -> String {
    match key {
        KeyScalar::Int(v) => v.to_string(),
        KeyScalar::Bool(v) => v.to_string(),
        KeyScalar::Str(v) => v.clone(),
        KeyScalar::Bytes(v) => hex_bytes(v),
        KeyScalar::Date(v) => date_text(*v),
        KeyScalar::Instant(v) => instant_text(*v),
        KeyScalar::Duration(v) => marrow_temporal::format_duration(*v),
    }
}

/// `Id(k0, k1)` — the key tuple that addresses an entry, each column canonical. A
/// program declares one store root, so the tuple identifies it without a discriminator.
pub fn id_text(keys: &[KeyScalar]) -> String {
    let mut out = String::from("Id(");
    for (position, key) in keys.iter().enumerate() {
        if position > 0 {
            out.push_str(", ");
        }
        out.push_str(&key_text(key));
    }
    out.push(')');
    out
}

/// `Enum::member` or `Enum::member(payload, ...)`, the declared and member names read
/// from the sealed enum types. Payload columns render through [`value_text`]; a toolchain
/// generic enum (`Option`/`Result`) carries an arbitrary payload value this way.
pub fn enum_text(
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    enum_idx: u16,
    variant: u16,
    payload: &[Value],
) -> String {
    let enum_def = enums.get(enum_idx as usize);
    let variant_def = enum_def.and_then(|e| e.variants().get(variant as usize));
    let enum_name = enum_def.map(SealedEnumType::name).unwrap_or("enum");
    let member = variant_def.map(|v| v.name.as_ref()).unwrap_or("?");
    let mut out = format!("{enum_name}::{member}");
    if !payload.is_empty() {
        out.push('(');
        for (position, value) in payload.iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            out.push_str(&value_text(value, types, enums));
        }
        out.push(')');
    }
    out
}

/// The canonical text of any runtime value. Scalars, enums, and identities render to
/// their E03w forms; a record renders `{field: value, ...}` in declaration order, a
/// list `[a, b, ...]`, a map `[k: v, ...]` in ascending key order, and an optional its
/// inner value or `absent`. The renderer is total: interpolation and `string(...)` only
/// reach it with a scalar, enum, or identity (the checker refuses a bare aggregate hole),
/// but an enum payload or a returned value may be any shape, so every arm is real.
pub fn value_text(value: &Value, types: &[SealedRecordType], enums: &[SealedEnumType]) -> String {
    match value {
        Value::Int(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Text(v) => v.to_string(),
        Value::Bytes(v) => hex_bytes(v),
        Value::Date(v) => date_text(*v),
        Value::Instant(v) => instant_text(*v),
        Value::Duration(v) => marrow_temporal::format_duration(*v),
        Value::Enum(idx, variant, payload) => enum_text(types, enums, *idx, *variant, payload),
        Value::Id(_, keys) => id_text(keys),
        Value::Optional(None) => "absent".to_string(),
        Value::Optional(Some(inner)) => value_text(inner, types, enums),
        // `{field: value, ...}` in field declaration order, names from the record type.
        Value::Record(idx, slots) => {
            let fields = types.get(*idx as usize).map(SealedRecordType::fields);
            let mut out = String::from("{");
            for (position, slot) in slots.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                if let Some(field) = fields.and_then(|fields| fields.get(position)) {
                    out.push_str(&field.name);
                    out.push_str(": ");
                }
                match slot {
                    Some(inner) => out.push_str(&value_text(inner, types, enums)),
                    None => out.push_str("absent"),
                }
            }
            out.push('}');
            out
        }
        // `[a, b, ...]` in insertion order.
        Value::List(_, _, items) => {
            let mut out = String::from("[");
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                out.push_str(&value_text(item, types, enums));
            }
            out.push(']');
            out
        }
        // `[k: v, ...]` in ascending key order; the empty map is `[:]`.
        Value::Map(_, _, entries) => {
            let mut out = String::from("[");
            for (position, (key, value)) in entries.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                out.push_str(&key_text(key));
                out.push_str(": ");
                out.push_str(&value_text(value, types, enums));
            }
            if entries.is_empty() {
                out.push(':');
            }
            out.push(']');
            out
        }
    }
}
