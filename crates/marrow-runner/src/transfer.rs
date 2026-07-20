//! The transfer codec: wire JSON values ↔ runtime [`Value`]s, driven by the
//! verified image's own types.
//!
//! Decoding maps a request's JSON argument onto an export parameter's
//! [`ImageType`], resolving record fields and enum variants against the image's
//! sealed tables (which carry the field names, variant names, and image-local type
//! indices a bare wire value does not). Encoding is the inverse over a returned
//! [`Value`]. Both cover exactly the G00a transfer graph — the seven scalars, an
//! optional wrapper, a product (record), and a sum (enum, including `Option`/
//! `Result`). A collection never reaches this codec: an export whose signature
//! touches one is not admitted to the served interface, so the runner never
//! launches with it.

use marrow_image::{ImageType, Scalar};
use marrow_local_wire::Json;
use marrow_verify::VerifiedImage;
use marrow_vm::Value;
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
        // Excluded from the served interface; unreachable for a launched runner. An
        // entry identity is excluded for the same reason.
        ImageType::Collection { .. } | ImageType::Identity { .. } => None,
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
        // Outside the G00a transfer graph; a served export never returns one. An
        // entry identity is excluded from the wire interface for the same reason.
        Value::List(..) | Value::Map(..) | Value::Id(..) => return None,
    })
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
