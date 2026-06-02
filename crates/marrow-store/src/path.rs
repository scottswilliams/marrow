//! Saved-path encoding.
//!
//! A Marrow saved path is a sequence of [`PathSegment`]s. Each segment encodes
//! to a self-delimiting byte run whose lexicographic order matches Marrow's
//! ordering rules:
//! at one tree level record keys sort before named members; integer keys sort
//! by numeric value; booleans sort false before true; names sort by UTF-8 byte
//! order. The byte layout is Marrow's own, so a backend that merely orders raw
//! bytes yields Marrow order regardless of its locale or collation.

pub use crate::key::{SavedKey, decode_key_value, encode_key_value};

#[cfg(test)]
use crate::key::KEY_STR;
use crate::key::{KEY_INT, decode_key, encode_key_into, key_len};
use crate::value::{SavedValue, ScalarType, decode_value, encode_value};

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
                encode_key_into(key, &mut bytes);
            }
            PathSegment::Field(name) | PathSegment::ChildLayer(name) | PathSegment::Index(name) => {
                bytes.push(KIND_NAMED);
                encode_name(name, &mut bytes);
            }
            PathSegment::IndexKey(key) => {
                bytes.push(KIND_INDEX_KEY);
                encode_key_into(key, &mut bytes);
            }
        }
    }
    bytes
}

/// The half-open byte range `[lo, hi)` over the immediate integer record-key
/// children of `prefix`. Integer record keys all share the `prefix`, the
/// record-key kind tag, and the integer key-type tag, and their sign-flipped
/// big-endian bodies sort in numeric order, so they form one contiguous band; a
/// backend ranges over `lo..hi` and takes the last entry to find the highest
/// integer record key without scanning every child. `path.rs` owns these bounds
/// so the store never references the tag constants.
pub(crate) fn int_record_key_band(prefix: &[u8]) -> (Vec<u8>, Vec<u8>) {
    int_key_band(prefix, KIND_RECORD_KEY)
}

/// The half-open byte range `[lo, hi)` over the immediate integer index-key
/// children of `prefix` (the positions inside a keyed child layer). The layout
/// matches [`int_record_key_band`] but with the index-key kind tag, so a backend
/// finds the highest integer position under a layer the same bounded way it finds
/// the highest record key under a root.
pub(crate) fn int_index_key_band(prefix: &[u8]) -> (Vec<u8>, Vec<u8>) {
    int_key_band(prefix, KIND_INDEX_KEY)
}

/// The half-open byte range `[lo, hi)` covering exactly the subtree at `prefix`:
/// `prefix`'s own entry and every descendant, and nothing else. `lo` is `prefix`
/// itself; `hi` is its least byte successor — the prefix with its last byte below
/// `0xff` raised by one, after dropping any trailing `0xff` bytes. Trailing `0xff`
/// can occur in a well-formed path (`i64::MAX` and the maximum date/duration/
/// instant encode to all-`0xff` bodies), so a composite-identity prefix may end in
/// it; the byte before is then a lower key-type tag, which becomes the bound. When
/// the prefix is empty (the whole store) or every byte is `0xff`, there is no
/// successor and `hi` is `None`, meaning the subtree runs to the end of the store.
/// A reversed range needs this upper bound: an unbounded reverse range starts at
/// the global maximum, where the first rows lie outside the subtree, so the bound
/// keeps the walk inside it. `path.rs` owns this byte math so the store never
/// hand-rolls it.
pub(crate) fn subtree_band(prefix: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
    let mut hi = prefix.to_vec();
    while let Some(last) = hi.last_mut() {
        if *last < 0xff {
            *last += 1;
            return (prefix.to_vec(), Some(hi));
        }
        hi.pop();
    }
    // An empty prefix (the whole store) or one of all `0xff` bytes has no byte
    // successor: the subtree runs to the end of the store.
    (prefix.to_vec(), None)
}

/// Build the half-open `[lo, hi)` band over the immediate integer children of
/// `prefix` carrying `kind`'s tag: the band starts at the lowest integer key
/// (the integer key-type tag with an empty body) and ends just past the highest
/// (the next type tag), so the run is exactly the integer keys of that kind.
fn int_key_band(prefix: &[u8], kind: u8) -> (Vec<u8>, Vec<u8>) {
    let mut lo = prefix.to_vec();
    lo.push(kind);
    lo.push(KEY_INT);
    let mut hi = prefix.to_vec();
    hi.push(kind);
    hi.push(KEY_INT + 1);
    (lo, hi)
}

/// Append a name as UTF-8 bytes terminated by `0x00`. Names are Marrow
/// identifiers or quoted data names, which do not contain `0x00`.
fn encode_name(name: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(name.as_bytes());
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

/// Decode a whole encoded key into its segments, or `None` if any segment is
/// malformed. The inverse of [`encode_path`]; the store cannot distinguish a
/// field, child-layer, or index name from bytes alone — the schema does that —
/// so all three named kinds decode to [`PathSegment::Field`]. An empty key
/// decodes to no segments.
pub fn decode_path(bytes: &[u8]) -> Option<Vec<PathSegment>> {
    let mut segments = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let len = segment_len(rest)?;
        segments.push(decode_segment(&rest[..len])?);
        rest = &rest[len..];
    }
    Some(segments)
}

/// Append the lowercase two-digit hex of each byte to `out`, writing in place so
/// no per-byte heap allocation is needed.
fn push_hex(out: &mut String, bytes: &[u8]) {
    use std::fmt::Write;
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
}

/// Render an encoded key as canonical Marrow path text for raw inspection, e.g.
/// `^books(1).versions("v2").title`. Uses the stable encoded segment order. Never
/// fails: a key that does not decode renders as `?<hex>` so a corrupt key is
/// still visible to an operator rather than silently dropped.
pub fn display_path(bytes: &[u8]) -> String {
    let Some(segments) = decode_path(bytes) else {
        let mut text = String::from("?");
        push_hex(&mut text, bytes);
        return text;
    };
    let mut text = String::new();
    for segment in &segments {
        match segment {
            PathSegment::Root(name) => {
                text.push('^');
                text.push_str(name);
            }
            // Fields, child layers, and index names share one byte kind, so they
            // all render with the `.name` member form.
            PathSegment::Field(name) | PathSegment::ChildLayer(name) | PathSegment::Index(name) => {
                text.push('.');
                text.push_str(name);
            }
            PathSegment::RecordKey(key) | PathSegment::IndexKey(key) => {
                text.push('(');
                text.push_str(&display_key(key));
                text.push(')');
            }
        }
    }
    text
}

/// The Marrow path-text grammar rejected a `<path>` argument. The message names
/// what was expected so a CLI user can correct the path they typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathParseError {
    pub message: String,
}

/// Parse Marrow path text (the surface [`display_path`] emits, e.g.
/// `^books(1).title`) into path segments, the inverse of `display_path`. A
/// leading `^root`, then `.name` members and `(key)` keys; the store does not
/// distinguish a record key from an index key in bytes, so a parsed `(key)`
/// after a name is a record key and a `(key)` after another key is an index key,
/// matching the encode-side kinds.
pub fn parse_path(text: &str) -> Result<Vec<PathSegment>, PathParseError> {
    let mut parser = PathTextParser {
        rest: text.trim(),
        segments: Vec::new(),
        seen_member: false,
    };
    parser.parse()?;
    Ok(parser.segments)
}

/// Decode a single self-delimiting segment to its [`PathSegment`], preserving its
/// kind tag. Unlike [`decode_child_segment`] it keeps the four kinds distinct so
/// a whole path round-trips; named members still collapse to [`PathSegment::Field`]
/// because the schema, not the bytes, knows field vs. layer vs. index.
fn decode_segment(bytes: &[u8]) -> Option<PathSegment> {
    match *bytes.first()? {
        KIND_ROOT => Some(PathSegment::Root(decode_name(bytes)?)),
        KIND_NAMED => Some(PathSegment::Field(decode_name(bytes)?)),
        KIND_RECORD_KEY => Some(PathSegment::RecordKey(decode_key(bytes.get(1..)?)?)),
        KIND_INDEX_KEY => Some(PathSegment::IndexKey(decode_key(bytes.get(1..)?)?)),
        _ => None,
    }
}

/// Render a key as its canonical Marrow literal: an int as decimal, a bool as
/// `true`/`false`, a string as a quoted literal, bytes as `0x<hex>`, and a date,
/// instant, or duration through the one canonical value formatter so a key reads
/// the same as its value would.
fn display_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Bytes(value) => {
            let mut text = String::from("0x");
            push_hex(&mut text, value);
            text
        }
        // Date/instant/duration keys reuse the canonical value codec so a path
        // shows the same ISO text the value side prints; an out-of-range calendar
        // year (which the codec rejects) falls back to a tagged numeric form.
        SavedKey::Date(days) => render_temporal(SavedValue::Date(*days)),
        SavedKey::Instant(nanos) => render_temporal(SavedValue::Instant(*nanos)),
        SavedKey::Duration(nanos) => render_temporal(SavedValue::Duration(*nanos)),
    }
}

/// Render a temporal key value through the canonical value codec, falling back to
/// a debug form only for an out-of-envelope calendar year the codec cannot spell.
fn render_temporal(value: SavedValue) -> String {
    match encode_value(&value) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| format!("{value:?}")),
        Err(_) => format!("{value:?}"),
    }
}

/// A recursive-descent parser over Marrow path text.
struct PathTextParser<'a> {
    rest: &'a str,
    segments: Vec<PathSegment>,
    /// Whether a named member has been parsed yet. Keys before the first member
    /// are identity record keys directly under the root; keys after a member are
    /// index keys inside a layer or index, matching the encode-side kinds.
    seen_member: bool,
}

impl PathTextParser<'_> {
    fn parse(&mut self) -> Result<(), PathParseError> {
        let after_root = self
            .rest
            .strip_prefix('^')
            .ok_or_else(|| self.error("a saved path starts with `^root`"))?;
        let (root, rest) = split_name(after_root);
        if root.is_empty() {
            return Err(self.error("a saved root name after `^`"));
        }
        self.segments.push(PathSegment::Root(root.to_string()));
        self.rest = rest;

        while !self.rest.is_empty() {
            match self.rest.as_bytes()[0] {
                b'.' => {
                    let (name, rest) = split_name(&self.rest[1..]);
                    if name.is_empty() {
                        return Err(self.error("a member name after `.`"));
                    }
                    self.segments.push(PathSegment::Field(name.to_string()));
                    self.rest = rest;
                    self.seen_member = true;
                }
                b'(' => {
                    let close = self
                        .rest
                        .find(')')
                        .ok_or_else(|| self.error("a closing `)` for a key"))?;
                    let key = self.parse_key(&self.rest[1..close])?;
                    // Keys before any named member are identity record keys under
                    // the root; keys after a member are index keys in a layer/index.
                    self.segments.push(if self.seen_member {
                        PathSegment::IndexKey(key)
                    } else {
                        PathSegment::RecordKey(key)
                    });
                    self.rest = &self.rest[close + 1..];
                }
                _ => return Err(self.error("`.name` or `(key)` after a path segment")),
            }
        }
        Ok(())
    }

    /// Parse a key literal between `(` and `)`: an int, `true`/`false`, a quoted
    /// string, `0x<hex>` bytes, or a temporal literal read back through the value
    /// codec so it matches `display_key`.
    fn parse_key(&self, text: &str) -> Result<SavedKey, PathParseError> {
        let text = text.trim();
        if let Some(quoted) = text.strip_prefix('"') {
            let inner = quoted
                .strip_suffix('"')
                .ok_or_else(|| self.error("a closing quote in a string key"))?;
            return Ok(SavedKey::Str(unescape_string(inner)));
        }
        if let Some(hex) = text.strip_prefix("0x") {
            let bytes = decode_hex(hex).ok_or_else(|| self.error("valid hex bytes after `0x`"))?;
            return Ok(SavedKey::Bytes(bytes));
        }
        if text == "true" {
            return Ok(SavedKey::Bool(true));
        }
        if text == "false" {
            return Ok(SavedKey::Bool(false));
        }
        if let Ok(value) = text.parse::<i64>() {
            return Ok(SavedKey::Int(value));
        }
        // A temporal literal: decode it with the canonical value codec, which
        // accepts exactly the ISO forms `display_key` emits.
        if let Some(SavedValue::Date(days)) = decode_value(text.as_bytes(), ScalarType::Date) {
            return Ok(SavedKey::Date(days));
        }
        if let Some(SavedValue::Instant(nanos)) = decode_value(text.as_bytes(), ScalarType::Instant)
        {
            return Ok(SavedKey::Instant(nanos));
        }
        if let Some(SavedValue::Duration(nanos)) =
            decode_value(text.as_bytes(), ScalarType::Duration)
        {
            return Ok(SavedKey::Duration(nanos));
        }
        Err(self.error(
            "a key literal: an int, true/false, \"text\", 0x<hex>, or an ISO date/instant/duration",
        ))
    }

    fn error(&self, expected: &str) -> PathParseError {
        PathParseError {
            message: format!("malformed saved path: expected {expected}"),
        }
    }
}

/// Split a leading member/root name (up to the next `.` or `(`) from the rest.
fn split_name(text: &str) -> (&str, &str) {
    let end = text.find(['.', '(']).unwrap_or(text.len());
    (&text[..end], &text[end..])
}

/// Unescape a Rust-style double-quoted body the way `format!("{:?}")` produced
/// it: `\"`, `\\`, `\n`, `\t`, `\r`, and `\0` to their characters.
fn unescape_string(inner: &str) -> String {
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Decode an even-length lowercase/uppercase hex string to bytes, or `None`.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let chars = text.as_bytes();
    for pair in chars.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        bytes.push((hi * 16 + lo) as u8);
    }
    Some(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One path of every segment and key kind: a root, an integer record key, a
    /// named field, a child layer with a string index key, and the temporal/bytes/bool
    /// keys. After encode→decode every kind survives, with named members
    /// collapsing to `Field` as documented.
    fn every_kind() -> Vec<PathSegment> {
        vec![
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::Field("versions".into()),
            PathSegment::IndexKey(SavedKey::Str("v2".into())),
            PathSegment::Field("title".into()),
        ]
    }

    #[test]
    fn decode_path_inverts_encode_path_collapsing_named_members() {
        // ChildLayer/Index encode like Field, so they decode back as Field; an
        // already-Field path round-trips exactly.
        let original = every_kind();
        let decoded = decode_path(&encode_path(&original)).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_path_round_trips_every_key_type() {
        for key in [
            SavedKey::Int(-9),
            SavedKey::Bool(true),
            SavedKey::Bool(false),
            SavedKey::Str("hi\0there".into()),
            SavedKey::Date(19_000),
            SavedKey::Instant(1_700_000_000_000_000_000),
            SavedKey::Duration(1_500_000_000),
            SavedKey::Bytes(vec![0x00, 0xff, 0x10]),
        ] {
            let path = vec![
                PathSegment::Root("r".into()),
                PathSegment::RecordKey(key.clone()),
            ];
            let decoded = decode_path(&encode_path(&path)).expect("decodes");
            assert_eq!(decoded, path, "{key:?}");
        }
    }

    #[test]
    fn empty_key_decodes_to_no_segments() {
        assert_eq!(decode_path(&[]), Some(Vec::new()));
    }

    #[test]
    fn decode_path_rejects_a_malformed_key() {
        // A record-key tag followed by an unknown key-type tag is not decodable.
        assert_eq!(decode_path(&[KIND_RECORD_KEY, 0xfe]), None);
    }

    #[test]
    fn decode_path_rejects_a_truncated_segment() {
        // A torn write can leave a known tag with a too-short body. Each of these is
        // a valid tag whose declared body is cut off, which must reject rather than
        // read past the end or decode a partial key.
        //
        // An integer key is a tag plus eight bytes; a two-byte body is short.
        assert_eq!(decode_path(&[KIND_RECORD_KEY, KEY_INT, 0x00, 0x00]), None);
        // A string key body runs to a `00 00` terminator; without it the run is
        // unterminated and the segment cannot be delimited.
        assert_eq!(
            decode_path(&[KIND_RECORD_KEY, KEY_STR, b'a', b'b']),
            None,
            "an unterminated string key is rejected"
        );
        // A root name runs to a `00` terminator; a name with none is unterminated.
        assert_eq!(
            decode_path(&[KIND_ROOT, b'r', b'o', b'o', b't']),
            None,
            "an unterminated root name is rejected"
        );
    }

    #[test]
    fn subtree_band_handles_a_trailing_max_int_key() {
        // A composite prefix ending in an i64::MAX record key has an all-0xff key
        // body, so the successor cannot simply bump the last byte. `subtree_band`
        // must drop the trailing 0xff bytes and raise the key-type tag, yielding a
        // strict successor that still bounds the subtree.
        let prefix = encode_path(&[
            PathSegment::Root("r".into()),
            PathSegment::RecordKey(SavedKey::Int(i64::MAX)),
        ]);
        let (lo, hi) = subtree_band(&prefix);
        assert_eq!(lo, prefix, "lo is the prefix itself");
        let hi = hi.expect("a non-empty prefix has a byte successor");
        assert!(hi > prefix, "hi is a strict successor of the prefix");
        // A descendant of the prefix (a field under that record) sorts inside the
        // band: at or after lo and strictly before hi.
        let descendant = encode_path(&[
            PathSegment::Root("r".into()),
            PathSegment::RecordKey(SavedKey::Int(i64::MAX)),
            PathSegment::Field("title".into()),
        ]);
        assert!(
            descendant >= lo && descendant < hi,
            "a descendant is in the band"
        );

        // An all-0xff prefix has no byte successor: the subtree runs to the end of
        // the store, so `hi` is `None`.
        assert_eq!(subtree_band(&[0xff, 0xff]).1, None);
        // The empty prefix (the whole store) likewise has no successor.
        assert_eq!(subtree_band(&[]).1, None);
    }

    #[test]
    fn display_path_renders_canonical_marrow_text() {
        assert_eq!(
            display_path(&encode_path(&every_kind())),
            "^books(1).versions(\"v2\").title"
        );
    }

    #[test]
    fn display_path_renders_each_key_literal() {
        let cases = [
            (SavedKey::Int(42), "^r(42)"),
            (SavedKey::Bool(true), "^r(true)"),
            (SavedKey::Str("hi".into()), "^r(\"hi\")"),
            (SavedKey::Bytes(vec![0x0a, 0xff]), "^r(0x0aff)"),
            (SavedKey::Date(0), "^r(1970-01-01)"),
        ];
        for (key, expected) in cases {
            let path = vec![
                PathSegment::Root("r".into()),
                PathSegment::RecordKey(key.clone()),
            ];
            assert_eq!(display_path(&encode_path(&path)), expected, "{key:?}");
        }
    }

    #[test]
    fn display_path_renders_a_corrupt_key_as_hex() {
        assert_eq!(display_path(&[KIND_RECORD_KEY, 0xfe]), "?02fe");
    }

    #[test]
    fn parse_path_inverts_display_path() {
        // The parser yields the same segments display_path was built from: named
        // members are Field, the first key after a name is a RecordKey, and a
        // following key is an IndexKey — exactly the every_kind shape.
        let text = display_path(&encode_path(&every_kind()));
        assert_eq!(parse_path(&text), Ok(every_kind()));
    }

    #[test]
    fn parse_path_round_trips_key_literals_through_bytes() {
        for key in [
            SavedKey::Int(-7),
            SavedKey::Bool(false),
            SavedKey::Str("a quote \" inside".into()),
            SavedKey::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
            SavedKey::Date(19_000),
            SavedKey::Instant(1_700_000_000_000_000_000),
            SavedKey::Duration(2_000_000_000),
        ] {
            let path = vec![
                PathSegment::Root("r".into()),
                PathSegment::RecordKey(key.clone()),
            ];
            let text = display_path(&encode_path(&path));
            let parsed = parse_path(&text).expect("parses");
            // The parsed path encodes back to the very same bytes.
            assert_eq!(
                encode_path(&parsed),
                encode_path(&path),
                "{key:?} via {text}"
            );
        }
    }

    #[test]
    fn parse_path_requires_a_caret_root() {
        assert!(parse_path("books(1)").is_err());
        assert!(parse_path("").is_err());
    }

    #[test]
    fn parse_path_rejects_a_bad_key_literal() {
        assert!(parse_path("^r(not-a-key)").is_err());
        assert!(parse_path("^r(1").is_err());
    }

    #[test]
    fn keys_are_the_orderable_projection_of_scalars() {
        // A key is exactly the orderable projection of its scalar value, so the
        // scalar maps back to the same key; its wire tag is the stable serve
        // spelling (`str`, not the `string` source keyword).
        for (value, key, tag) in [
            (SavedValue::Int(-9), SavedKey::Int(-9), "int"),
            (SavedValue::Bool(true), SavedKey::Bool(true), "bool"),
            (
                SavedValue::Str("v".into()),
                SavedKey::Str("v".into()),
                "str",
            ),
            (
                SavedValue::Bytes(vec![0x01]),
                SavedKey::Bytes(vec![0x01]),
                "bytes",
            ),
            (SavedValue::Date(19_000), SavedKey::Date(19_000), "date"),
            (
                SavedValue::Duration(1_500_000_000),
                SavedKey::Duration(1_500_000_000),
                "duration",
            ),
            (
                SavedValue::Instant(1_700_000_000_000_000_000),
                SavedKey::Instant(1_700_000_000_000_000_000),
                "instant",
            ),
        ] {
            assert_eq!(value.as_key(), Some(key.clone()), "{key:?}");
            assert_eq!(key.wire_tag(), tag, "{key:?}");
            // A key carries the scalar kind of the value it projects, so the key
            // guard can compare a written key's kind against a declared key type.
            assert_eq!(key.scalar_type(), value.ty(), "{key:?}");
        }
        // A decimal has no order-preserving key encoding.
        assert_eq!(SavedValue::Decimal(crate::Decimal::ZERO).as_key(), None);
    }
}
