//! Typed saved-key values and their order-preserving byte codec.

use crate::value::ScalarType;

/// A scalar key value in a record-key or index-key position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedKey {
    Int(i64),
    Bool(bool),
    Str(String),
    /// A calendar date as days since the Unix epoch (1970-01-01).
    Date(i32),
    /// An elapsed span as a signed count of nanoseconds.
    Duration(i128),
    /// A UTC instant as a signed count of nanoseconds since the epoch.
    Instant(i128),
    /// Arbitrary bytes, ordered by byte value.
    Bytes(Vec<u8>),
}

impl SavedKey {
    /// The stable wire tag for this key in the serve JSON codec.
    pub fn wire_tag(&self) -> &'static str {
        match self {
            SavedKey::Str(_) => "str",
            SavedKey::Bool(_) => ScalarType::Bool.name(),
            SavedKey::Int(_) => ScalarType::Int.name(),
            SavedKey::Bytes(_) => ScalarType::Bytes.name(),
            SavedKey::Date(_) => ScalarType::Date.name(),
            SavedKey::Duration(_) => ScalarType::Duration.name(),
            SavedKey::Instant(_) => ScalarType::Instant.name(),
        }
    }

    /// The scalar kind this key projects.
    pub fn scalar_type(&self) -> ScalarType {
        match self {
            SavedKey::Bool(_) => ScalarType::Bool,
            SavedKey::Int(_) => ScalarType::Int,
            SavedKey::Str(_) => ScalarType::Str,
            SavedKey::Bytes(_) => ScalarType::Bytes,
            SavedKey::Date(_) => ScalarType::Date,
            SavedKey::Duration(_) => ScalarType::Duration,
            SavedKey::Instant(_) => ScalarType::Instant,
        }
    }
}

// Key-type tags, in Marrow's typed key order: booleans, numbers, then dates,
// instants, and durations, then strings.
pub(crate) const KEY_BOOL: u8 = 0x01;
pub(crate) const KEY_INT: u8 = 0x02;
pub(crate) const KEY_DATE: u8 = 0x03;
pub(crate) const KEY_INSTANT: u8 = 0x04;
pub(crate) const KEY_DURATION: u8 = 0x05;
pub(crate) const KEY_STR: u8 = 0x07;
pub(crate) const KEY_BYTES: u8 = 0x08;

// The bounded int-key band uses `KEY_INT + 1` as its exclusive upper bound.
const _: () = assert!(KEY_DATE == KEY_INT + 1);

/// Encode a single scalar key to its order-preserving bytes.
pub fn encode_key_value(key: &SavedKey) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_key_into(key, &mut bytes);
    bytes
}

/// Decode one scalar key from the front of `bytes`.
pub fn decode_key_value(bytes: &[u8]) -> Option<(SavedKey, usize)> {
    Some((decode_key(bytes)?, key_len(bytes)?))
}

pub(crate) fn encode_key_into(key: &SavedKey, out: &mut Vec<u8>) {
    match key {
        SavedKey::Bool(value) => {
            out.push(KEY_BOOL);
            out.push(u8::from(*value));
        }
        SavedKey::Int(value) => {
            out.push(KEY_INT);
            out.extend_from_slice(&((*value as u64) ^ (1u64 << 63)).to_be_bytes());
        }
        SavedKey::Date(value) => {
            out.push(KEY_DATE);
            out.extend_from_slice(&((*value as u32) ^ (1u32 << 31)).to_be_bytes());
        }
        SavedKey::Duration(value) => {
            out.push(KEY_DURATION);
            out.extend_from_slice(&((*value as u128) ^ (1u128 << 127)).to_be_bytes());
        }
        SavedKey::Instant(value) => {
            out.push(KEY_INSTANT);
            out.extend_from_slice(&((*value as u128) ^ (1u128 << 127)).to_be_bytes());
        }
        SavedKey::Str(value) => {
            out.push(KEY_STR);
            encode_escaped_bytes(value.as_bytes(), out);
        }
        SavedKey::Bytes(value) => {
            out.push(KEY_BYTES);
            encode_escaped_bytes(value, out);
        }
    }
}

pub(crate) fn decode_key(bytes: &[u8]) -> Option<SavedKey> {
    match *bytes.first()? {
        KEY_BOOL => match *bytes.get(1)? {
            0 => Some(SavedKey::Bool(false)),
            1 => Some(SavedKey::Bool(true)),
            _ => None,
        },
        KEY_INT => {
            let raw: [u8; 8] = bytes.get(1..9)?.try_into().ok()?;
            Some(SavedKey::Int(
                (u64::from_be_bytes(raw) ^ (1u64 << 63)) as i64,
            ))
        }
        KEY_DATE => {
            let raw: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            Some(SavedKey::Date(
                (u32::from_be_bytes(raw) ^ (1u32 << 31)) as i32,
            ))
        }
        KEY_DURATION => {
            let raw: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            Some(SavedKey::Duration(
                (u128::from_be_bytes(raw) ^ (1u128 << 127)) as i128,
            ))
        }
        KEY_INSTANT => {
            let raw: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            Some(SavedKey::Instant(
                (u128::from_be_bytes(raw) ^ (1u128 << 127)) as i128,
            ))
        }
        KEY_STR => {
            let (decoded, _) = read_escaped_str(bytes.get(1..)?)?;
            Some(SavedKey::Str(String::from_utf8(decoded).ok()?))
        }
        KEY_BYTES => {
            let (decoded, _) = read_escaped_str(bytes.get(1..)?)?;
            Some(SavedKey::Bytes(decoded))
        }
        _ => None,
    }
}

pub(crate) fn key_len(bytes: &[u8]) -> Option<usize> {
    let fixed = |len: usize| (bytes.len() >= len).then_some(len);
    match *bytes.first()? {
        KEY_BOOL => fixed(2),
        KEY_INT => fixed(9),
        KEY_DATE => fixed(5),
        KEY_DURATION | KEY_INSTANT => fixed(17),
        KEY_STR | KEY_BYTES => Some(1 + read_escaped_str(bytes.get(1..)?)?.1),
        _ => None,
    }
}

pub(crate) fn encode_escaped_bytes(value: &[u8], out: &mut Vec<u8>) {
    for &byte in value {
        out.push(byte);
        if byte == 0x00 {
            out.push(0x01);
        }
    }
    out.push(0x00);
    out.push(0x00);
}

fn read_escaped_str(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut decoded = Vec::new();
    let mut index = 0;
    loop {
        match *bytes.get(index)? {
            0x00 => match *bytes.get(index + 1)? {
                0x00 => return Some((decoded, index + 2)),
                0x01 => {
                    decoded.push(0x00);
                    index += 2;
                }
                _ => return None,
            },
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
}
