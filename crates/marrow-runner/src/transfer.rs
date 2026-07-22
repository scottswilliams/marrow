//! The transfer codec: wire JSON values ↔ runtime [`Value`]s, driven by the
//! verified image's own types.
//!
//! Decoding maps a request's JSON argument onto an export parameter's
//! [`ImageType`], resolving record fields, enum variants, collection element/key/
//! value types, and root key columns against the image's sealed tables (which carry
//! the names and image-local type indices a bare wire value does not). Encoding is
//! the inverse over a returned [`Value`]. Both cover the whole transfer graph — the
//! seven scalars, an optional wrapper, a product (record), a sum (enum, including
//! `Option`/`Result`), a finite `List<T>`, an ordered `Map<K, V>` (an array of
//! `[key, value]` pair-arrays, never a JS object), and an entry identity `Id(^root)`
//! (the array of its key-column scalars). The graph is closed over every
//! `ImageType`, so a served signature always has a codec.

use marrow_image::{ImageType, Scalar};
use marrow_local_wire::Json;
use marrow_verify::{SealedCollectionType, VerifiedImage};
use marrow_vm::{KeyScalar, Value};
use std::collections::HashSet;
use std::rc::Rc;

/// Decode a JSON argument into a runtime value against `ty`, or `None` when the
/// value does not match the declared parameter type. `None` is the runner's
/// `runner.arg_mismatch` signal.
pub(crate) fn decode_arg(image: &VerifiedImage, ty: &ImageType, json: &Json) -> Option<Value> {
    match ty {
        ImageType::Unit => None, // unit is not an argument value
        ImageType::Scalar { scalar, optional } => {
            wrap_optional(*optional, json, |j| decode_scalar(*scalar, j))
        }
        ImageType::Record { idx, optional } => {
            wrap_optional(*optional, json, |j| decode_record(image, *idx, j))
        }
        ImageType::Enum { idx, optional } => {
            wrap_optional(*optional, json, |j| decode_enum(image, *idx, j))
        }
        ImageType::Collection { idx, optional } => {
            wrap_optional(*optional, json, |j| decode_collection(image, *idx, j))
        }
        ImageType::Identity { root, optional } => {
            wrap_optional(*optional, json, |j| decode_identity(image, *root, j))
        }
    }
}

/// Apply an optional wrapper: `null` decodes to a vacant optional, any other value
/// to the bare decode wrapped in `Optional(Some(_))`; a non-optional type decodes
/// bare.
fn wrap_optional(
    optional: bool,
    json: &Json,
    decode_bare: impl FnOnce(&Json) -> Option<Value>,
) -> Option<Value> {
    match (optional, json) {
        (true, Json::Null) => Some(Value::Optional(None)),
        (true, other) => Some(Value::Optional(Some(Box::new(decode_bare(other)?)))),
        (false, other) => decode_bare(other),
    }
}

fn decode_scalar(scalar: Scalar, json: &Json) -> Option<Value> {
    match (scalar, json) {
        (Scalar::Int, Json::Int(n)) => Some(Value::Int(*n)),
        (Scalar::Bool, Json::Bool(b)) => Some(Value::Bool(*b)),
        (Scalar::Text, Json::Str(s)) => Some(Value::Text(Rc::from(s.as_str()))),
        (Scalar::Bytes, Json::Str(s)) => {
            decode_hex_bytes(s).map(|bytes| Value::Bytes(Rc::from(bytes.as_slice())))
        }
        (Scalar::Date, Json::Str(s)) => marrow_temporal::parse_date(s.as_bytes()).map(Value::Date),
        (Scalar::Instant, Json::Str(s)) => {
            marrow_temporal::parse_instant(s.as_bytes()).map(Value::Instant)
        }
        (Scalar::Duration, Json::Str(s)) => {
            marrow_temporal::parse_duration(s.as_bytes()).map(Value::Duration)
        }
        _ => None,
    }
}

fn decode_record(image: &VerifiedImage, idx: u16, json: &Json) -> Option<Value> {
    let Json::Object(pairs) = json else {
        return None;
    };
    let record = image.record_type(idx);
    let mut slots: Vec<Option<Value>> = Vec::with_capacity(record.fields().len());
    for field in record.fields() {
        match pairs
            .iter()
            .find(|(key, _)| key.as_str() == field.name.as_ref())
        {
            Some((_, value)) => slots.push(Some(decode_arg(image, &field.ty, value)?)),
            None if !field.required => slots.push(None),
            None => return None, // a required field is missing
        }
    }
    // Every object key must belong to the record: an extra key is a mismatch.
    if pairs.len() != slots.iter().filter(|slot| slot.is_some()).count() {
        return None;
    }
    Some(Value::Record(idx, slots.into_boxed_slice()))
}

fn decode_enum(image: &VerifiedImage, idx: u16, json: &Json) -> Option<Value> {
    let Json::Object(pairs) = json else {
        return None;
    };
    if pairs.len() != 2 {
        return None;
    }
    let member = match pairs.iter().find(|(k, _)| k == "member")?.1 {
        Json::Str(ref s) => s.as_str(),
        _ => return None,
    };
    let payload_json = match &pairs.iter().find(|(k, _)| k == "payload")?.1 {
        Json::Array(items) => items,
        _ => return None,
    };
    let enum_type = &image.enums()[idx as usize];
    let (variant_index, variant) = enum_type
        .variants()
        .iter()
        .enumerate()
        .find(|(_, v)| v.name.as_ref() == member)?;
    if variant.payload.len() != payload_json.len() {
        return None;
    }
    let mut values = Vec::with_capacity(payload_json.len());
    for (leaf_ty, leaf_json) in variant.payload.iter().zip(payload_json) {
        values.push(decode_arg(image, leaf_ty, leaf_json)?);
    }
    Some(Value::Enum(
        idx,
        variant_index as u16,
        values.into_boxed_slice(),
    ))
}

/// Decode a finite collection argument against the image's COLLTYPES entry: a
/// `List<T>` from a JSON array of element values, or an ordered `Map<K, V>` from a
/// JSON array of `[key, value]` pair-arrays. A map with a duplicate key, a
/// mis-shaped pair, or a key/value that does not match the declared type is a
/// mismatch.
fn decode_collection(image: &VerifiedImage, idx: u16, json: &Json) -> Option<Value> {
    let Json::Array(items) = json else {
        return None;
    };
    match image.collection_type(idx) {
        SealedCollectionType::List { elem } => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(decode_arg(image, &elem, item)?);
            }
            Some(Value::list(idx, Rc::new(values)))
        }
        SealedCollectionType::Map { key, value } => {
            let key_scalar = scalar_of(key)?;
            let mut entries = Vec::with_capacity(items.len());
            let mut seen: HashSet<String> = HashSet::with_capacity(items.len());
            for item in items {
                let Json::Array(pair) = item else {
                    return None;
                };
                let [key_json, value_json] = pair.as_slice() else {
                    return None;
                };
                let key_value = decode_key(key_scalar, key_json)?;
                // A map has unique keys; the canonical key spelling detects a
                // duplicate in bounded work.
                if !seen.insert(marrow_local_wire::encode(&encode_key(&key_value))) {
                    return None;
                }
                entries.push((key_value, decode_arg(image, &value, value_json)?));
            }
            Some(Value::map(idx, Rc::new(entries)))
        }
    }
}

/// Decode an entry identity `Id(^root)` argument: a JSON array of the root's
/// key-column scalars, one per declared column and in declaration order.
///
/// An `Id(^root)` parameter is currently trough-absent (the checker rejects an
/// identity in any parameter position), so no verified image reaches this from an
/// argument; identity crosses only as a return today. This half keeps the codec
/// total and symmetric with [`encode_value`]'s identity arm and rejects a hostile
/// identity-shaped argument rather than trusting it.
fn decode_identity(image: &VerifiedImage, root: u16, json: &Json) -> Option<Value> {
    let Json::Array(items) = json else {
        return None;
    };
    let columns = image.roots()[root as usize].keys();
    if items.len() != columns.len() {
        return None;
    }
    let mut keys = Vec::with_capacity(columns.len());
    for (column, item) in columns.iter().zip(items) {
        keys.push(decode_key(*column, item)?);
    }
    Some(Value::Id(root, Rc::from(keys.as_slice())))
}

/// The bare scalar of a key type. A map key or identity key column is always a bare
/// scalar (the verifier proved it); anything else is a mismatch.
fn scalar_of(ty: ImageType) -> Option<Scalar> {
    match ty {
        ImageType::Scalar {
            scalar,
            optional: false,
        } => Some(scalar),
        _ => None,
    }
}

/// Decode a JSON value into a [`KeyScalar`] against a declared key scalar type,
/// mirroring [`decode_scalar`]'s spellings (temporal canonical text, `0x`-hex bytes).
fn decode_key(scalar: Scalar, json: &Json) -> Option<KeyScalar> {
    Some(match (scalar, json) {
        (Scalar::Int, Json::Int(n)) => KeyScalar::Int(*n),
        (Scalar::Bool, Json::Bool(b)) => KeyScalar::Bool(*b),
        (Scalar::Text, Json::Str(s)) => KeyScalar::Str(s.clone()),
        (Scalar::Bytes, Json::Str(s)) => KeyScalar::Bytes(decode_hex_bytes(s)?),
        (Scalar::Date, Json::Str(s)) => KeyScalar::Date(marrow_temporal::parse_date(s.as_bytes())?),
        (Scalar::Instant, Json::Str(s)) => {
            KeyScalar::Instant(marrow_temporal::parse_instant(s.as_bytes())?)
        }
        (Scalar::Duration, Json::Str(s)) => {
            KeyScalar::Duration(marrow_temporal::parse_duration(s.as_bytes())?)
        }
        _ => return None,
    })
}

/// Encode a returned value into its wire JSON, or `None` for a value outside the
/// transfer graph (a collection — unreachable for a served export).
pub(crate) fn encode_value(image: &VerifiedImage, value: &Value) -> Option<Json> {
    Some(match value {
        Value::Int(n) => Json::Int(*n),
        Value::Bool(b) => Json::Bool(*b),
        Value::Text(text) => Json::Str(text.to_string()),
        Value::Bytes(bytes) => Json::Str(hex_bytes(bytes)),
        Value::Date(days) => Json::Str(date_text(*days)),
        Value::Instant(nanos) => Json::Str(instant_text(*nanos)),
        Value::Duration(nanos) => Json::Str(marrow_temporal::format_duration(*nanos)),
        Value::Optional(None) => Json::Null,
        Value::Optional(Some(inner)) => encode_value(image, inner)?,
        Value::Record(idx, slots) => {
            let record = image.record_type(*idx);
            let mut pairs = Vec::new();
            for (position, slot) in slots.iter().enumerate() {
                // A vacant sparse slot is omitted; a present slot carries its value.
                if let Some(inner) = slot {
                    let name = record.fields()[position].name.to_string();
                    pairs.push((name, encode_value(image, inner)?));
                }
            }
            Json::Object(pairs)
        }
        Value::Enum(idx, variant, payload) => {
            let variant_name = image.enums()[*idx as usize].variants()[*variant as usize]
                .name
                .to_string();
            let mut items = Vec::with_capacity(payload.len());
            for leaf in payload.iter() {
                items.push(encode_value(image, leaf)?);
            }
            Json::Object(vec![
                ("member".to_string(), Json::Str(variant_name)),
                ("payload".to_string(), Json::Array(items)),
            ])
        }
        Value::List(_, _, items) => {
            let mut encoded = Vec::with_capacity(items.len());
            for item in items.iter() {
                encoded.push(encode_value(image, item)?);
            }
            Json::Array(encoded)
        }
        // An ordered map crosses as an array of `[key, value]` pair-arrays, never a
        // JS object, so a non-string key and entry order both survive.
        Value::Map(_, _, entries) => {
            let mut encoded = Vec::with_capacity(entries.len());
            for (key, value) in entries.iter() {
                encoded.push(Json::Array(vec![
                    encode_key(key),
                    encode_value(image, value)?,
                ]));
            }
            Json::Array(encoded)
        }
        // An entry identity crosses as the array of its key-column scalars.
        Value::Id(_, keys) => Json::Array(keys.iter().map(encode_key).collect()),
    })
}

/// Encode a [`KeyScalar`] into its wire JSON, mirroring [`encode_value`]'s scalar
/// spellings (temporal canonical text, `0x`-hex bytes).
fn encode_key(key: &KeyScalar) -> Json {
    match key {
        KeyScalar::Int(n) => Json::Int(*n),
        KeyScalar::Bool(b) => Json::Bool(*b),
        KeyScalar::Str(s) => Json::Str(s.clone()),
        KeyScalar::Bytes(bytes) => Json::Str(hex_bytes(bytes)),
        KeyScalar::Date(days) => Json::Str(date_text(*days)),
        KeyScalar::Instant(nanos) => Json::Str(instant_text(*nanos)),
        KeyScalar::Duration(nanos) => Json::Str(marrow_temporal::format_duration(*nanos)),
    }
}

/// Decode a `0x`-prefixed even-length lowercase-hex string to bytes, matching the
/// canonical `bytes` rendering.
fn decode_hex_bytes(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("0x")?;
    if !hex.len().is_multiple_of(2)
        || hex
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Render bytes as `0x`-prefixed lowercase hex.
fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn date_text(days: i32) -> String {
    marrow_temporal::format_date(days).unwrap_or_else(|| days.to_string())
}

fn instant_text(nanos: i128) -> String {
    marrow_temporal::format_instant(nanos).unwrap_or_else(|| nanos.to_string())
}
