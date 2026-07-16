//! Physical cell layout for the T01 store profile (design §G).
//!
//! Durable cells are keyed by name at this stage — a compromise the versioned
//! store profile makes safe (a stale reinterpretation is a typed refusal, not
//! silent misreading); durable identity proper lands at D00, the named deletion
//! point. Every logical entry is a *marker* cell (its payload-presence record) plus
//! one *field leaf* per present field, and — for a hierarchical resource — one keyed
//! *branch* family per declared branch nested beneath the entry's marker:
//!
//! ```text
//! entry family prefix   0x01 0x20 esc(root_name)
//! marker key            <family> enc(key) 0x00                        value = 0x01
//! field leaf key        <marker> 0x10 esc(field)                      value = codec bytes
//! branch child marker   <marker> 0x30 esc(branch) enc(childKey) 0x00  value = 0x01
//! iteration cursor      <family> enc(key) 0xFF
//! meta cell key         0x10 esc(name)
//! ```
//!
//! The layout is recursive: a branch child's marker (`<marker> 0x30 esc(branch)
//! enc(childKey) 0x00`) is itself a marker stem, so the child's own field leaves,
//! nested branches, and iteration cursor derive from it exactly as a root entry's
//! do. An entry's marker is therefore a byte-prefix of every cell it owns — its
//! field leaves and its whole branch subtree — so the marker sorts first among the
//! entry's cells.
//!
//! Because `enc(key)` is prefix-free (fixed width, or `0x00,0x00`-terminated with
//! `0x00,0x01` escapes) and the structural tags ascend `0x00 < 0x10 < 0x30 < 0xFF`
//! (marker terminator, field, branch, cursor), every cell of entry `k` — including
//! every descendant in every branch — sorts inside `(marker(k), cursor(k)]`, and no
//! cell of another entry does. Two consequences the kernel relies on: one
//! prefix-successor seek past `cursor(k)` skips `k`'s whole subtree regardless of
//! branch fan-out (the traversal-skip law), and `k`'s own field leaves (`0x10`) sort
//! ahead of its branch descendants (`0x30`), so a scan of `k`'s cells meets an
//! orphan own-leaf before any descendant — the precedence the bounded prefix probe
//! uses to surface a marker/field corruption ahead of a legitimate descendant-only
//! node.

use crate::codec::key::{KeyScalar, decode_key_value, encode_escaped_bytes, encode_key_value};

/// First byte of every entry cell (marker or field leaf).
const ENTRY_FAMILY: u8 = 0x01;
/// Root discriminator, following [`ENTRY_FAMILY`].
const ROOT_TAG: u8 = 0x20;
/// Separator between an entry's marker stem and a field leaf's name.
const FIELD_TAG: u8 = 0x10;
/// Separator introducing a keyed branch family beneath an entry's marker stem.
/// Sorts above [`FIELD_TAG`], so an entry's own field leaves precede its branch
/// descendants (the precedence the bounded prefix probe relies on to surface an
/// orphan own-leaf ahead of a legitimate descendant-only node), and below
/// [`CURSOR_SENTINEL`], so every branch descendant stays inside the entry's
/// `(marker, cursor]` range and one seek past the cursor skips the whole subtree.
const BRANCH_TAG: u8 = 0x30;
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

/// The marker key of entry `key` under `root`. A root entry's marker key is its
/// marker *stem*: the byte prefix from which its field leaves, its branch
/// descendants, and its cursor all derive (see [`stem_field_leaf`], [`stem_cursor`]).
pub(super) fn marker_key(root: &str, key: &KeyScalar) -> Vec<u8> {
    let mut out = entry_family_prefix(root);
    out.extend_from_slice(&encode_key_value(key));
    out.push(MARKER_TERMINATOR);
    out
}

/// The field-leaf key of `field` of the entry whose marker `stem` is given. The leaf
/// extends the stem with the field tag and the escaped field name, so it nests
/// inside the entry's `(marker, cursor]` range ahead of any branch descendant. This
/// is the single owner of the marker-stem-to-field-leaf mapping; the flat
/// [`field_leaf_key`] is the root-entry convenience over it.
pub(super) fn stem_field_leaf(stem: &[u8], field: &str) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(FIELD_TAG);
    encode_escaped_bytes(field.as_bytes(), &mut out);
    out
}

/// The field-leaf key of `field` of entry `key` under `root`.
pub(super) fn field_leaf_key(root: &str, key: &KeyScalar, field: &str) -> Vec<u8> {
    stem_field_leaf(&marker_key(root, key), field)
}

/// The iteration/subtree cursor of the entry whose marker `stem` is given: the stem
/// with its trailing marker terminator replaced by the cursor sentinel. It sorts
/// after every cell the entry owns — its field leaves and its whole branch subtree —
/// and before the next sibling's marker, so one prefix-successor seek past it skips
/// the entry's subtree regardless of branch fan-out.
pub(super) fn stem_cursor(stem: &[u8]) -> Vec<u8> {
    debug_assert_eq!(
        stem.last(),
        Some(&MARKER_TERMINATOR),
        "a marker stem ends in the marker terminator",
    );
    let mut out = stem.to_vec();
    if let Some(last) = out.last_mut() {
        *last = CURSOR_SENTINEL;
    }
    out
}

/// The iteration cursor that resumes a forward scan after every cell of entry `key`
/// under `root`.
pub(super) fn cursor(root: &str, key: &KeyScalar) -> Vec<u8> {
    stem_cursor(&marker_key(root, key))
}

/// A meta cell key in the `0x10` family.
pub(super) fn meta_key(name: &str) -> Vec<u8> {
    let mut out = vec![META_FAMILY];
    encode_escaped_bytes(name.as_bytes(), &mut out);
    out
}

/// Classify a cell key found at or after an iteration cursor, relative to `root`'s
/// entry family. A well-formed marker yields its key; a branch descendant reached
/// where a marker would begin identifies a descendant-only entry (a node with
/// children but no payload); a field leaf reached there is an orphan (a marker/field
/// mismatch — corruption); a cell outside the family ends iteration.
pub(super) enum CellKind {
    /// An entry marker: the decoded root-level key.
    Marker(KeyScalar),
    /// A branch descendant of the root-level entry `key` whose own payload marker is
    /// absent — a descendant-only node. Iteration seeks past the entry's cursor to
    /// skip its whole subtree: the node holds children but no visitable payload.
    Descendant(KeyScalar),
    /// A field leaf sitting where a marker must be — a marker/field mismatch
    /// (corruption).
    Orphan,
    /// A cell outside this root's entry family: iteration is done.
    Foreign,
}

/// Decode a scanned cell key relative to `root`'s entry family: an entry marker, a
/// branch descendant of a markerless (descendant-only) entry, an orphan field leaf,
/// or foreign. The structural tag immediately after the entry key distinguishes
/// them — a lone marker terminator is the marker, a terminator then the branch tag a
/// descendant, and a terminator then the field tag (or any other shape) an orphan.
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
        // terminator then a branch tag: a branch descendant of a markerless entry.
        [MARKER_TERMINATOR, BRANCH_TAG, ..] => CellKind::Descendant(key),
        // terminator then a field tag, or any other shape: a field leaf where a
        // marker belongs — corruption.
        _ => CellKind::Orphan,
    }
}

/// What a cell sorting strictly after a node's marker `stem`, under the stem's own
/// prefix, is: one of the node's own field leaves (`stem 0x10 …`), a cell of one of
/// its branch descendants (`stem 0x30 …`), or foreign (not under the stem — which
/// the bounded probe's prefix bound already excludes). The probe reads the first
/// such cell to tell a descendant-only node (a branch descendant with no marker)
/// from an orphan (an own field leaf with no marker).
pub(super) enum BelowMarker {
    OwnField,
    BranchDescendant,
    Foreign,
}

/// Classify a cell sitting strictly after the marker `stem`, relative to that stem.
/// The structural tag immediately after the stem distinguishes an own field leaf
/// from a branch descendant.
pub(super) fn below_marker(stem: &[u8], cell_key: &[u8]) -> BelowMarker {
    match cell_key.strip_prefix(stem) {
        Some([FIELD_TAG, ..]) => BelowMarker::OwnField,
        Some([BRANCH_TAG, ..]) => BelowMarker::BranchDescendant,
        _ => BelowMarker::Foreign,
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

    /// Build a branch child's marker stem the way the recursive layout prescribes:
    /// the parent's marker stem, the branch tag, the escaped branch name, the child
    /// key, and a marker terminator. Slice 2 promotes this into a `pub(super)`
    /// builder the planner consumes; here it pins the layout recipe the ordering
    /// laws below rest on, so the two never drift.
    fn branch_child_marker(parent_stem: &[u8], branch: &str, child_key: &KeyScalar) -> Vec<u8> {
        let mut out = parent_stem.to_vec();
        out.push(BRANCH_TAG);
        encode_escaped_bytes(branch.as_bytes(), &mut out);
        out.extend_from_slice(&encode_key_value(child_key));
        out.push(MARKER_TERMINATOR);
        out
    }

    /// A spread of parent keys whose escaped encodings are prefix-related, so the
    /// containment and separation laws are exercised where they are hardest.
    fn representative_keys() -> Vec<KeyScalar> {
        vec![
            KeyScalar::Int(i64::MIN),
            KeyScalar::Int(-1),
            KeyScalar::Int(0),
            KeyScalar::Int(i64::MAX),
            KeyScalar::Str(String::new()),
            KeyScalar::Str("a".into()),
            KeyScalar::Str("a\u{0}".into()),
            KeyScalar::Str("ab".into()),
            KeyScalar::Bytes(vec![0x00, 0xff]),
            KeyScalar::Bool(true),
        ]
    }

    /// Every cell a branch descendant can occupy — its marker, a field leaf on it, a
    /// nested sub-branch marker, and its cursor — sorts strictly inside the parent
    /// root entry's `(marker(parent), cursor(parent))` range, for every
    /// representative parent key. This is the recursive containment law: a whole
    /// subtree lives under one entry's marker and below its cursor.
    #[test]
    fn branch_descendants_nest_inside_the_parent_entry_range() {
        let root = "books";
        for parent in representative_keys() {
            let parent_marker = marker_key(root, &parent);
            let parent_cursor = cursor(root, &parent);
            let child = branch_child_marker(&parent_marker, "notes", &KeyScalar::Int(7));
            let child_field = stem_field_leaf(&child, "text");
            let child_cursor = stem_cursor(&child);
            let grandchild = branch_child_marker(&child, "tags", &KeyScalar::Str("x".into()));
            for cell in [&child, &child_field, &child_cursor, &grandchild] {
                assert!(
                    parent_marker.as_slice() < cell.as_slice(),
                    "descendant sorts after the parent marker for {parent:?}",
                );
                assert!(
                    cell.as_slice() < parent_cursor.as_slice(),
                    "descendant sorts before the parent cursor for {parent:?}",
                );
            }
        }
    }

    /// A parent entry's own field leaves sort ahead of its branch descendants, so a
    /// forward scan of the entry's cells meets an orphan own-leaf before any
    /// descendant — the precedence the bounded prefix probe relies on.
    #[test]
    fn own_field_leaves_sort_before_branch_descendants() {
        let root = "books";
        let parent = marker_key(root, &KeyScalar::Str("a".into()));
        let own_field = stem_field_leaf(&parent, "title");
        let branch_child = branch_child_marker(&parent, "notes", &KeyScalar::Int(1));
        assert!(
            parent.as_slice() < own_field.as_slice(),
            "marker precedes own field"
        );
        assert!(
            own_field.as_slice() < branch_child.as_slice(),
            "own field precedes branch descendants",
        );
    }

    /// The parent cursor sorts after the parent's whole subtree (own fields and every
    /// branch descendant) and before the next root sibling's marker, so one seek past
    /// the cursor skips the subtree regardless of branch fan-out.
    #[test]
    fn parent_cursor_skips_the_whole_subtree_and_precedes_the_next_sibling() {
        let root = "books";
        let a = marker_key(root, &KeyScalar::Str("a".into()));
        let a_cursor = cursor(root, &KeyScalar::Str("a".into()));
        let b_marker = marker_key(root, &KeyScalar::Str("b".into()));
        let subtree = [
            stem_field_leaf(&a, "title"),
            branch_child_marker(&a, "notes", &KeyScalar::Int(i64::MIN)),
            branch_child_marker(&a, "notes", &KeyScalar::Int(i64::MAX)),
            stem_field_leaf(
                &branch_child_marker(&a, "notes", &KeyScalar::Int(1)),
                "text",
            ),
        ];
        for cell in &subtree {
            assert!(
                cell.as_slice() < a_cursor.as_slice(),
                "cell precedes the cursor"
            );
        }
        assert!(
            a_cursor.as_slice() < b_marker.as_slice(),
            "cursor precedes the next sibling"
        );
    }

    /// The recursion holds at the branch level: within one branch, a child's whole
    /// footprint sorts below its own cursor, which sorts below the next child's
    /// marker — the same separation `cursor_separates_adjacent_and_prefix_related_keys`
    /// proves at the root, one level down.
    #[test]
    fn branch_children_are_separated_by_their_own_cursor() {
        let root = "books";
        let parent = marker_key(root, &KeyScalar::Str("a".into()));
        let mut children = representative_keys();
        children.sort();
        for pair in children.windows(2) {
            let lo = branch_child_marker(&parent, "notes", &pair[0]);
            let lo_field = stem_field_leaf(&lo, "text");
            let lo_cursor = stem_cursor(&lo);
            let hi = branch_child_marker(&parent, "notes", &pair[1]);
            assert!(
                lo.as_slice() < lo_field.as_slice(),
                "child marker precedes its field"
            );
            assert!(
                lo_field.as_slice() < lo_cursor.as_slice(),
                "child field precedes its cursor"
            );
            assert!(
                lo_cursor.as_slice() < hi.as_slice(),
                "child cursor precedes the next child marker: {:?} < {:?}",
                pair[0],
                pair[1],
            );
        }
    }

    /// A branch descendant reached where a root entry marker would begin classifies
    /// as a descendant of that entry's key (so iteration skips a markerless
    /// descendant-only node), while a marker classifies as present and an own field
    /// leaf as an orphan. A deep sub-branch cell still classifies against the
    /// root-level key it descends from.
    #[test]
    fn classify_recognizes_a_branch_descendant() {
        let root = "books";
        let key = KeyScalar::Str("a".into());
        let parent = marker_key(root, &key);
        let child = branch_child_marker(&parent, "notes", &KeyScalar::Int(1));
        let grandchild = branch_child_marker(&child, "tags", &KeyScalar::Int(2));
        assert!(matches!(
            classify_cell(root, &child),
            CellKind::Descendant(k) if k == key
        ));
        assert!(
            matches!(classify_cell(root, &grandchild), CellKind::Descendant(k) if k == key),
            "a deep descendant classifies against its root-level key",
        );
        assert!(matches!(
            classify_cell(root, &parent),
            CellKind::Marker(k) if k == key
        ));
        assert!(matches!(
            classify_cell(root, &stem_field_leaf(&parent, "title")),
            CellKind::Orphan
        ));
    }
}
