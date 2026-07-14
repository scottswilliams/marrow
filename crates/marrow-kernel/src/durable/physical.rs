//! Physical cell layout for the T01 store profile (design §G).
//!
//! Durable cells are keyed by name at this stage — a compromise the versioned
//! store profile makes safe (a stale reinterpretation is a typed refusal, not
//! silent misreading); durable identity proper lands at D00, the named deletion
//! point. Every logical entry is a *marker* cell plus one *field leaf* per present
//! field:
//!
//! ```text
//! entry family prefix   0x01 0x20 esc(root_name)
//! marker key            <family> enc(key) 0x00              value = 0x01
//! field leaf key        <family> enc(key) 0x00 0x10 esc(field)   value = codec bytes
//! iteration cursor      <family> enc(key) 0xFF
//! meta cell key         0x10 esc(name)
//! ```
//!
//! The marker is a byte-prefix of every field leaf of its entry, so a marker sorts
//! first among an entry's cells. Because `enc(key)` is prefix-free (fixed width, or
//! `0x00,0x00`-terminated with `0x00,0x01` escapes) and `0x00 < 0x10 < 0xFF`, every
//! cell of entry `k` sorts inside `(marker(k), cursor(k)]` and no cell of another
//! entry does — the property the iteration cursor relies on.

use crate::codec::key::{KeyScalar, decode_key_value, encode_escaped_bytes, encode_key_value};

/// First byte of every entry cell (marker or field leaf).
const ENTRY_FAMILY: u8 = 0x01;
/// Root discriminator, following [`ENTRY_FAMILY`].
const ROOT_TAG: u8 = 0x20;
/// Separator between an entry's marker stem and a field leaf's name.
const FIELD_TAG: u8 = 0x10;
/// Marker-stem terminator; sorts below [`FIELD_TAG`], so the marker precedes leaves.
const MARKER_TERMINATOR: u8 = 0x00;
/// Iteration-cursor sentinel; sorts above every cell of its entry.
const CURSOR_SENTINEL: u8 = 0xFF;
/// First byte of every meta cell (witness, profile). Disjoint from [`ENTRY_FAMILY`].
const META_FAMILY: u8 = 0x10;

/// The value stored at a marker cell: the payload presence record.
pub(super) const MARKER_VALUE: &[u8] = &[0x01];

/// The `0x01 0x20 esc(root)` prefix shared by every cell of `root`'s entries.
pub(super) fn entry_family_prefix(root: &str) -> Vec<u8> {
    let mut out = vec![ENTRY_FAMILY, ROOT_TAG];
    encode_escaped_bytes(root.as_bytes(), &mut out);
    out
}

/// The marker key of entry `key` under `root`.
pub(super) fn marker_key(root: &str, key: &KeyScalar) -> Vec<u8> {
    let mut out = entry_family_prefix(root);
    out.extend_from_slice(&encode_key_value(key));
    out.push(MARKER_TERMINATOR);
    out
}

/// The field-leaf key of `field` of entry `key` under `root`.
pub(super) fn field_leaf_key(root: &str, key: &KeyScalar, field: &str) -> Vec<u8> {
    let mut out = marker_key(root, key);
    out.push(FIELD_TAG);
    encode_escaped_bytes(field.as_bytes(), &mut out);
    out
}

/// The iteration cursor that resumes a forward scan after every cell of `key`.
pub(super) fn cursor(root: &str, key: &KeyScalar) -> Vec<u8> {
    let mut out = entry_family_prefix(root);
    out.extend_from_slice(&encode_key_value(key));
    out.push(CURSOR_SENTINEL);
    out
}

/// A meta cell key in the `0x10` family.
pub(super) fn meta_key(name: &str) -> Vec<u8> {
    let mut out = vec![META_FAMILY];
    encode_escaped_bytes(name.as_bytes(), &mut out);
    out
}

/// Classify a cell key found at or after an iteration cursor, relative to `root`'s
/// entry family. A well-formed marker yields its key; a field leaf with no marker
/// context is an orphan (corruption); a foreign cell ends iteration.
pub(super) enum CellKind {
    /// An entry marker: the decoded key and its cursor for the next step.
    Marker(KeyScalar),
    /// A field leaf sitting where a marker must be — corruption.
    Orphan,
    /// A cell outside this root's entry family: iteration is done.
    Foreign,
}

/// Decode a scanned cell key: is it a marker of `root`, an orphan leaf, or foreign?
pub(super) fn classify_cell(root: &str, cell_key: &[u8]) -> CellKind {
    let prefix = entry_family_prefix(root);
    let Some(rest) = cell_key.strip_prefix(prefix.as_slice()) else {
        return CellKind::Foreign;
    };
    let Some((key, used)) = decode_key_value(rest) else {
        return CellKind::Orphan;
    };
    match &rest[used..] {
        // marker stem terminator and nothing more: a marker.
        [MARKER_TERMINATOR] => CellKind::Marker(key),
        // terminator then a field tag: a field leaf where a marker belongs.
        [MARKER_TERMINATOR, FIELD_TAG, ..] => CellKind::Orphan,
        _ => CellKind::Orphan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The ordering property iteration relies on: for keys k < k',
    // marker(k) < every cell of k < cursor(k) < marker(k').
    fn assert_between(root: &str, key: &KeyScalar, fields: &[&str]) {
        let marker = marker_key(root, key);
        let cur = cursor(root, key);
        assert!(marker < cur, "marker precedes cursor for {key:?}");
        for field in fields {
            let leaf = field_leaf_key(root, key, field);
            assert!(marker < leaf, "marker precedes leaf {field} for {key:?}");
            assert!(leaf < cur, "leaf {field} precedes cursor for {key:?}");
        }
    }

    #[test]
    fn cursor_separates_adjacent_and_prefix_related_keys() {
        let root = "counters";
        let keys = [
            KeyScalar::Int(i64::MIN),
            KeyScalar::Int(-1),
            KeyScalar::Int(0),
            KeyScalar::Int(1),
            KeyScalar::Int(i64::MAX),
            KeyScalar::Str(String::new()),
            KeyScalar::Str("a".into()),
            KeyScalar::Str("a\u{0}".into()),
            KeyScalar::Str("ab".into()),
            KeyScalar::Bool(false),
            KeyScalar::Bool(true),
        ];
        let mut sorted = keys.to_vec();
        sorted.sort();
        for key in &sorted {
            assert_between(root, key, &["value", "label"]);
        }
        // Between consecutive keys, cursor(k) < marker(k').
        for pair in sorted.windows(2) {
            let cur = cursor(root, &pair[0]);
            let next_marker = marker_key(root, &pair[1]);
            assert!(
                cur < next_marker,
                "cursor({:?}) precedes marker({:?})",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn classify_distinguishes_marker_leaf_and_foreign() {
        let root = "counters";
        let key = KeyScalar::Str("a".into());
        assert!(matches!(
            classify_cell(root, &marker_key(root, &key)),
            CellKind::Marker(k) if k == key
        ));
        assert!(matches!(
            classify_cell(root, &field_leaf_key(root, &key, "value")),
            CellKind::Orphan
        ));
        assert!(matches!(
            classify_cell(root, &meta_key("witness")),
            CellKind::Foreign
        ));
    }

    #[test]
    fn meta_family_is_disjoint_from_entries() {
        let root = "counters";
        let entry = marker_key(root, &KeyScalar::Int(0));
        let meta = meta_key("profile");
        assert_ne!(entry.first(), meta.first());
    }
}
