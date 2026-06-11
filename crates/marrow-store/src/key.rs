//! Typed saved-key values used by the tree-cell store.

use std::cmp::Ordering;

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
    pub fn scalar_type(&self) -> crate::value::ScalarType {
        use crate::value::ScalarType;

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

    fn order_tag(&self) -> u8 {
        match self {
            SavedKey::Bool(_) => KEY_BOOL,
            SavedKey::Int(_) => KEY_INT,
            SavedKey::Date(_) => KEY_DATE,
            SavedKey::Instant(_) => KEY_INSTANT,
            SavedKey::Duration(_) => KEY_DURATION,
            SavedKey::Str(_) => KEY_STR,
            SavedKey::Bytes(_) => KEY_BYTES,
        }
    }
}

impl PartialOrd for SavedKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SavedKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.order_tag().cmp(&other.order_tag()) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
        match (self, other) {
            (SavedKey::Bool(left), SavedKey::Bool(right)) => left.cmp(right),
            (SavedKey::Int(left), SavedKey::Int(right)) => left.cmp(right),
            (SavedKey::Date(left), SavedKey::Date(right)) => left.cmp(right),
            (SavedKey::Instant(left), SavedKey::Instant(right)) => left.cmp(right),
            (SavedKey::Duration(left), SavedKey::Duration(right)) => left.cmp(right),
            (SavedKey::Str(left), SavedKey::Str(right)) => left.cmp(right),
            (SavedKey::Bytes(left), SavedKey::Bytes(right)) => left.cmp(right),
            _ => unreachable!(),
        }
    }
}

// Type tags ascend in Marrow's typed key order, so a byte comparison of two
// differently-typed keys yields their canonical order without decoding.
pub(crate) const KEY_BOOL: u8 = 0x01;
pub(crate) const KEY_INT: u8 = 0x02;
pub(crate) const KEY_DATE: u8 = 0x03;
pub(crate) const KEY_INSTANT: u8 = 0x04;
pub(crate) const KEY_DURATION: u8 = 0x05;
pub(crate) const KEY_STR: u8 = 0x07;
pub(crate) const KEY_BYTES: u8 = 0x08;

// The bounded int-key band uses this tag-only cursor as its exclusive upper bound.
pub(crate) const KEY_INT_EXCLUSIVE_END: u8 = KEY_INT + 1;
const _: () = assert!(KEY_DATE == KEY_INT_EXCLUSIVE_END);

pub(crate) fn encode_key_value(key: &SavedKey) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_key_into(key, &mut bytes);
    bytes
}

/// Decodes the leading scalar key, returning it with the byte count it consumed.
pub(crate) fn decode_key_value(bytes: &[u8]) -> Option<(SavedKey, usize)> {
    match *bytes.first()? {
        KEY_BOOL => {
            let value = match *bytes.get(1)? {
                0 => false,
                1 => true,
                _ => return None,
            };
            Some((SavedKey::Bool(value), 2))
        }
        KEY_INT => {
            let raw: [u8; 8] = bytes.get(1..9)?.try_into().ok()?;
            Some((
                SavedKey::Int((u64::from_be_bytes(raw) ^ (1u64 << 63)) as i64),
                9,
            ))
        }
        KEY_DATE => {
            let raw: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            Some((
                SavedKey::Date((u32::from_be_bytes(raw) ^ (1u32 << 31)) as i32),
                5,
            ))
        }
        KEY_DURATION => {
            let raw: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            Some((
                SavedKey::Duration((u128::from_be_bytes(raw) ^ (1u128 << 127)) as i128),
                17,
            ))
        }
        KEY_INSTANT => {
            let raw: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            Some((
                SavedKey::Instant((u128::from_be_bytes(raw) ^ (1u128 << 127)) as i128),
                17,
            ))
        }
        KEY_STR => {
            let (decoded, used) = decode_escaped_bytes(bytes.get(1..)?)?;
            Some((SavedKey::Str(String::from_utf8(decoded).ok()?), 1 + used))
        }
        KEY_BYTES => {
            let (decoded, used) = decode_escaped_bytes(bytes.get(1..)?)?;
            Some((SavedKey::Bytes(decoded), 1 + used))
        }
        _ => None,
    }
}

pub fn encode_identity_payload(identity: &[SavedKey]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for key in identity {
        bytes.extend_from_slice(&encode_key_value(key));
    }
    bytes
}

/// Decodes a canonical identity payload, returning `None` unless it holds exactly `arity` keys.
pub fn decode_identity_payload_arity(bytes: &[u8], arity: usize) -> Option<Vec<SavedKey>> {
    let mut keys = Vec::with_capacity(arity);
    let mut rest = bytes;
    for _ in 0..arity {
        let (key, used) = decode_key_value(rest)?;
        keys.push(key);
        rest = rest.get(used..)?;
    }
    rest.is_empty().then_some(keys)
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

// Inner `0x00` escapes to `0x00 0x01` and a `0x00 0x00` terminates the run, so an
// embedded null can never be confused with the end of the field.
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

/// Decodes one `encode_escaped_bytes` run, returning its bytes and the count consumed up to and including the terminator.
pub(crate) fn decode_escaped_bytes(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
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

#[cfg(test)]
mod tests {
    use super::{SavedKey, decode_key_value, encode_key_value};

    fn representative_keys() -> Vec<SavedKey> {
        vec![
            SavedKey::Bool(false),
            SavedKey::Bool(true),
            SavedKey::Int(i64::MIN),
            SavedKey::Int(-1),
            SavedKey::Int(0),
            SavedKey::Int(i64::MAX),
            SavedKey::Date(-719_162),
            SavedKey::Date(0),
            SavedKey::Date(2_932_896),
            SavedKey::Instant(i128::MIN),
            SavedKey::Instant(0),
            SavedKey::Instant(i128::MAX),
            SavedKey::Duration(i128::MIN),
            SavedKey::Duration(-1),
            SavedKey::Duration(i128::MAX),
            SavedKey::Str(String::new()),
            SavedKey::Str("a\u{0}b".into()),
            SavedKey::Bytes(vec![]),
            SavedKey::Bytes(vec![0x00, 0x01, 0xff]),
        ]
    }

    #[test]
    fn saved_key_codec_round_trips_representative_values() {
        for key in representative_keys() {
            let bytes = encode_key_value(&key);
            let (decoded, used) = decode_key_value(&bytes).expect("key decodes");
            assert_eq!(decoded, key);
            assert_eq!(used, bytes.len(), "decoder consumes the exact key frame");
        }
    }

    #[test]
    fn saved_key_codec_preserves_typed_order_in_bytes() {
        let mut by_type = representative_keys();
        by_type.sort();

        let mut by_bytes = representative_keys();
        by_bytes.sort_by_key(encode_key_value);

        assert_eq!(
            by_bytes, by_type,
            "encoded key bytes must sort like SavedKey"
        );
    }

    #[test]
    fn saved_key_codec_preserves_typed_reverse_order_in_bytes() {
        let mut by_type = representative_keys();
        by_type.sort_by(|left, right| right.cmp(left));

        let mut by_bytes = representative_keys();
        by_bytes.sort_by_key(|key| std::cmp::Reverse(encode_key_value(key)));

        assert_eq!(
            by_bytes, by_type,
            "reverse encoded key bytes must sort like reverse SavedKey order"
        );
    }

    #[test]
    fn escaped_key_byte_fingerprints_are_stable() {
        assert_eq!(
            encode_key_value(&SavedKey::Str("a\u{0}b".into())),
            vec![0x07, b'a', 0x00, 0x01, b'b', 0x00, 0x00]
        );
        assert_eq!(
            encode_key_value(&SavedKey::Bytes(vec![0x00, 0x01, 0xff])),
            vec![0x08, 0x00, 0x01, 0x01, 0xff, 0x00, 0x00]
        );
    }
}
