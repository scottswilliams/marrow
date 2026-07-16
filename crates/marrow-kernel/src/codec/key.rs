//! Typed key values and their order-preserving byte encoding.
//!
//! A [`KeyScalar`] is a typed key value; its encoding sorts byte-wise into the
//! same order as the typed value, so an ordered-byte engine ranges over keys in
//! Marrow's key order without decoding them. Type tags ascend in that order, and
//! variable-length keys use a `0x00`-escape framing so an embedded null never
//! ends a field early.

use std::cmp::Ordering;

use super::value::ScalarKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyScalar {
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

impl KeyScalar {
    pub fn scalar_kind(&self) -> ScalarKind {
        match self {
            KeyScalar::Bool(_) => ScalarKind::Bool,
            KeyScalar::Int(_) => ScalarKind::Int,
            KeyScalar::Str(_) => ScalarKind::Str,
            KeyScalar::Bytes(_) => ScalarKind::Bytes,
            KeyScalar::Date(_) => ScalarKind::Date,
            KeyScalar::Duration(_) => ScalarKind::Duration,
            KeyScalar::Instant(_) => ScalarKind::Instant,
        }
    }

    fn kind(&self) -> KeyKind {
        match self {
            KeyScalar::Bool(_) => KeyKind::Bool,
            KeyScalar::Int(_) => KeyKind::Int,
            KeyScalar::Date(_) => KeyKind::Date,
            KeyScalar::Instant(_) => KeyKind::Instant,
            KeyScalar::Duration(_) => KeyKind::Duration,
            KeyScalar::Str(_) => KeyKind::Str,
            KeyScalar::Bytes(_) => KeyKind::Bytes,
        }
    }

    fn order_tag(&self) -> u8 {
        self.kind().tag()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KeyKind {
    Bool,
    Int,
    Date,
    Instant,
    Duration,
    Str,
    Bytes,
}

impl KeyKind {
    fn tag(self) -> u8 {
        match self {
            Self::Bool => KEY_BOOL,
            Self::Int => KEY_INT,
            Self::Date => KEY_DATE,
            Self::Instant => KEY_INSTANT,
            Self::Duration => KEY_DURATION,
            Self::Str => KEY_STR,
            Self::Bytes => KEY_BYTES,
        }
    }
}

impl PartialOrd for KeyScalar {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KeyScalar {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (KeyScalar::Bool(left), KeyScalar::Bool(right)) => left.cmp(right),
            (KeyScalar::Int(left), KeyScalar::Int(right)) => left.cmp(right),
            (KeyScalar::Date(left), KeyScalar::Date(right)) => left.cmp(right),
            (KeyScalar::Instant(left), KeyScalar::Instant(right)) => left.cmp(right),
            (KeyScalar::Duration(left), KeyScalar::Duration(right)) => left.cmp(right),
            (KeyScalar::Str(left), KeyScalar::Str(right)) => left.cmp(right),
            (KeyScalar::Bytes(left), KeyScalar::Bytes(right)) => left.cmp(right),
            _ => self.kind().cmp(&other.kind()),
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
// 0x06 is intentionally left unassigned: it was reserved for a decimal value
// type that was removed at B00 and may return later, and holding the slot keeps
// the relative tag order of the other kinds stable. Pre-beta there is no stored
// data and no compatibility promise; the durable-key contract that freezes
// these tags is established by the lane that first writes data under them.
pub(crate) const KEY_STR: u8 = 0x07;
pub(crate) const KEY_BYTES: u8 = 0x08;

pub fn encode_key_value(key: &KeyScalar) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_key_into(key, &mut bytes);
    bytes
}

/// Encode a composite key tuple as the ordered concatenation of its columns'
/// encodings, in column order. Each column's encoding is prefix-free — a fixed-width
/// kind, or a `0x00,0x00`-terminated escaped run — so the columns self-delimit and an
/// embedded `0x00` in one column can never be read as part of the next. The
/// concatenation is therefore prefix-free as a whole and sorts byte-wise into
/// column-major tuple order, exactly as a single key encoding sorts into key order.
pub fn encode_key_tuple(keys: &[KeyScalar]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for key in keys {
        encode_key_into(key, &mut bytes);
    }
    bytes
}

/// Decodes the leading scalar key, returning it with the byte count it consumed.
pub fn decode_key_value(bytes: &[u8]) -> Option<(KeyScalar, usize)> {
    match *bytes.first()? {
        KEY_BOOL => {
            let value = match *bytes.get(1)? {
                0 => false,
                1 => true,
                _ => return None,
            };
            Some((KeyScalar::Bool(value), 2))
        }
        KEY_INT => {
            let raw: [u8; 8] = bytes.get(1..9)?.try_into().ok()?;
            Some((
                KeyScalar::Int((u64::from_be_bytes(raw) ^ (1u64 << 63)) as i64),
                9,
            ))
        }
        KEY_DATE => {
            let raw: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            Some((
                KeyScalar::Date((u32::from_be_bytes(raw) ^ (1u32 << 31)) as i32),
                5,
            ))
        }
        KEY_DURATION => {
            let raw: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            Some((
                KeyScalar::Duration((u128::from_be_bytes(raw) ^ (1u128 << 127)) as i128),
                17,
            ))
        }
        KEY_INSTANT => {
            let raw: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            Some((
                KeyScalar::Instant((u128::from_be_bytes(raw) ^ (1u128 << 127)) as i128),
                17,
            ))
        }
        KEY_STR => {
            let (decoded, used) = decode_escaped_bytes(bytes.get(1..)?)?;
            Some((KeyScalar::Str(String::from_utf8(decoded).ok()?), 1 + used))
        }
        KEY_BYTES => {
            let (decoded, used) = decode_escaped_bytes(bytes.get(1..)?)?;
            Some((KeyScalar::Bytes(decoded), 1 + used))
        }
        _ => None,
    }
}

pub fn encode_key_into(key: &KeyScalar, out: &mut Vec<u8>) {
    out.push(key.order_tag());
    match key {
        KeyScalar::Bool(value) => {
            out.push(u8::from(*value));
        }
        KeyScalar::Int(value) => {
            out.extend_from_slice(&((*value as u64) ^ (1u64 << 63)).to_be_bytes());
        }
        KeyScalar::Date(value) => {
            out.extend_from_slice(&((*value as u32) ^ (1u32 << 31)).to_be_bytes());
        }
        KeyScalar::Duration(value) => {
            out.extend_from_slice(&((*value as u128) ^ (1u128 << 127)).to_be_bytes());
        }
        KeyScalar::Instant(value) => {
            out.extend_from_slice(&((*value as u128) ^ (1u128 << 127)).to_be_bytes());
        }
        KeyScalar::Str(value) => {
            encode_escaped_bytes(value.as_bytes(), out);
        }
        KeyScalar::Bytes(value) => {
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
    use super::{KeyScalar, decode_key_value, encode_key_tuple, encode_key_value};

    fn representative_keys() -> Vec<KeyScalar> {
        vec![
            KeyScalar::Bool(false),
            KeyScalar::Bool(true),
            KeyScalar::Int(i64::MIN),
            KeyScalar::Int(-1),
            KeyScalar::Int(0),
            KeyScalar::Int(i64::MAX),
            KeyScalar::Date(i32::MIN),
            KeyScalar::Date(-719_162),
            KeyScalar::Date(0),
            KeyScalar::Date(2_932_896),
            KeyScalar::Date(i32::MAX),
            KeyScalar::Instant(i128::MIN),
            KeyScalar::Instant(0),
            KeyScalar::Instant(i128::MAX),
            KeyScalar::Duration(i128::MIN),
            KeyScalar::Duration(-1),
            KeyScalar::Duration(i128::MAX),
            KeyScalar::Str(String::new()),
            KeyScalar::Str("a\u{0}b".into()),
            KeyScalar::Bytes(vec![]),
            KeyScalar::Bytes(vec![0x00, 0x01, 0xff]),
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
            "encoded key bytes must sort like KeyScalar"
        );
    }

    #[test]
    fn saved_key_order_matches_encoded_byte_order_pairwise() {
        let keys = representative_keys();
        for left in &keys {
            for right in &keys {
                assert_eq!(
                    left.cmp(right),
                    encode_key_value(left).cmp(&encode_key_value(right)),
                    "ordering mismatch for {left:?} and {right:?}"
                );
            }
        }
    }

    #[test]
    fn saved_key_codec_preserves_typed_reverse_order_in_bytes() {
        let mut by_type = representative_keys();
        by_type.sort_by(|left, right| right.cmp(left));

        let mut by_bytes = representative_keys();
        by_bytes.sort_by_key(|key| std::cmp::Reverse(encode_key_value(key)));

        assert_eq!(
            by_bytes, by_type,
            "reverse encoded key bytes must sort like reverse KeyScalar order"
        );
    }

    /// Composite tuples whose adjacent columns are NUL-laden or escape-shaped, so a
    /// naive concatenation could let one column's bytes bleed into the next. Because
    /// each column self-delimits, the tuple encoding is prefix-free and sorts
    /// column-major — a difference in an earlier column dominates any later column.
    fn adversarial_tuples() -> Vec<Vec<KeyScalar>> {
        vec![
            vec![KeyScalar::Str("a".into()), KeyScalar::Str("b".into())],
            // A trailing NUL in column 0 must not merge with column 1's frame.
            vec![KeyScalar::Str("a\u{0}".into()), KeyScalar::Str("b".into())],
            vec![KeyScalar::Str("a\u{0}".into()), KeyScalar::Str("".into())],
            // "a\0" > "a" in column 0, so this outranks the plain "a" prefix rows
            // regardless of column 1.
            vec![KeyScalar::Str("a".into()), KeyScalar::Str("b\u{0}c".into())],
            vec![
                KeyScalar::Bytes(vec![0x00]),
                KeyScalar::Bytes(vec![0x00, 0x00]),
            ],
            vec![KeyScalar::Bytes(vec![0x00, 0x00]), KeyScalar::Bytes(vec![])],
            // Mixed fixed-width and variable columns spanning a boundary.
            vec![KeyScalar::Int(-1), KeyScalar::Str("\u{0}".into())],
            vec![KeyScalar::Int(-1), KeyScalar::Str("".into())],
            vec![KeyScalar::Int(0), KeyScalar::Bytes(vec![0x00, 0x01])],
        ]
    }

    #[test]
    fn composite_tuple_encoding_sorts_column_major_across_nul_boundaries() {
        // Byte order of the tuple encoding must equal column-major (lexicographic on
        // columns) order, even where a column ends in a NUL that abuts the next column.
        let tuples = adversarial_tuples();
        for left in &tuples {
            for right in &tuples {
                assert_eq!(
                    left.cmp(right),
                    encode_key_tuple(left).cmp(&encode_key_tuple(right)),
                    "tuple byte order must match column-major order for {left:?} vs {right:?}",
                );
            }
        }
    }

    #[test]
    fn no_composite_tuple_encoding_is_a_prefix_of_a_distinct_one() {
        // Prefix-freeness across column boundaries: no distinct tuple's bytes are a
        // prefix of another's, so a marker built from one tuple can never be a prefix of
        // a sibling's — the property the physical containment/separation laws rest on.
        let tuples = adversarial_tuples();
        for left in &tuples {
            for right in &tuples {
                if left == right {
                    continue;
                }
                let lb = encode_key_tuple(left);
                let rb = encode_key_tuple(right);
                assert!(
                    !rb.starts_with(&lb),
                    "{left:?} encodes to a prefix of {right:?}",
                );
            }
        }
    }

    #[test]
    fn escaped_key_byte_fingerprints_are_stable() {
        assert_eq!(
            encode_key_value(&KeyScalar::Str("a\u{0}b".into())),
            vec![0x07, b'a', 0x00, 0x01, b'b', 0x00, 0x00]
        );
        assert_eq!(
            encode_key_value(&KeyScalar::Bytes(vec![0x00, 0x01, 0xff])),
            vec![0x08, 0x00, 0x01, 0x01, 0xff, 0x00, 0x00]
        );
    }
}
