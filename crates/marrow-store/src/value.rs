//! Canonical saved-value encoding.
//!
//! Saved values are stored in their canonical Marrow byte form
//! (docs/language/types.md): the bytes do not depend on the backend, so backup,
//! diff, traversal, equality, and restore are stable. Unlike keys, values are
//! not order-preserving — the store orders by path, not by value — so the
//! encoding optimizes for a clear canonical round-trip. A value's type comes
//! from the schema at read time, so the bytes carry no type tag.

/// A scalar saved value in decoded form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedValue {
    Bool(bool),
    Int(i64),
    Str(String),
    Bytes(Vec<u8>),
    ErrorCode(String),
}

/// The type to decode saved bytes as. A typed read knows this from the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    Int,
    Str,
    Bytes,
    ErrorCode,
}

/// Encode a value to its canonical saved bytes: `bool` as `0`/`1`, `int` as
/// decimal text, strings and error codes as UTF-8, bytes verbatim
/// (docs/language/types.md).
pub fn encode_value(value: &SavedValue) -> Vec<u8> {
    match value {
        SavedValue::Bool(value) => vec![if *value { b'1' } else { b'0' }],
        SavedValue::Int(value) => value.to_string().into_bytes(),
        SavedValue::Str(text) | SavedValue::ErrorCode(text) => text.as_bytes().to_vec(),
        SavedValue::Bytes(bytes) => bytes.clone(),
    }
}

/// Decode canonical saved bytes as `ty`, or `None` if the bytes are not a valid
/// canonical form for that type. The check is strict, so non-canonical bytes
/// (e.g. `+1`, `01`, or a non-`0`/`1` boolean) are rejected rather than
/// silently normalized.
pub fn decode_value(bytes: &[u8], ty: ValueType) -> Option<SavedValue> {
    match ty {
        ValueType::Bool => match bytes {
            b"0" => Some(SavedValue::Bool(false)),
            b"1" => Some(SavedValue::Bool(true)),
            _ => None,
        },
        ValueType::Int => Some(SavedValue::Int(parse_canonical_int(bytes)?)),
        ValueType::Str => Some(SavedValue::Str(String::from_utf8(bytes.to_vec()).ok()?)),
        ValueType::Bytes => Some(SavedValue::Bytes(bytes.to_vec())),
        ValueType::ErrorCode => Some(SavedValue::ErrorCode(
            String::from_utf8(bytes.to_vec()).ok()?,
        )),
    }
}

/// Parse the exact canonical decimal form `encode_value` produces: an optional
/// `-` then digits, no `+`, no leading zeros. Rejects anything that would not
/// round-trip identically (`+1`, `01`, `-0`, whitespace).
fn parse_canonical_int(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: i64 = text.parse().ok()?;
    (value.to_string() == text).then_some(value)
}
