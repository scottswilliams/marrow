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
use marrow_local_wire::{Json, ValueWriter, WireError};
use marrow_verify::{SealedCollectionType, VerifiedImage};
use marrow_vm::{KeyScalar, Value, collection_within_limits, key_bytes};
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
        ImageType::Record { idx, optional } => wrap_optional(*optional, json, |j| {
            decode_record(image, sealed_ordinal(idx.index()), j)
        }),
        ImageType::Enum { idx, optional } => wrap_optional(*optional, json, |j| {
            decode_enum(image, sealed_ordinal(idx.index()), j)
        }),
        ImageType::Collection { idx, optional } => wrap_optional(*optional, json, |j| {
            decode_collection(image, sealed_ordinal(idx.index()), j)
        }),
        ImageType::Identity { root, optional } => wrap_optional(*optional, json, |j| {
            decode_identity(image, sealed_ordinal(root.index()), j)
        }),
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

/// The sealed wire-domain `u16` of a verified typed table reference: every value here
/// was decoded from a `u16` wire read, so the narrowing is total; it is spelled checked
/// so the wire domain is stated.
fn sealed_ordinal(index: u32) -> u16 {
    u16::try_from(index).expect("a verified table reference was decoded from a u16 wire read")
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
/// JSON array of `[key, value]` pair-arrays, normalized to ascending typed key
/// order. Duplicate keys, mis-shaped pairs, or key/value type mismatches reject
/// the argument. Each collection must fit the VM value owner's fixed count and
/// aggregate structural-byte limits before it can enter execution.
fn decode_collection(image: &VerifiedImage, idx: u16, json: &Json) -> Option<Value> {
    let Json::Array(items) = json else {
        return None;
    };
    if !collection_within_limits(items.len(), 0) {
        return None;
    }
    let mut bytes = 0usize;
    match image.collection_type(idx) {
        SealedCollectionType::List { elem } => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                let value = decode_arg(image, &elem, item)?;
                bytes = bytes.checked_add(value.structural_bytes())?;
                if !collection_within_limits(items.len(), bytes) {
                    return None;
                }
                values.push(value);
            }
            Some(Value::List(idx, bytes, Rc::new(values)))
        }
        SealedCollectionType::Map { key, value } => {
            let key_scalar = scalar_of(key)?;
            let mut entries = Vec::with_capacity(items.len());
            for item in items {
                let Json::Array(pair) = item else {
                    return None;
                };
                let [key_json, value_json] = pair.as_slice() else {
                    return None;
                };
                let key_value = decode_key(key_scalar, key_json)?;
                let entry_value = decode_arg(image, &value, value_json)?;
                bytes = bytes
                    .checked_add(key_bytes(&key_value))?
                    .checked_add(entry_value.structural_bytes())?;
                if !collection_within_limits(items.len(), bytes) {
                    return None;
                }
                entries.push((key_value, entry_value));
            }
            // VM lookup and mutation require unique entries in typed key order.
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return None;
            }
            Some(Value::Map(idx, bytes, Rc::new(entries)))
        }
    }
}

/// Decode an entry identity `Id(^root)` argument: a JSON array of the root's
/// key-column scalars, one per declared column and in declaration order. A wrong
/// arity, or a key that does not match its declared column scalar, is a mismatch.
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

/// Stream a returned value while its verified image and payload remain borrowed.
pub(crate) fn encode_value(
    image: &VerifiedImage,
    value: &Value,
    slot: ValueWriter<'_>,
) -> Result<(), WireError> {
    match value {
        Value::Int(n) => slot.integer(*n),
        Value::Bool(b) => slot.boolean(*b),
        Value::Text(text) => slot.string(text),
        Value::Bytes(bytes) => slot.hex_bytes(bytes),
        Value::Date(days) => slot.string(&date_text(*days)),
        Value::Instant(nanos) => slot.string(&instant_text(*nanos)),
        Value::Duration(nanos) => slot.string(&marrow_temporal::format_duration(*nanos)),
        Value::Optional(None) => slot.null(),
        Value::Optional(Some(inner)) => encode_value(image, inner, slot),
        Value::Record(idx, slots) => {
            let record = image.record_type(*idx);
            let mut fields: Vec<(&str, &Value)> = record
                .fields()
                .iter()
                .zip(slots.iter())
                .filter_map(|(field, value)| {
                    value.as_ref().map(|value| (field.name.as_ref(), value))
                })
                .collect();
            fields.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            slot.object(|object| {
                for (name, value) in fields {
                    object.field(name, |slot| encode_value(image, value, slot))?;
                }
                Ok(())
            })
        }
        Value::Enum(idx, variant, payload) => {
            let variant_name = &image.enums()[*idx as usize].variants()[*variant as usize].name;
            slot.object(|object| {
                object.field("member", |slot| slot.string(variant_name))?;
                object.field("payload", |slot| {
                    slot.array(|array| {
                        for value in payload.iter() {
                            array.element(|slot| encode_value(image, value, slot))?;
                        }
                        Ok(())
                    })
                })
            })
        }
        Value::List(_, _, items) => slot.array(|array| {
            for value in items.iter() {
                array.element(|slot| encode_value(image, value, slot))?;
            }
            Ok(())
        }),
        // Map order and non-string keys survive as pair-arrays.
        Value::Map(_, _, entries) => slot.array(|array| {
            for (key, value) in entries.iter() {
                array.element(|slot| {
                    slot.array(|pair| {
                        pair.element(|slot| encode_key(key, slot))?;
                        pair.element(|slot| encode_value(image, value, slot))
                    })
                })?;
            }
            Ok(())
        }),
        Value::Id(_, keys) => slot.array(|array| {
            for key in keys.iter() {
                array.element(|slot| encode_key(key, slot))?;
            }
            Ok(())
        }),
    }
}

/// Entry and map keys use the same scalar spelling as returned values.
fn encode_key(key: &KeyScalar, slot: ValueWriter<'_>) -> Result<(), WireError> {
    match key {
        KeyScalar::Int(n) => slot.integer(*n),
        KeyScalar::Bool(b) => slot.boolean(*b),
        KeyScalar::Str(s) => slot.string(s),
        KeyScalar::Bytes(bytes) => slot.hex_bytes(bytes),
        KeyScalar::Date(days) => slot.string(&date_text(*days)),
        KeyScalar::Instant(nanos) => slot.string(&instant_text(*nanos)),
        KeyScalar::Duration(nanos) => slot.string(&marrow_temporal::format_duration(*nanos)),
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

fn date_text(days: i32) -> String {
    marrow_temporal::format_date(days).unwrap_or_else(|| days.to_string())
}

fn instant_text(nanos: i128) -> String {
    marrow_temporal::format_instant(nanos).unwrap_or_else(|| nanos.to_string())
}
