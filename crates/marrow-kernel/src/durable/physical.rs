//! Physical cells keyed by store-local, never-reused node numbers.
//!
//! Each root or branch declaration has its own static entry family. Its marker
//! contains the full ancestor-and-own key tuple, whose arity and kinds come from
//! that family's schema. Children occupy separate families even when ancestor
//! payloads are absent. Fields and groups belong only to their containing entry.
//!
//! ```text
//! entry family prefix  0x01 0x20 num(family)
//! marker key           <family> enc(fullKeyTuple) 0x00                value = 0x01
//! field leaf           <marker> 0x10 num(field)                      value = codec bytes
//! group leaf           <marker> 0x28 num(group) 0x10 num(field)       value = codec bytes
//! entry cursor         <family> enc(fullKeyTuple) 0xFF
//! index cell           0x02 num(root) index_id[16] enc(projection)    value = enc(sourceKey)
//! meta cell            0x10 esc(name)
//! ```
//!
//! Numbers are fixed-width big-endian u32s. Ordered key columns self-delimit;
//! the schema fixes the number of ancestor and own columns. Holding encoded
//! ancestors narrows one family's prefix to the immediate entries at that path.
//! A marker precedes its own payload, and the cursor sorts after that payload but
//! before the next entry. Other families cannot appear in this range, so a step
//! either finds a marker, encounters orphan payload, or reaches the range's end.
//!
//! A group is an unkeyed payload namespace with no marker of its own. Indexes and
//! metadata remain separate cell families. Only the closed metadata namespace
//! uses names; source spelling never enters an entry, group, branch or index key.

use crate::codec::key::{
    KeyScalar, decode_key_value, encode_escaped_bytes, encode_key_tuple, encode_key_value,
};

/// A store-local number shared by the root, field, group and branch node space.
pub(super) type NodeNumber = u32;

/// The canonical fixed-width number encoding, independent of source spelling.
fn push_component(out: &mut Vec<u8>, number: NodeNumber) {
    out.extend_from_slice(&number.to_be_bytes());
}

const ENTRY_FAMILY: u8 = 0x01;
const ENTRY_TAG: u8 = 0x20;
const FIELD_TAG: u8 = 0x10;
const GROUP_TAG: u8 = 0x28;
const MARKER_TERMINATOR: u8 = 0x00;
const CURSOR_SENTINEL: u8 = 0xFF;
const META_FAMILY: u8 = 0x10;
const INDEX_FAMILY: u8 = 0x02;

/// The payload-presence record, checked by complete logical inspection.
pub(super) const MARKER_VALUE: &[u8] = &[0x01];

/// Every entry cell of one root or branch declaration shares this prefix.
pub(super) fn entry_family_prefix(family: NodeNumber) -> Vec<u8> {
    let mut out = vec![ENTRY_FAMILY, ENTRY_TAG];
    push_component(&mut out, family);
    out
}

/// An entry's marker stem, using all declared ancestor and own key columns.
pub(super) fn marker_key(family: NodeNumber, keys: &[KeyScalar]) -> Vec<u8> {
    child_marker(entry_family_prefix(family), keys)
}

/// One own field leaf, derived identically for root and branch entries.
pub(super) fn stem_field_leaf(stem: &[u8], field: NodeNumber) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(FIELD_TAG);
    push_component(&mut out, field);
    out
}

/// The entry's own field range, excluding groups and every other entry family.
pub(super) fn field_leaf_range(stem: &[u8]) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(FIELD_TAG);
    out
}

/// A cursor after this entry's own payload and before the next entry in its family.
fn stem_cursor(mut stem: Vec<u8>) -> Vec<u8> {
    debug_assert_eq!(
        stem.last(),
        Some(&MARKER_TERMINATOR),
        "a marker stem ends in the marker terminator",
    );
    if let Some(last) = stem.last_mut() {
        *last = CURSOR_SENTINEL;
    }
    stem
}

/// One group's own leaves. A group carries its containing entry's presence and
/// has neither a key nor a marker; its field leaves extend this prefix.
pub(super) fn group_stem(stem: &[u8], group: NodeNumber) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(GROUP_TAG);
    push_component(&mut out, group);
    out
}

/// Complete the fixed family/ancestor prefix with the remaining key columns.
fn child_marker(mut out: Vec<u8>, keys: &[KeyScalar]) -> Vec<u8> {
    out.extend_from_slice(&encode_key_tuple(keys));
    out.push(MARKER_TERMINATOR);
    out
}

/// A meta cell key in the `0x10` family.
pub(super) fn meta_key(name: &str) -> Vec<u8> {
    let mut out = vec![META_FAMILY];
    encode_escaped_bytes(name.as_bytes(), &mut out);
    out
}

/// The physical cell key of one managed-index row: the index family byte, the root's
/// number, the index's 16-byte identity, and the prefix-free encoding of its ordered
/// projected component values. The single owner of the index cell key shape — the
/// consequence planner builds every index write and removal through it, so no second site
/// spells an index cell. Because the root number is fixed width, the identity is fixed
/// width, and the projected-value encoding is prefix-free, one index's cells occupy a
/// distinct, self-delimited key range under the root.
pub(super) fn index_cell_key(
    root: NodeNumber,
    index_id: &[u8; 16],
    projected: &[KeyScalar],
) -> Vec<u8> {
    let mut out = vec![INDEX_FAMILY];
    push_component(&mut out, root);
    out.extend_from_slice(index_id);
    out.extend_from_slice(&encode_key_tuple(projected));
    out
}

/// The value stored at a managed-index cell: the prefix-free encoding of the source
/// entry's key tuple — the `Id(^root)` an index lookup or scan yields. Paired with
/// [`index_cell_key`] as the single owner of the index cell shape.
pub(super) fn index_cell_value(source_key: &[KeyScalar]) -> Vec<u8> {
    encode_key_tuple(source_key)
}

/// Decode the source key tuple stored at an index cell — the `arity` root key columns an
/// index read yields. The inverse of [`index_cell_value`] paired with it as the index
/// cell-value owner. `None` when the bytes do not decode as exactly `arity` leading key
/// values with no trailing bytes: a truncated, over-long, or undecodable value is a
/// corrupt cell, never a partial or extended source key.
pub(super) fn decode_index_source_key(bytes: &[u8], arity: usize) -> Option<Vec<KeyScalar>> {
    let mut out = Vec::with_capacity(arity);
    let mut rest = bytes;
    for _ in 0..arity {
        let (key, used) = decode_key_value(rest)?;
        out.push(key);
        rest = rest.get(used..)?;
    }
    rest.is_empty().then_some(out)
}

/// A scanned cell relative to one traversed family and fixed ancestor prefix.
pub(super) enum CellKind {
    /// A well-formed marker key; the caller checks its declared scalar domain.
    Marker(KeyScalar),
    /// Own payload or malformed key where a marker should be.
    Orphan,
    /// Outside the traversed prefix.
    Foreign,
}

fn classify_under_prefix(prefix: &[u8], cell_key: &[u8]) -> CellKind {
    let Some(rest) = cell_key.strip_prefix(prefix) else {
        return CellKind::Foreign;
    };
    let Some((key, used)) = decode_key_value(rest) else {
        return CellKind::Orphan;
    };
    if rest[used..] == [MARKER_TERMINATOR] {
        CellKind::Marker(key)
    } else {
        CellKind::Orphan
    }
}

/// Own payload shape below a canonical marker, shared with logical inspection.
pub(super) enum BelowMarker {
    OwnField,
    OwnGroup,
    Corrupt,
    Foreign,
}

pub(super) fn below_marker(stem: &[u8], cell_key: &[u8]) -> BelowMarker {
    match cell_key.strip_prefix(stem) {
        Some([FIELD_TAG, ..]) => BelowMarker::OwnField,
        Some([GROUP_TAG, ..]) => BelowMarker::OwnGroup,
        Some([_, ..]) => BelowMarker::Corrupt,
        _ => BelowMarker::Foreign,
    }
}

/// Immediate entries in one static family with fixed ancestor key columns.
/// Traversal admits one own key column; composite ancestors remain supported.
pub(super) struct Layer {
    prefix: Vec<u8>,
}

impl Layer {
    pub(super) fn new(family: NodeNumber, ancestor_keys: &[KeyScalar]) -> Self {
        let mut prefix = entry_family_prefix(family);
        prefix.extend_from_slice(&encode_key_tuple(ancestor_keys));
        Self { prefix }
    }

    pub(super) fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    /// The inclusive lower bound sorts below the matching marker because it
    /// omits the marker terminator; the engine's cursor itself is exclusive.
    pub(super) fn seek_from(&self, from: &KeyScalar) -> Vec<u8> {
        let mut out = self.prefix.clone();
        out.extend_from_slice(&encode_key_value(from));
        out
    }

    /// Skip one entry's own payload; other families are outside this prefix.
    pub(super) fn child_cursor(&self, key: &KeyScalar) -> Vec<u8> {
        stem_cursor(child_marker(self.prefix.clone(), std::slice::from_ref(key)))
    }

    pub(super) fn classify(&self, cell_key: &[u8]) -> CellKind {
        classify_under_prefix(&self.prefix, cell_key)
    }
}

/// One managed index's cell family narrowed to a fixed leading projection prefix: the
/// `0x02 num(root) index_id enc(fixed)` byte range a progressive-prefix scan traverses.
/// A cell of this index whose first `fixed.len()` projected components equal `fixed`
/// begins with this prefix and continues with the encoding of the next component
/// (an incomplete prefix) or nothing (the complete projection); a differently-prefixed
/// cell, another index's cell, or an entry/meta cell is foreign to it. The single owner
/// of index-scan cursor and next-component decoding, mirroring [`Layer`] for the index
/// family: the nonunique bounded scan steps forward through it exactly as the entry
/// traversal steps through a `Layer`.
pub(super) struct IndexLayer {
    prefix: Vec<u8>,
}

impl IndexLayer {
    /// The scan range of `index_id` under `root` with the leading components `fixed`
    /// held. `fixed` is a strict prefix of the index's ordered projection (fewer columns
    /// than the whole projection), so no full cell equals this prefix — every matching
    /// cell strictly extends it with at least the next component's encoding.
    pub(super) fn new(root: NodeNumber, index_id: &[u8; 16], fixed: &[KeyScalar]) -> Self {
        Self {
            prefix: index_cell_key(root, index_id, fixed),
        }
    }

    /// The byte prefix bounding the scan: every cell of this index sharing the fixed
    /// leading components starts with it, and a `scan_after` bounded by it stays inside
    /// the range.
    pub(super) fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    /// The inclusive-`from` seek start over the next component: `prefix ++ enc(from)`.
    /// At an incomplete prefix the matching cell continues with a following column tag
    /// (`>= 0x01`), so this cursor sorts strictly below it and a forward scan yields it;
    /// the complete-projection case (where a cell equals `prefix ++ enc(from)` exactly)
    /// is handled by the scan's own equality probe, since a bare `scan_after` excludes an
    /// equal cursor.
    pub(super) fn seek_from(&self, from: &KeyScalar) -> Vec<u8> {
        let mut out = self.prefix.clone();
        out.extend_from_slice(&encode_key_value(from));
        out
    }

    /// The cursor that resumes a forward scan strictly past every cell whose next
    /// component equals `component`: the component row key raised by the cursor
    /// sentinel. Because every real index cell that extends `prefix ++ enc(component)`
    /// continues with a key type tag (`0x01..=0x08`) — never `0xFF` — this sentinel
    /// sorts above every such cell and below the next distinct component's cells, and is
    /// never itself a real cell. One seek past it therefore skips a whole distinct
    /// component's rows regardless of how many share it, so a scan of `d` distinct
    /// component values costs `O(d + 1)` seeks independent of total row fan-out.
    pub(super) fn skip_cursor(&self, component: &KeyScalar) -> Vec<u8> {
        let mut out = self.prefix.clone();
        out.extend_from_slice(&encode_key_value(component));
        out.push(CURSOR_SENTINEL);
        out
    }

    /// Decode the next projected component of a cell scanned under this prefix: the one
    /// key value immediately following the fixed leading prefix. `None` when the cell is
    /// not under the prefix (foreign — the scan is done) or the following bytes do not
    /// decode as a leading key value (corruption).
    pub(super) fn next_component(&self, cell_key: &[u8]) -> Option<KeyScalar> {
        let rest = cell_key.strip_prefix(self.prefix.as_slice())?;
        let (component, _used) = decode_key_value(rest)?;
        Some(component)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Distinct cell-key numbers standing in for the durable nodes these layout tests
    // exercise. The tests assert byte ordering, containment, and classification, which
    // depend only on the numbers being distinct and on the structural tag bytes — not on
    // any particular value — so arbitrary distinct numbers suffice in place of the former
    // node names.
    const ROOT_COUNTERS: NodeNumber = 0;
    const ROOT_BOOKS: NodeNumber = 1;
    const ROOT_CELLS: NodeNumber = 2;
    const ROOT_TOMES: NodeNumber = 3;
    const ROOT_STOCK: NodeNumber = 4;
    const F_VALUE: NodeNumber = 10;
    const F_LABEL: NodeNumber = 11;
    const F_TITLE: NodeNumber = 12;
    const F_TEXT: NodeNumber = 13;
    const F_PAGES: NodeNumber = 14;
    const F_LANGUAGE: NodeNumber = 15;
    const B_NOTES: NodeNumber = 20;
    const B_TAGS: NodeNumber = 21;
    const B_SPANS: NodeNumber = 22;
    const G_DETAILS: NodeNumber = 30;
    const G_CREDITS: NodeNumber = 31;

    /// A single-column marker key: the layout tests below exercise the single-column
    /// case, so `mk` wraps the one key column as a one-element tuple. Composite-tuple
    /// containment and separation have their own test.
    fn mk(root: NodeNumber, key: &KeyScalar) -> Vec<u8> {
        marker_key(root, std::slice::from_ref(key))
    }

    fn bcs(ancestors: &[KeyScalar], branch: NodeNumber, key: &KeyScalar) -> Vec<u8> {
        let mut keys = ancestors.to_vec();
        keys.push(key.clone());
        marker_key(branch, &keys)
    }

    /// A root entry's own-payload cursor, the convenience over [`Layer::child_cursor`]
    /// the ordering tests assert against.
    fn cursor(root: NodeNumber, key: &KeyScalar) -> Vec<u8> {
        Layer::new(root, &[]).child_cursor(key)
    }

    /// Classify a cell against a root's entry family, the root convenience over
    /// [`Layer::classify`] the classification tests assert against.
    fn classify_cell(root: NodeNumber, cell_key: &[u8]) -> CellKind {
        Layer::new(root, &[]).classify(cell_key)
    }

    // The ordering property iteration relies on: for keys k < k',
    // marker(k) < every cell of k < cursor(k) < marker(k').
    fn assert_between(root: NodeNumber, key: &KeyScalar, fields: &[NodeNumber]) {
        let marker = mk(root, key);
        let cur = cursor(root, key);
        assert!(marker < cur, "marker precedes cursor for {key:?}");
        for &field in fields {
            let leaf = stem_field_leaf(&mk(root, key), field);
            assert!(marker < leaf, "marker precedes leaf {field} for {key:?}");
            assert!(leaf < cur, "leaf {field} precedes cursor for {key:?}");
        }
    }

    #[test]
    fn cursor_separates_adjacent_and_prefix_related_keys() {
        let root = ROOT_COUNTERS;
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
            assert_between(root, key, &[F_VALUE, F_LABEL]);
        }
        // Between consecutive keys, cursor(k) < marker(k').
        for pair in sorted.windows(2) {
            let cur = cursor(root, &pair[0]);
            let next_marker = mk(root, &pair[1]);
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
        let root = ROOT_COUNTERS;
        let key = KeyScalar::Str("a".into());
        assert!(matches!(
            classify_cell(root, &mk(root, &key)),
            CellKind::Marker(k) if k == key
        ));
        assert!(matches!(
            classify_cell(root, &stem_field_leaf(&mk(root, &key), F_VALUE)),
            CellKind::Orphan
        ));
        assert!(matches!(
            classify_cell(root, &meta_key("witness")),
            CellKind::Foreign
        ));
    }

    #[test]
    fn meta_family_is_disjoint_from_entries() {
        let root = ROOT_COUNTERS;
        let entry = mk(root, &KeyScalar::Int(0));
        let meta = meta_key("profile");
        assert_ne!(entry.first(), meta.first());
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

    #[test]
    fn branch_families_are_disjoint_from_parent_payload_ranges() {
        for parent in representative_keys() {
            let parent_marker = mk(ROOT_BOOKS, &parent);
            let parent_cursor = cursor(ROOT_BOOKS, &parent);
            let own_field = stem_field_leaf(&parent_marker, F_TITLE);
            assert!(parent_marker < own_field && own_field < parent_cursor);
            let child = bcs(std::slice::from_ref(&parent), B_NOTES, &KeyScalar::Int(7));
            let child_field = stem_field_leaf(&child, F_TEXT);
            let child_cursor = stem_cursor(child.clone());
            let grandchild = bcs(
                &[parent.clone(), KeyScalar::Int(7)],
                B_TAGS,
                &KeyScalar::Str("x".into()),
            );
            for cell in [&child, &child_field, &child_cursor, &grandchild] {
                assert!(!cell.starts_with(&entry_family_prefix(ROOT_BOOKS)));
                assert!(
                    cell.as_slice() < parent_marker.as_slice()
                        || cell.as_slice() > parent_cursor.as_slice()
                );
            }
            assert!(!grandchild.starts_with(&entry_family_prefix(B_NOTES)));
        }
    }

    /// A group leaf of `group` and field `field` of the entry keyed `key`: the
    /// entry-marker-stem, the group prefix, then the field leaf, through the shared
    /// [`stem_field_leaf`] owner one namespace level down.
    fn group_leaf(
        root: NodeNumber,
        key: &KeyScalar,
        group: NodeNumber,
        field: NodeNumber,
    ) -> Vec<u8> {
        stem_field_leaf(&group_stem(&mk(root, key), group), field)
    }

    #[test]
    fn group_leaves_follow_own_fields_inside_the_entry_range() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let marker = mk(root, &key);
        let cur = cursor(root, &key);
        let own_field = stem_field_leaf(&marker, F_TITLE);
        let group_leaf = group_leaf(root, &key, G_DETAILS, F_PAGES);
        assert!(marker < own_field);
        assert!(own_field < group_leaf);
        assert!(group_leaf < cur);
    }

    /// Every cell a group can occupy — each of its leaves, over representative parent
    /// keys — nests strictly inside the entry's `(marker, cursor)` range, so a group is
    /// part of exactly one entry's own payload and one seek past the cursor skips it.
    #[test]
    fn group_leaves_nest_inside_the_entry_range() {
        let root = ROOT_BOOKS;
        for key in representative_keys() {
            let marker = mk(root, &key);
            let cur = cursor(root, &key);
            for field in [F_PAGES, F_LANGUAGE] {
                let leaf = group_leaf(root, &key, G_DETAILS, field);
                assert!(
                    marker.as_slice() < leaf.as_slice(),
                    "group leaf sorts after the marker for {key:?}",
                );
                assert!(
                    leaf.as_slice() < cur.as_slice(),
                    "group leaf sorts before the cursor for {key:?}",
                );
            }
        }
    }

    /// A group's leaves occupy a byte range disjoint from the entry's top-level field
    /// leaves, from a differently-named sibling group's leaves, and from the entry's
    /// branches: a group write confined to `<marker> 0x28 num(group)` never aliases any
    /// of them. The `group_stem` prefix bounds one group's cells and no other's.
    #[test]
    fn group_leaves_are_disjoint_from_fields_sibling_groups_and_branches() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let marker = mk(root, &key);
        let details = group_stem(&marker, G_DETAILS);
        let details_leaf = stem_field_leaf(&details, F_PAGES);
        // A top-level field named identically to a group leaf's field is a distinct cell.
        let top_field = stem_field_leaf(&marker, F_PAGES);
        assert!(
            !details_leaf.starts_with(&marker_field_prefix(&marker)),
            "a group leaf is not under the top-level field tag"
        );
        assert_ne!(details_leaf, top_field, "group leaf ≠ top-level field leaf");
        assert!(
            !top_field.starts_with(&details),
            "a top-level field leaf is outside the group prefix"
        );
        // A sibling group's leaves are outside this group's prefix, and vice versa.
        let credits = group_stem(&marker, G_CREDITS);
        let credits_leaf = stem_field_leaf(&credits, F_PAGES);
        assert!(
            !credits_leaf.starts_with(&details),
            "a sibling group's leaf is outside this group's prefix"
        );
        assert!(
            !details_leaf.starts_with(&credits),
            "this group's leaf is outside the sibling group's prefix"
        );
        let branch_child = bcs(std::slice::from_ref(&key), B_NOTES, &KeyScalar::Int(1));
        assert!(
            !branch_child.starts_with(&details),
            "a branch cell is outside the group prefix"
        );
    }

    /// The field tag prefix of an entry's own field-leaf namespace: the marker followed
    /// by the field tag. A group leaf must not fall under it (a group leaf's first
    /// post-marker byte is the group tag, not the field tag).
    fn marker_field_prefix(marker: &[u8]) -> Vec<u8> {
        let mut out = marker.to_vec();
        out.push(FIELD_TAG);
        out
    }

    /// The layer rejects a markerless group leaf; the payload classifier identifies
    /// the same cell as a group leaf for complete inspection.
    #[test]
    fn a_markerless_group_leaf_is_an_orphan_on_both_paths() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let stem = mk(root, &key);
        let leaf = group_leaf(root, &key, G_DETAILS, F_PAGES);
        assert!(matches!(classify_cell(root, &leaf), CellKind::Orphan));
        assert!(matches!(below_marker(&stem, &leaf), BelowMarker::OwnGroup));
    }

    #[test]
    fn entry_cursor_skips_own_payload_and_precedes_the_next_sibling() {
        let a = mk(ROOT_BOOKS, &KeyScalar::Str("a".into()));
        let a_cursor = cursor(ROOT_BOOKS, &KeyScalar::Str("a".into()));
        let b_marker = mk(ROOT_BOOKS, &KeyScalar::Str("b".into()));
        for cell in [
            stem_field_leaf(&a, F_TITLE),
            stem_field_leaf(&group_stem(&a, G_DETAILS), F_PAGES),
        ] {
            assert!(a < cell && cell < a_cursor);
        }
        assert!(a_cursor < b_marker);
    }

    /// The same ordering law holds in a branch family: an entry's own payload
    /// sorts below its cursor, which sorts below the next entry's marker.
    #[test]
    fn branch_children_are_separated_by_their_own_cursor() {
        let parent = [KeyScalar::Str("a".into())];
        let mut children = representative_keys();
        children.sort();
        for pair in children.windows(2) {
            let lo = bcs(&parent, B_NOTES, &pair[0]);
            let lo_field = stem_field_leaf(&lo, F_TEXT);
            let lo_cursor = stem_cursor(lo.clone());
            let hi = bcs(&parent, B_NOTES, &pair[1]);
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

    #[test]
    fn branch_cells_are_foreign_to_parent_families() {
        let key = KeyScalar::Str("a".into());
        let parent = mk(ROOT_BOOKS, &key);
        let child = bcs(std::slice::from_ref(&key), B_NOTES, &KeyScalar::Int(1));
        let grandchild = bcs(
            &[key.clone(), KeyScalar::Int(1)],
            B_TAGS,
            &KeyScalar::Int(2),
        );
        assert!(matches!(
            classify_cell(ROOT_BOOKS, &child),
            CellKind::Foreign
        ));
        assert!(matches!(
            classify_cell(ROOT_BOOKS, &grandchild),
            CellKind::Foreign
        ));
        let child_layer = Layer::new(B_NOTES, std::slice::from_ref(&key));
        assert!(matches!(
            child_layer.classify(&child),
            CellKind::Marker(KeyScalar::Int(1))
        ));
        assert!(matches!(
            child_layer.classify(&grandchild),
            CellKind::Foreign
        ));
        assert!(matches!(classify_cell(ROOT_BOOKS, &parent), CellKind::Marker(k) if k == key));
        assert!(matches!(
            classify_cell(ROOT_BOOKS, &stem_field_leaf(&parent, F_TITLE)),
            CellKind::Orphan
        ));
    }

    /// An unknown payload tag is corruption in both physical classifiers.
    #[test]
    fn an_unknown_post_stem_tag_is_corruption_on_both_paths() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let stem = mk(root, &key);
        // A tag the layout never emits.
        let mut rogue = stem.clone();
        rogue.push(0x40);
        rogue.extend_from_slice(b"junk");
        assert!(matches!(below_marker(&stem, &rogue), BelowMarker::Corrupt));
        assert!(matches!(classify_cell(root, &rogue), CellKind::Orphan));
    }

    /// Full tuples with escaped later columns remain separated. A child's family
    /// prefix fixes every ancestor column without including the parent's payload.
    #[test]
    fn composite_key_markers_are_contained_and_separated_across_column_boundaries() {
        let root = ROOT_CELLS;
        // Column 0 shared, column 1 differs — including a trailing NUL that abuts the
        // marker terminator. Column-major order is a < a\0 < b in column 1.
        let a = &[KeyScalar::Int(1), KeyScalar::Str("a".into())][..];
        let a_nul = &[KeyScalar::Int(1), KeyScalar::Str("a\u{0}".into())][..];
        let b = &[KeyScalar::Int(1), KeyScalar::Str("b".into())][..];
        let mut tuples = [a, a_nul, b];
        tuples.sort();
        for pair in tuples.windows(2) {
            let lo = marker_key(root, pair[0]);
            let lo_cursor = stem_cursor(marker_key(root, pair[0]));
            let hi = marker_key(root, pair[1]);
            // Neither marker is a prefix of the other (prefix-free tuples).
            assert!(
                !hi.starts_with(&lo),
                "a composite marker is a prefix of a sibling"
            );
            let lo_field = stem_field_leaf(&lo, F_VALUE);
            assert!(lo < lo_field && lo_field < lo_cursor);
            let mut child_keys = pair[0].to_vec();
            child_keys.extend([KeyScalar::Int(9), KeyScalar::Bytes(vec![0x00, 0x00])]);
            let lo_branch = marker_key(B_SPANS, &child_keys);
            assert!(!lo_branch.starts_with(&entry_family_prefix(root)));
            assert!(lo_branch.starts_with(Layer::new(B_SPANS, pair[0]).prefix()));
            assert!(!lo_branch.starts_with(Layer::new(B_SPANS, pair[1]).prefix()));
            assert!(
                lo_cursor.as_slice() < hi.as_slice(),
                "a composite entry's cursor precedes the next sibling's marker"
            );
        }
    }

    const IDX_A: [u8; 16] = [0x70; 16];
    const IDX_B: [u8; 16] = [0x71; 16];

    /// An index cell begins with the index family byte, disjoint from the entry and meta
    /// families, so an index cell never aliases an entry marker/leaf or a meta cell.
    #[test]
    fn index_cells_are_their_own_family() {
        let key = index_cell_key(ROOT_BOOKS, &IDX_A, &[KeyScalar::Str("a".into())]);
        assert_eq!(key.first(), Some(&INDEX_FAMILY));
        let entry = marker_key(ROOT_BOOKS, &[KeyScalar::Int(1)]);
        let meta = meta_key("profile");
        assert_ne!(key.first(), entry.first(), "disjoint from the entry family");
        assert_ne!(key.first(), meta.first(), "disjoint from the meta family");
    }

    /// One index's cells are separated from another's, and from a different root's, by the
    /// index identity and the escaped root name; the same identity, root, and projected
    /// values are deterministic.
    #[test]
    fn index_cell_keys_separate_by_identity_root_and_values() {
        let proj = [KeyScalar::Str("a".into()), KeyScalar::Int(1)];
        let base = index_cell_key(ROOT_BOOKS, &IDX_A, &proj);
        assert_eq!(
            base,
            index_cell_key(ROOT_BOOKS, &IDX_A, &proj),
            "deterministic"
        );
        assert_ne!(
            base,
            index_cell_key(ROOT_BOOKS, &IDX_B, &proj),
            "distinct index id"
        );
        assert_ne!(
            base,
            index_cell_key(ROOT_TOMES, &IDX_A, &proj),
            "distinct root"
        );
        assert_ne!(
            base,
            index_cell_key(
                ROOT_BOOKS,
                &IDX_A,
                &[KeyScalar::Str("a".into()), KeyScalar::Int(2)],
            ),
            "distinct projected values",
        );
    }

    /// The projected-value encoding is prefix-free and column-major: two rows differing in
    /// a later component order correctly with neither key a prefix of the other, and a
    /// leading-component projection is a byte-prefix of the full key — the bound a
    /// progressive-prefix scan seeks over.
    #[test]
    fn index_cell_keys_are_prefix_free_and_prefix_bounded() {
        let a1 = index_cell_key(
            ROOT_BOOKS,
            &IDX_A,
            &[KeyScalar::Str("a".into()), KeyScalar::Int(1)],
        );
        let a2 = index_cell_key(
            ROOT_BOOKS,
            &IDX_A,
            &[KeyScalar::Str("a".into()), KeyScalar::Int(2)],
        );
        let ab = index_cell_key(
            ROOT_BOOKS,
            &IDX_A,
            &[KeyScalar::Str("ab".into()), KeyScalar::Int(1)],
        );
        assert!(a1 < a2, "later component orders the rows");
        assert!(!a2.starts_with(&a1), "no row key is a prefix of a sibling");
        assert!(
            !ab.starts_with(&a1),
            "a longer leading column does not prefix-alias"
        );

        // The leading-component projection is a byte-prefix of every full key sharing it,
        // so a scan over `shelf = "a"` seeks that prefix and meets a1 then a2.
        let a_prefix = index_cell_key(ROOT_BOOKS, &IDX_A, &[KeyScalar::Str("a".into())]);
        assert!(a1.starts_with(&a_prefix) && a2.starts_with(&a_prefix));
        assert!(
            !ab.starts_with(&a_prefix),
            "shelf=\"ab\" is outside the shelf=\"a\" prefix",
        );
    }

    /// An index cell's value is the encoded source key tuple — the `Id(^root)` a read
    /// yields — through the shared key-tuple codec.
    #[test]
    fn index_cell_value_is_the_encoded_source_key() {
        let source = [KeyScalar::Int(42)];
        assert_eq!(index_cell_value(&source), encode_key_tuple(&source));
    }

    /// The index scan's skip cursor for a distinct component value sorts strictly above
    /// every cell that shares that component — whether the cell is the complete
    /// projection (the component is the last column, so the cell equals the component
    /// row key) or an incomplete prefix (the cell continues with a further column) — and
    /// strictly below the next distinct component's cells, and is never itself a real
    /// cell. This is the O(distinct + 1) traversal-skip law for the index family: one
    /// seek past the cursor passes a whole component's rows regardless of fan-out.
    #[test]
    fn index_skip_cursor_passes_one_component_and_stops_before_the_next() {
        // `byShelf[shelf, id]` held at `shelf = "a"`: enumerate the `id` component. The
        // `id` column is the last projected column, so a cell equals its component row key.
        let layer = IndexLayer::new(ROOT_BOOKS, &IDX_A, &[KeyScalar::Str("a".into())]);
        let cell_a1 = index_cell_key(
            ROOT_BOOKS,
            &IDX_A,
            &[KeyScalar::Str("a".into()), KeyScalar::Int(1)],
        );
        let cell_a2 = index_cell_key(
            ROOT_BOOKS,
            &IDX_A,
            &[KeyScalar::Str("a".into()), KeyScalar::Int(2)],
        );
        let cursor = layer.skip_cursor(&KeyScalar::Int(1));
        assert!(
            cell_a1 < cursor,
            "the component's own cell precedes its skip cursor"
        );
        assert!(
            cursor < cell_a2,
            "the skip cursor precedes the next distinct component"
        );
        assert_ne!(cursor, cell_a1, "the skip cursor is never a real cell");
        assert_eq!(
            layer.next_component(&cell_a1),
            Some(KeyScalar::Int(1)),
            "the next component decodes from the scanned cell",
        );

        // An incomplete prefix (a further column follows the enumerated component) obeys
        // the same law: the skip cursor still sits between the component's rows and the
        // next component. `byRegionShelfId[region, shelf, id]` held at `region = "west"`,
        // enumerating `shelf`, with two rows sharing `shelf = "a"`.
        let wide = IndexLayer::new(ROOT_STOCK, &IDX_B, &[KeyScalar::Str("west".into())]);
        let west_a_1 = index_cell_key(
            ROOT_STOCK,
            &IDX_B,
            &[
                KeyScalar::Str("west".into()),
                KeyScalar::Str("a".into()),
                KeyScalar::Int(1),
            ],
        );
        let west_a_2 = index_cell_key(
            ROOT_STOCK,
            &IDX_B,
            &[
                KeyScalar::Str("west".into()),
                KeyScalar::Str("a".into()),
                KeyScalar::Int(2),
            ],
        );
        let west_b_1 = index_cell_key(
            ROOT_STOCK,
            &IDX_B,
            &[
                KeyScalar::Str("west".into()),
                KeyScalar::Str("b".into()),
                KeyScalar::Int(1),
            ],
        );
        let cursor_a = wide.skip_cursor(&KeyScalar::Str("a".into()));
        assert!(
            west_a_1 < cursor_a && west_a_2 < cursor_a,
            "both a-rows precede the cursor"
        );
        assert!(cursor_a < west_b_1, "the cursor precedes the next shelf");
        assert_eq!(
            wide.next_component(&west_a_1),
            Some(KeyScalar::Str("a".into()))
        );
    }

    /// The absence gate of record (FR01 §3): no source spelling enters entry/group/branch/
    /// index cell-key construction. The escaped-name grammar (`encode_escaped_bytes`) and a
    /// `&str` node parameter survive in exactly one place — the meta family's `meta_key`, the
    /// sanctioned kernel-internal exception ("profile"/"witness"). Reverting any cell-key
    /// constructor to a name parameter would add a second `&str` function or a second
    /// escaped-name call and fail this gate.
    #[test]
    fn no_source_spelling_in_cell_keys() {
        let src = include_str!("physical.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production precedes the test module");
        assert_eq!(
            production.matches("encode_escaped_bytes(").count(),
            1,
            "the escaped-name grammar survives only in meta_key; a cell-key constructor \
             must never spell a name",
        );
        assert_eq!(
            production.matches(": &str").count(),
            1,
            "only meta_key takes a &str; every entry/group/branch/index cell-key \
             constructor takes a NodeNumber",
        );
        let meta = production.find("fn meta_key").expect("meta_key exists");
        let call = production
            .find("encode_escaped_bytes(")
            .expect("the one escaped-name call exists");
        assert!(
            call > meta,
            "the sole escaped-name call sits inside meta_key",
        );
    }

    /// Pin the current physical encoding of root/branch markers, field/group
    /// leaves and index cells so the grammar cannot drift silently.
    #[test]
    fn id_keyed_cell_key_layout_is_frozen() {
        let key = KeyScalar::Int(1);
        let enc_key = encode_key_value(&key); // 0x02 then 8 order-preserving bytes

        // Marker: 0x01 0x20 num(root=0) enc(key) 0x00
        let marker = marker_key(0, std::slice::from_ref(&key));
        let mut expected = vec![0x01, 0x20, 0x00, 0x00, 0x00, 0x00];
        expected.extend_from_slice(&enc_key);
        expected.push(0x00);
        assert_eq!(marker, expected, "marker layout");

        // Field leaf: <marker> 0x10 num(field=10)
        let leaf = stem_field_leaf(&marker, 10);
        let mut expected = marker.clone();
        expected.extend_from_slice(&[0x10, 0x00, 0x00, 0x00, 0x0A]);
        assert_eq!(leaf, expected, "field-leaf layout");

        // Group leaf: <marker> 0x28 num(group=30) 0x10 num(field=10)
        let group_leaf = stem_field_leaf(&group_stem(&marker, 30), 10);
        let mut expected = marker.clone();
        expected.extend_from_slice(&[0x28, 0x00, 0x00, 0x00, 0x1E, 0x10, 0x00, 0x00, 0x00, 0x0A]);
        assert_eq!(group_leaf, expected, "group-leaf layout");

        // Branch family, then root and child key columns, then marker terminator.
        let child = KeyScalar::Int(7);
        let branch = marker_key(20, &[key.clone(), child.clone()]);
        let mut expected = vec![0x01, 0x20, 0x00, 0x00, 0x00, 0x14];
        expected.extend_from_slice(&enc_key);
        expected.extend_from_slice(&encode_key_value(&child));
        expected.push(0x00);
        assert_eq!(branch, expected, "branch-child-marker layout");

        // Index cell key: 0x02 num(root=0) index_id[16] enc(projValues)
        let index = index_cell_key(0, &[0xAB; 16], std::slice::from_ref(&key));
        let mut expected = vec![0x02, 0x00, 0x00, 0x00, 0x00];
        expected.extend_from_slice(&[0xAB; 16]);
        expected.extend_from_slice(&encode_key_tuple(std::slice::from_ref(&key)));
        assert_eq!(index, expected, "index-cell-key layout");
    }
}
