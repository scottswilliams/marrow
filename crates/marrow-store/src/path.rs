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

// Key-type tags, in Marrow's typed key order (booleans before numbers, ...).
const KEY_BOOL: u8 = 0x01;
const KEY_INT: u8 = 0x02;

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
    }
}
