//! Saved-path encoding.
//!
//! A Marrow saved path is a sequence of [`PathSegment`]s. Each segment encodes
//! to a self-delimiting byte run whose lexicographic order matches Marrow's
//! ordering rules (docs/language/types.md, docs/language/resources-and-storage.md):
//! at one tree level record keys sort before named members; integer keys sort
//! by numeric value; booleans sort false before true; names sort by UTF-8 byte
//! order. The byte layout is Marrow's own, so a backend that merely orders raw
//! bytes yields Marrow order regardless of its locale or collation.

/// A scalar key value in a record-key or index-key position. Keys encode to
/// order-preserving bytes, so byte order is Marrow key order.
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

/// One segment of a saved path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// The saved root, e.g. `^books` is `Root("books")`. Always the first
    /// segment of a path.
    Root(String),
    /// An identity key value, e.g. the `id` in `^books(id)`.
    RecordKey(SavedKey),
    /// A declared field name, e.g. the `title` in `^books(id).title`.
    Field(String),
    /// A declared child-layer name, e.g. `versions`.
    ChildLayer(String),
    /// A declared index name, e.g. `byShelf`.
    Index(String),
    /// A key value inside an index or child layer.
    IndexKey(SavedKey),
}

// Segment-kind tags. Their values define cross-kind order at one tree level: a
// record key (path component) sorts before a named member (field/layer/index),
// matching the tree shape. Fields, child layers, and index names share one tag
// because the schema already forbids a name collision among them, so the byte
// order is simply their UTF-8 order.
const KIND_ROOT: u8 = 0x01;
const KIND_RECORD_KEY: u8 = 0x02;
const KIND_NAMED: u8 = 0x03;
const KIND_INDEX_KEY: u8 = 0x04;

// Key-type tags, in Marrow's typed key order: booleans, numbers, then dates,
// instants, and durations, then strings (docs/language/types.md).
const KEY_BOOL: u8 = 0x01;
const KEY_INT: u8 = 0x02;
const KEY_DATE: u8 = 0x03;
const KEY_INSTANT: u8 = 0x04;
const KEY_DURATION: u8 = 0x05;
const KEY_STR: u8 = 0x07;
const KEY_BYTES: u8 = 0x08;

/// Encode a saved path to its ordered byte key.
pub fn encode_path(segments: &[PathSegment]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for segment in segments {
        match segment {
            PathSegment::Root(name) => {
                bytes.push(KIND_ROOT);
                encode_name(name, &mut bytes);
            }
            PathSegment::RecordKey(key) => {
                bytes.push(KIND_RECORD_KEY);
                encode_key(key, &mut bytes);
            }
            PathSegment::Field(name) | PathSegment::ChildLayer(name) | PathSegment::Index(name) => {
                bytes.push(KIND_NAMED);
                encode_name(name, &mut bytes);
            }
            PathSegment::IndexKey(key) => {
                bytes.push(KIND_INDEX_KEY);
                encode_key(key, &mut bytes);
            }
        }
    }
    bytes
}

/// Encode a single scalar key to its order-preserving bytes for a value
/// position — for example the identity a unique index entry points to. The
/// encoding is self-delimiting, so several keys may be concatenated and walked
/// back one at a time with [`decode_key_value`].
pub fn encode_key_value(key: &SavedKey) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_key(key, &mut bytes);
    bytes
}

/// Decode one scalar key from the front of `bytes`, returning the key and the
/// number of bytes it consumed, or `None` if the front is not a well-formed
/// key. The length lets a concatenation of encoded keys be walked in order.
pub fn decode_key_value(bytes: &[u8]) -> Option<(SavedKey, usize)> {
    Some((decode_key(bytes)?, key_len(bytes)?))
}

/// Append a name as UTF-8 bytes terminated by `0x00`. Names are Marrow
/// identifiers or quoted data names, which do not contain `0x00`.
fn encode_name(name: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(name.as_bytes());
    out.push(0x00);
}

/// Append a scalar key: a type tag followed by order-preserving type bytes.
fn encode_key(key: &SavedKey, out: &mut Vec<u8>) {
    match key {
        SavedKey::Bool(value) => {
            out.push(KEY_BOOL);
            out.push(u8::from(*value));
        }
        SavedKey::Int(value) => {
            out.push(KEY_INT);
            // Flip the sign bit so two's-complement big-endian bytes sort in
            // signed numeric order: i64::MIN encodes to all-zero, i64::MAX to
            // all-one.
            out.extend_from_slice(&((*value as u64) ^ (1u64 << 63)).to_be_bytes());
        }
        SavedKey::Date(value) => {
            out.push(KEY_DATE);
            // Days since the epoch, sign-flipped big-endian, so dates sort
            // chronologically just like signed integers.
            out.extend_from_slice(&((*value as u32) ^ (1u32 << 31)).to_be_bytes());
        }
        SavedKey::Duration(value) => {
            out.push(KEY_DURATION);
            // Signed nanoseconds, sign-flipped big-endian, so durations sort by
            // signed length: more-negative spans first.
            out.extend_from_slice(&((*value as u128) ^ (1u128 << 127)).to_be_bytes());
        }
        SavedKey::Instant(value) => {
            out.push(KEY_INSTANT);
            // Nanoseconds since the epoch (UTC), sign-flipped big-endian, so
            // instants sort chronologically.
            out.extend_from_slice(&((*value as u128) ^ (1u128 << 127)).to_be_bytes());
        }
        SavedKey::Str(value) => {
            out.push(KEY_STR);
            encode_escaped(value.as_bytes(), out);
        }
        SavedKey::Bytes(value) => {
            out.push(KEY_BYTES);
            encode_escaped(value, out);
        }
    }
}

/// Append an order-preserving escaped byte run for a `str` or `bytes` key:
/// escape `0x00` as `0x00 0x01` and terminate with `0x00 0x00`. The run is
/// self-delimiting within a longer path, and a shorter value sorts before a
/// longer one that extends it (UTF-8 / byte order is preserved).
fn encode_escaped(value: &[u8], out: &mut Vec<u8>) {
    for &byte in value {
        out.push(byte);
        if byte == 0x00 {
            out.push(0x01);
        }
    }
    out.push(0x00);
    out.push(0x00);
}

/// An immediate child of a path: either a key value (a record or index key) or a
/// member name. The store cannot tell a field, child layer, or index name apart
/// from bytes alone — the schema does that — so all three decode to [`Name`].
///
/// [`Name`]: ChildSegment::Name
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildSegment {
    Key(SavedKey),
    Name(String),
}

/// The byte length of the path segment at the front of `bytes`, or `None` if it
/// is not a well-formed segment. Lets callers walk an encoded path one segment
/// at a time without fully decoding it.
pub(crate) fn segment_len(bytes: &[u8]) -> Option<usize> {
    match *bytes.first()? {
        KIND_ROOT | KIND_NAMED => name_segment_len(bytes),
        KIND_RECORD_KEY | KIND_INDEX_KEY => Some(1 + key_len(bytes.get(1..)?)?),
        _ => None,
    }
}

/// The root name of an encoded path, or `None` if it does not begin with a root
/// segment. Root names are plain identifiers, so this decode is lossless.
pub(crate) fn root_name(bytes: &[u8]) -> Option<String> {
    if *bytes.first()? != KIND_ROOT {
        return None;
    }
    decode_name(bytes)
}

/// Decode one segment as an immediate child: a key for record/index segments, a
/// name for a named member. A root segment is never a child, so returns `None`.
pub(crate) fn decode_child_segment(bytes: &[u8]) -> Option<ChildSegment> {
    match *bytes.first()? {
        KIND_NAMED => Some(ChildSegment::Name(decode_name(bytes)?)),
        KIND_RECORD_KEY | KIND_INDEX_KEY => Some(ChildSegment::Key(decode_key(bytes.get(1..)?)?)),
        _ => None,
    }
}

/// The length of a tag-and-name segment: tag, name bytes, `0x00` terminator.
fn name_segment_len(bytes: &[u8]) -> Option<usize> {
    let terminator = bytes.iter().skip(1).position(|&b| b == 0)?;
    Some(1 + terminator + 1)
}

/// Decode the name from a tag-and-name segment (skipping the kind tag).
fn decode_name(bytes: &[u8]) -> Option<String> {
    let terminator = bytes.iter().skip(1).position(|&b| b == 0)?;
    String::from_utf8(bytes[1..1 + terminator].to_vec()).ok()
}

/// The byte length of a key encoding: its type tag plus the typed bytes.
fn key_len(bytes: &[u8]) -> Option<usize> {
    match *bytes.first()? {
        KEY_BOOL => Some(2),
        KEY_INT => Some(9),
        KEY_DATE => Some(5),
        KEY_DURATION | KEY_INSTANT => Some(17),
        KEY_STR | KEY_BYTES => Some(1 + read_escaped_str(bytes.get(1..)?)?.1),
        _ => None,
    }
}

/// Decode a key encoding (type tag + typed bytes) back to a [`SavedKey`].
fn decode_key(bytes: &[u8]) -> Option<SavedKey> {
    match *bytes.first()? {
        KEY_BOOL => Some(SavedKey::Bool(*bytes.get(1)? != 0)),
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

/// Read an escaped string key body (the bytes after the `KEY_STR` tag):
/// unescape `0x00 0x01` back to `0x00` and stop at the `0x00 0x00` terminator.
/// Returns the decoded bytes and the number of body bytes consumed, including
/// the terminator.
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
