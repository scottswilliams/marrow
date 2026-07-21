//! Physical cell layout for the durable store (design §G; FR01 §3 id-keyed cell keys).
//!
//! Durable cells are keyed by a durable node's compact **number** — a store-local
//! never-reused `u32` allocated for each root, field, group, and branch — never by its
//! source spelling (FR01 §3). A rename is therefore zero-cell metadata: the source anchor
//! moves, the node's identity and number are unchanged, and no cell is touched. Every
//! logical entry is a *marker* cell (its payload-presence record) plus one *field leaf* per
//! present field, and — for a hierarchical resource — one keyed *branch* family per declared
//! branch nested beneath the entry's marker. Each number component is a fixed-width 4-byte
//! big-endian integer, so it self-delimits trivially: the containment/separation laws below
//! depend only on each component's self-delimitation, never on what fills it, so a
//! fixed-width number satisfies them by construction:
//!
//! ```text
//! entry family prefix   0x01 0x20 num(root)
//! marker key            <family> enc(keytuple) 0x00                        value = 0x01
//! field leaf key        <marker> 0x10 num(field)                           value = codec bytes
//! group leaf key        <marker> 0x28 num(group) 0x10 num(field)           value = codec bytes
//! branch child marker   <marker> 0x30 num(branch) enc(childTuple) 0x00     value = 0x01
//! iteration cursor      <family> enc(keytuple) 0xFF
//! index cell key        0x02 num(root) index_id[16] enc(projValues)        value = enc(sourceKey)
//! meta cell key         0x10 esc(name)
//! ```
//!
//! `num(x)` is the node's 4-byte big-endian store-local number. The meta family is the sole
//! exception (FR01 §3): a closed, small, kernel-internal namespace (`profile`/`witness`
//! constants) that never renames and never scales with data, so it keeps its name-keyed
//! grammar. No source spelling ever enters an entry, group, branch, or index cell key —
//! the enforcement artifact of record is the absence gate at `no_source_spelling_in_cell_keys`.
//!
//! An unkeyed *group* is a static field-path namespace inside the entry's payload — not a
//! keyed node. It carries no marker and no key; its leaves are the entry's own payload
//! namespaced under the group number (`<marker> 0x28 num(group) 0x10 num(field)`), and its
//! presence is the entry's presence. The group tag `0x28` sorts between the field tag
//! `0x10` and the branch tag `0x30`, so an entry's own field leaves precede its group
//! leaves, which precede its branch descendants; every group leaf still nests inside the
//! entry's `(marker, cursor]` range. A group's whole read/replace/erase confine to the
//! group's own leaves under its `<marker> 0x28 num(group)` prefix, disjoint from the
//! entry's top-level fields, its sibling groups, and its branches.
//!
//! A managed index's cells form their own family (`0x02`), disjoint from the entry family
//! (`0x01`) and the meta family (`0x10`). One index's cells are separated from another's
//! under the same root (identified by the root's number) by the index's stable 16-byte
//! identity, so an index rename (which preserves that identity) never orphans its cells.
//! After the identity comes the
//! prefix-free encoding of the index's ordered projected component values; because that
//! encoding self-delimits every column, two index rows never share a key where one is a
//! prefix of the other, and a leading-component prefix is a valid scan bound. Each cell's
//! value is the encoded source key tuple — the `Id(^root)` a lookup or scan yields. A
//! non-unique index's projection ends with the identity suffix, so its rows are distinct
//! by construction; a unique index's projection omits it, so two rows with equal projected
//! values collide on one key (the uniqueness constraint the maintenance write enforces).
//!
//! `enc(keytuple)` is the ordered concatenation of the node's key columns, each column
//! encoded prefix-free (see [`encode_key_tuple`]); a single-column key is the one-column
//! case. Because the concatenation is itself prefix-free and sorts column-major, every
//! ordering property below holds column-wise exactly as for a single key.
//!
//! The layout is recursive: a branch child's marker (`<marker> 0x30 num(branch)
//! enc(childKey) 0x00`) is itself a marker stem, so the child's own field leaves,
//! nested branches, and iteration cursor derive from it exactly as a root entry's
//! do. An entry's marker is therefore a byte-prefix of every cell it owns — its
//! field leaves and its whole branch subtree — so the marker sorts first among the
//! entry's cells.
//!
//! Because `enc(keytuple)` is prefix-free (each column fixed width, or `0x00,0x00`-
//! terminated with `0x00,0x01` escapes, and the columns self-delimit) and the structural
//! tags ascend `0x00 < 0x10 < 0x28 < 0x30 < 0xFF`
//! (marker terminator, field, group, branch, cursor), every cell of entry `k` — including
//! every group leaf and every descendant in every branch — sorts inside
//! `(marker(k), cursor(k)]`, and no cell of another entry does. Two consequences the
//! kernel relies on: one prefix-successor seek past `cursor(k)` skips `k`'s whole subtree
//! regardless of branch fan-out (the traversal-skip law), and `k`'s own payload leaves —
//! its field leaves (`0x10`) then its group leaves (`0x28`) — sort ahead of its branch
//! descendants (`0x30`), so a scan of `k`'s cells meets an orphan own-payload leaf before
//! any descendant — the precedence the bounded prefix probe uses to surface a
//! marker/payload corruption ahead of a legitimate descendant-only node.

use crate::codec::key::{
    KeyScalar, decode_key_value, encode_escaped_bytes, encode_key_tuple, encode_key_value,
};

/// A durable node's store-local cell-key number: root, field, group, or branch. A
/// fixed-width `u32` (FR01 §3/§4) chosen for the store's lifetime headroom and independent
/// of the image's `u16` table rings; never reused within a store.
pub(super) type NodeNumber = u32;

/// Append a node's cell-key number in the canonical fixed-width 4-byte big-endian form.
/// Fixed width self-delimits, so a number component is prefix-free by construction and the
/// containment/separation laws hold because each component self-delimits. This is
/// the single owner of the number-to-key-bytes mapping: no source spelling ever reaches an
/// entry, group, branch, or index cell key.
fn push_component(out: &mut Vec<u8>, number: NodeNumber) {
    out.extend_from_slice(&number.to_be_bytes());
}

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
/// Separator introducing a group's own field namespace beneath an entry's marker stem.
/// A group is part of its containing entry's payload, not a keyed node: it carries no
/// marker and no key, and its presence is exactly its entry's presence. Sorts above
/// [`FIELD_TAG`] and below [`BRANCH_TAG`] — a distinct byte in the open `(0x10, 0x30)`
/// interval — so within an entry the own field leaves (`0x10`) precede the group leaves
/// (`0x28`), which precede the branch descendants (`0x30`): the field < group < branch
/// precedence the iteration classifier and the bounded prefix probe rely on. Its own
/// structural byte, disjoint from every other tag, so a stray group-tag literal is caught
/// by its own scan in the topology-owner gate.
const GROUP_TAG: u8 = 0x28;
/// Marker-stem terminator; sorts below [`FIELD_TAG`], so the marker precedes leaves.
const MARKER_TERMINATOR: u8 = 0x00;
/// Iteration-cursor sentinel; sorts above every cell of its entry.
const CURSOR_SENTINEL: u8 = 0xFF;
/// First byte of every meta cell (witness, profile). Disjoint from [`ENTRY_FAMILY`].
const META_FAMILY: u8 = 0x10;
/// First byte of every managed-index cell. Disjoint from [`ENTRY_FAMILY`] and
/// [`META_FAMILY`], so an index cell never aliases an entry or meta cell.
const INDEX_FAMILY: u8 = 0x02;

/// The value stored at a marker cell: the payload presence record.
pub(super) const MARKER_VALUE: &[u8] = &[0x01];

/// The `0x01 0x20 num(root)` prefix shared by every cell of the root numbered `root`.
pub(super) fn entry_family_prefix(root: NodeNumber) -> Vec<u8> {
    let mut out = vec![ENTRY_FAMILY, ROOT_TAG];
    push_component(&mut out, root);
    out
}

/// The marker key of the entry keyed by the tuple `keys` under `root`. A root entry's
/// marker key is its marker *stem*: the byte prefix from which its field leaves, its
/// branch descendants, and its cursor all derive (see [`stem_field_leaf`],
/// [`stem_cursor`]). `keys` is the root's whole key tuple, one column per key scalar.
pub(super) fn marker_key(root: NodeNumber, keys: &[KeyScalar]) -> Vec<u8> {
    child_marker(&entry_family_prefix(root), keys)
}

/// The field-leaf key of `field` of the node whose marker `stem` is given. The leaf
/// extends the stem with the field tag and the escaped field name, so it nests
/// inside the node's `(marker, cursor]` range ahead of any branch descendant. The
/// single owner of the marker-stem-to-field-leaf mapping: the root entry and every
/// branch entry derive their field leaves through it from their own resolved stem.
pub(super) fn stem_field_leaf(stem: &[u8], field: NodeNumber) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(FIELD_TAG);
    push_component(&mut out, field);
    out
}

/// The byte prefix shared by every own field leaf of the node whose marker `stem` is
/// given: the stem and the field tag. A `scan_after` bounded by this prefix yields
/// exactly the node's own field-leaf cells (`stem 0x10 …`) in field-name order and
/// stops at the group tag (`0x28`), the branch tag (`0x30`), or the next node — so a
/// whole-node read visits only *present* field leaves, never a cell per declared
/// field. The single owner of the own-field-leaf range bound, paired with
/// [`stem_field_leaf`], which extends it with one escaped field name.
pub(super) fn field_leaf_range(stem: &[u8]) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(FIELD_TAG);
    out
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

/// The byte prefix shared by every cell of the branch family `branch` beneath the
/// node whose marker `parent_stem` is given: the parent stem, the branch tag, and the
/// branch number. Because the fixed-width branch number self-delimits, this prefix
/// uniquely delimits one branch family — a cell of a differently-named branch never
/// shares it — so it is the traversable [`Layer`] prefix of that branch. The single
/// owner of the branch-family prefix bytes; both [`branch_child_stem`] and
/// [`Layer::branch`] derive from it.
pub(super) fn branch_family_prefix(parent_stem: &[u8], branch: NodeNumber) -> Vec<u8> {
    let mut out = parent_stem.to_vec();
    out.push(BRANCH_TAG);
    push_component(&mut out, branch);
    out
}

/// The byte prefix shared by every field leaf of the group `group` beneath the node
/// whose marker `stem` is given: the node's marker stem, the group tag, and the escaped
/// group number. Because the fixed-width group number self-delimits, this prefix uniquely delimits one
/// group's leaves — a leaf of a differently-named sibling group, or one of the node's
/// own top-level field leaves, never shares it. A group carries no marker and no key
/// (its presence is its containing entry's presence), so this stem is *not* a marker
/// stem: its leaves derive from it through the shared [`stem_field_leaf`] owner exactly
/// as a node's own fields derive from its marker stem, one namespace level down. The
/// single owner of the group-namespace prefix bytes; both a group's leaf enumeration and
/// its whole-group read/replace/erase derive every cell it touches from this prefix.
pub(super) fn group_stem(stem: &[u8], group: NodeNumber) -> Vec<u8> {
    let mut out = stem.to_vec();
    out.push(GROUP_TAG);
    push_component(&mut out, group);
    out
}

/// The marker stem of a branch child: the parent node's marker `stem`, the branch
/// tag, the escaped branch name, the child key, and a marker terminator. The result
/// is itself a marker stem — the branch child's own field leaves, nested branches,
/// and cursor derive from it exactly as a root entry's do (the recursive layout of
/// this module's header) — and it nests inside the parent's `(marker, cursor]` range
/// ahead of the parent's cursor, so one seek past the parent cursor skips it. This is
/// the single owner of the marker-stem-to-branch-child mapping; the whole-entry
/// planner and every branch session op derive a branch node's stem through it.
pub(super) fn branch_child_stem(
    parent_stem: &[u8],
    branch: NodeNumber,
    child_keys: &[KeyScalar],
) -> Vec<u8> {
    child_marker(&branch_family_prefix(parent_stem, branch), child_keys)
}

/// The marker key of the entry keyed by the tuple `keys` directly under a layer
/// `prefix`: the prefix, the prefix-free encoding of the whole key tuple, and a marker
/// terminator. The single owner of the layer-prefix-plus-key marker shape shared by a
/// root entry (prefix `0x01 0x20 num(root)`) and a branch child (prefix
/// `parent 0x30 num(branch)`). Because the tuple encoding is prefix-free (each column
/// self-delimits), two distinct key tuples yield markers where neither is a prefix of
/// the other, so the containment and separation laws hold column-wise as they do for a
/// single key.
fn child_marker(prefix: &[u8], keys: &[KeyScalar]) -> Vec<u8> {
    let mut out = prefix.to_vec();
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

/// The structural role of the byte immediately after a node's marker terminator: one
/// of the node's own field leaves, one of its branch descendants, or an unrecognized
/// tag. This is the single owner of post-marker tag meaning, consulted by both the
/// iteration classifier ([`classify_under_prefix`]) and the bounded prefix probe
/// ([`below_marker`]) so the two never disagree on an unknown tag. An unrecognized
/// tag is a cell shape the layout never writes, so both read it as corruption
/// (fail-closed) rather than one skipping it and the other treating it as absent.
enum StemTag {
    Field,
    Group,
    Branch,
    Unknown,
}

fn stem_tag(byte: u8) -> StemTag {
    match byte {
        FIELD_TAG => StemTag::Field,
        GROUP_TAG => StemTag::Group,
        BRANCH_TAG => StemTag::Branch,
        _ => StemTag::Unknown,
    }
}

/// Classify a cell key found at or after an iteration cursor, relative to a traversed
/// layer's prefix. A well-formed marker yields its key; a branch descendant reached
/// where a marker would begin identifies a descendant-only child (a node with children
/// but no payload); a field leaf or an unrecognized tag reached there is an orphan (a
/// marker/field or unknown-shape mismatch — corruption); a cell outside the layer ends
/// iteration.
pub(super) enum CellKind {
    /// An entry marker: the decoded immediate child key.
    Marker(KeyScalar),
    /// A branch descendant of the immediate child `key` whose own payload marker is
    /// absent — a descendant-only node. Iteration seeks past the child's cursor to skip
    /// its whole subtree: the node holds children but no visitable payload.
    Descendant(KeyScalar),
    /// A field leaf sitting where a marker must be — a marker/field mismatch
    /// (corruption).
    Orphan,
    /// A cell outside the traversed layer's prefix: iteration is done.
    Foreign,
}

/// Classify a scanned cell relative to a layer `prefix` — the root entry family or a
/// branch family. The structural tag immediately after the layer's child key
/// distinguishes them: a lone marker terminator is the marker, a terminator then the
/// branch tag a markerless (descendant-only) child, a terminator then the field tag
/// (or any other shape) an orphan, and a cell not under the prefix foreign. The single
/// owner of layer-relative cell meaning, reached through [`Layer::classify`] for both
/// the root and branch layers.
fn classify_under_prefix(prefix: &[u8], cell_key: &[u8]) -> CellKind {
    let Some(rest) = cell_key.strip_prefix(prefix) else {
        return CellKind::Foreign;
    };
    let Some((key, used)) = decode_key_value(rest) else {
        return CellKind::Orphan;
    };
    match &rest[used..] {
        // marker stem terminator and nothing more: a marker.
        [MARKER_TERMINATOR] => CellKind::Marker(key),
        [MARKER_TERMINATOR, tag, ..] => match stem_tag(*tag) {
            // a branch tag: a branch descendant of a markerless entry.
            StemTag::Branch => CellKind::Descendant(key),
            // an own field leaf or a group leaf (both the entry's own payload) where a
            // marker belongs is a marker/payload mismatch; an unrecognized tag is a
            // shape the layout never writes — all corruption.
            StemTag::Field | StemTag::Group | StemTag::Unknown => CellKind::Orphan,
        },
        // no marker terminator after the key (a malformed cell): corruption.
        _ => CellKind::Orphan,
    }
}

/// What a cell sorting strictly after a node's marker `stem`, under the stem's own
/// prefix, is: one of the node's own field leaves (`stem 0x10 …`), one of its group
/// leaves (`stem 0x28 …`), a cell of one of its branch descendants (`stem 0x30 …`), an
/// unrecognized structural tag (a shape the layout never writes — corruption), or
/// foreign (not under the stem — which the bounded probe's prefix bound already
/// excludes). The probe reads the first such cell to tell a descendant-only node (a
/// branch descendant with no marker) from an orphan (an own field or group leaf with no
/// marker) and to fail closed on an unknown tag. A group leaf, like a field leaf, is the
/// entry's own payload, so a markerless one is an orphan — not a descendant-only node.
pub(super) enum BelowMarker {
    OwnField,
    OwnGroup,
    BranchDescendant,
    Corrupt,
    Foreign,
}

/// Classify a cell sitting strictly after the marker `stem`, relative to that stem,
/// through the shared [`stem_tag`] owner so it agrees with [`classify_under_prefix`] on an
/// unrecognized tag (both read it as corruption).
pub(super) fn below_marker(stem: &[u8], cell_key: &[u8]) -> BelowMarker {
    match cell_key.strip_prefix(stem) {
        Some([tag, ..]) => match stem_tag(*tag) {
            StemTag::Field => BelowMarker::OwnField,
            StemTag::Group => BelowMarker::OwnGroup,
            StemTag::Branch => BelowMarker::BranchDescendant,
            StemTag::Unknown => BelowMarker::Corrupt,
        },
        _ => BelowMarker::Foreign,
    }
}

/// A traversable layer of immediate keyed children sharing one byte prefix: the root's
/// own entry family (`0x01 0x20 num(root)`) or one keyed branch family beneath a fixed
/// parent entry (`parent 0x30 num(branch)`). A child's marker is
/// `prefix ++ enc(key) ++ MARKER_TERMINATOR`; raising that terminator to the cursor
/// sentinel yields the child's subtree cursor, so one prefix-successor seek past a
/// child skips its whole subtree regardless of branch fan-out (the traversal-skip
/// law). The root and branch layers therefore share one forward-traversal owner —
/// bounded acquisition drives both through this type.
pub(super) struct Layer {
    prefix: Vec<u8>,
}

impl Layer {
    /// The root's own entry family.
    pub(super) fn root(root: NodeNumber) -> Self {
        Self {
            prefix: entry_family_prefix(root),
        }
    }

    /// The branch family numbered `branch` beneath the entry whose marker `parent_stem` is
    /// given. Because the branch number is fixed width, this prefix delimits exactly one
    /// branch family: a differently-numbered sibling branch is foreign to it.
    pub(super) fn branch(parent_stem: &[u8], branch: NodeNumber) -> Self {
        Self {
            prefix: branch_family_prefix(parent_stem, branch),
        }
    }

    /// The byte prefix shared by every cell of this layer. A `scan_after` bounded by it
    /// stays inside the layer; a cell not under it is foreign (iteration is done).
    pub(super) fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    /// The inclusive-`from` seek start: `prefix ++ enc(from)`. It sorts strictly below
    /// `from`'s own marker (which appends the terminator) and strictly above every
    /// earlier child's cursor (the prefix-free key encoding orders them), so a forward
    /// scan strictly after it yields `from`'s marker when `from` is present, else the
    /// first present child above `from`. This expresses an inclusive lower bound over
    /// an engine scan that excludes its cursor.
    pub(super) fn seek_from(&self, from: &KeyScalar) -> Vec<u8> {
        let mut out = self.prefix.clone();
        out.extend_from_slice(&encode_key_value(from));
        out
    }

    /// The cursor that resumes a forward scan strictly past child `key`'s whole
    /// subtree: the child's marker with its terminator raised to the cursor sentinel. A
    /// traversable layer is single-column (composite-keyed layers are not traversed), so
    /// the child is named by one key column.
    pub(super) fn child_cursor(&self, key: &KeyScalar) -> Vec<u8> {
        stem_cursor(&child_marker(&self.prefix, std::slice::from_ref(key)))
    }

    /// Classify a cell scanned under this layer's prefix
    /// (see [`classify_under_prefix`]).
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

    /// A single-column branch-child stem, the tuple builder's one-column convenience.
    fn bcs(parent_stem: &[u8], branch: NodeNumber, key: &KeyScalar) -> Vec<u8> {
        branch_child_stem(parent_stem, branch, std::slice::from_ref(key))
    }

    /// A root entry's subtree cursor, the root convenience over [`Layer::child_cursor`]
    /// the ordering tests assert against.
    fn cursor(root: NodeNumber, key: &KeyScalar) -> Vec<u8> {
        Layer::root(root).child_cursor(key)
    }

    /// Classify a cell against a root's entry family, the root convenience over
    /// [`Layer::classify`] the classification tests assert against.
    fn classify_cell(root: NodeNumber, cell_key: &[u8]) -> CellKind {
        Layer::root(root).classify(cell_key)
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

    /// Every cell a branch descendant can occupy — its marker, a field leaf on it, a
    /// nested sub-branch marker, and its cursor — sorts strictly inside the parent
    /// root entry's `(marker(parent), cursor(parent))` range, for every
    /// representative parent key. This is the recursive containment law: a whole
    /// subtree lives under one entry's marker and below its cursor.
    #[test]
    fn branch_descendants_nest_inside_the_parent_entry_range() {
        let root = ROOT_BOOKS;
        for parent in representative_keys() {
            let parent_marker = mk(root, &parent);
            let parent_cursor = cursor(root, &parent);
            let child = bcs(&parent_marker, B_NOTES, &KeyScalar::Int(7));
            let child_field = stem_field_leaf(&child, F_TEXT);
            let child_cursor = stem_cursor(&child);
            let grandchild = bcs(&child, B_TAGS, &KeyScalar::Str("x".into()));
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
        let root = ROOT_BOOKS;
        let parent = mk(root, &KeyScalar::Str("a".into()));
        let own_field = stem_field_leaf(&parent, F_TITLE);
        let branch_child = bcs(&parent, B_NOTES, &KeyScalar::Int(1));
        assert!(
            parent.as_slice() < own_field.as_slice(),
            "marker precedes own field"
        );
        assert!(
            own_field.as_slice() < branch_child.as_slice(),
            "own field precedes branch descendants",
        );
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

    /// The group tag sits between the field tag and the branch tag, so within one entry
    /// the own field leaves precede the group leaves, which precede the branch
    /// descendants — the field < group < branch precedence the bounded prefix probe and
    /// iteration classifier rely on. Every group leaf still sorts after the marker and
    /// before the cursor.
    #[test]
    fn group_leaves_sort_between_own_fields_and_branch_descendants() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let marker = mk(root, &key);
        let cur = cursor(root, &key);
        let own_field = stem_field_leaf(&marker, F_TITLE);
        let group_leaf = group_leaf(root, &key, G_DETAILS, F_PAGES);
        let branch_child = bcs(&marker, B_NOTES, &KeyScalar::Int(1));
        assert!(marker < own_field, "marker precedes own field");
        assert!(
            own_field < group_leaf,
            "own field leaf precedes a group leaf"
        );
        assert!(
            group_leaf < branch_child,
            "group leaf precedes a branch descendant"
        );
        assert!(group_leaf < cur, "group leaf precedes the cursor");
    }

    /// Every cell a group can occupy — each of its leaves, over representative parent
    /// keys — nests strictly inside the entry's `(marker, cursor)` range, so a group is
    /// part of exactly one entry's subtree and one seek past the cursor skips it with the
    /// rest of the entry.
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
        // A branch cell is outside the group prefix (branch tag 0x30 ≠ group tag 0x28).
        let branch_child = bcs(&marker, B_NOTES, &KeyScalar::Int(1));
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

    /// A group leaf sitting where a root entry marker would begin classifies as an orphan
    /// (own payload with no marker — corruption), through the shared [`stem_tag`] owner
    /// that also reports it as [`BelowMarker::OwnGroup`] on the probe path — so a
    /// markerless group leaf can never slip through one path as a descendant-only node
    /// while the other calls it corruption.
    #[test]
    fn a_markerless_group_leaf_is_an_orphan_on_both_paths() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let stem = mk(root, &key);
        let leaf = group_leaf(root, &key, G_DETAILS, F_PAGES);
        assert!(matches!(classify_cell(root, &leaf), CellKind::Orphan));
        assert!(matches!(below_marker(&stem, &leaf), BelowMarker::OwnGroup));
    }

    /// The parent cursor sorts after the parent's whole subtree (own fields and every
    /// branch descendant) and before the next root sibling's marker, so one seek past
    /// the cursor skips the subtree regardless of branch fan-out.
    #[test]
    fn parent_cursor_skips_the_whole_subtree_and_precedes_the_next_sibling() {
        let root = ROOT_BOOKS;
        let a = mk(root, &KeyScalar::Str("a".into()));
        let a_cursor = cursor(root, &KeyScalar::Str("a".into()));
        let b_marker = mk(root, &KeyScalar::Str("b".into()));
        let subtree = [
            stem_field_leaf(&a, F_TITLE),
            bcs(&a, B_NOTES, &KeyScalar::Int(i64::MIN)),
            bcs(&a, B_NOTES, &KeyScalar::Int(i64::MAX)),
            stem_field_leaf(&bcs(&a, B_NOTES, &KeyScalar::Int(1)), F_TEXT),
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
        let root = ROOT_BOOKS;
        let parent = mk(root, &KeyScalar::Str("a".into()));
        let mut children = representative_keys();
        children.sort();
        for pair in children.windows(2) {
            let lo = bcs(&parent, B_NOTES, &pair[0]);
            let lo_field = stem_field_leaf(&lo, F_TEXT);
            let lo_cursor = stem_cursor(&lo);
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

    /// A branch descendant reached where a root entry marker would begin classifies
    /// as a descendant of that entry's key (so iteration skips a markerless
    /// descendant-only node), while a marker classifies as present and an own field
    /// leaf as an orphan. A deep sub-branch cell still classifies against the
    /// root-level key it descends from.
    #[test]
    fn classify_recognizes_a_branch_descendant() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let parent = mk(root, &key);
        let child = bcs(&parent, B_NOTES, &KeyScalar::Int(1));
        let grandchild = bcs(&child, B_TAGS, &KeyScalar::Int(2));
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
            classify_cell(root, &stem_field_leaf(&parent, F_TITLE)),
            CellKind::Orphan
        ));
    }

    /// An unrecognized structural tag after a marker stem is a cell shape the layout
    /// never writes; the iteration classifier and the bounded prefix probe agree it
    /// is corruption through the shared [`stem_tag`] owner, so a future third tag
    /// cannot slip through one path as absent while the other calls it corruption.
    #[test]
    fn an_unknown_post_stem_tag_is_corruption_on_both_paths() {
        let root = ROOT_BOOKS;
        let key = KeyScalar::Str("a".into());
        let stem = mk(root, &key);
        // 0x40 is neither FIELD_TAG (0x10) nor BRANCH_TAG (0x30): a tag the layout
        // never emits, so it can only arise from corruption.
        let mut rogue = stem.clone();
        rogue.push(0x40);
        rogue.extend_from_slice(b"junk");
        assert!(matches!(below_marker(&stem, &rogue), BelowMarker::Corrupt));
        assert!(matches!(classify_cell(root, &rogue), CellKind::Orphan));
    }

    /// The containment and separation laws hold for a *composite* key whose columns are
    /// NUL-laden and escape-shaped, so a naive per-byte reading could confuse a column
    /// boundary. Two composite entries differing only in a later column have markers
    /// where neither is a prefix of the other and each entry's whole subtree — its own
    /// fields and a composite-keyed branch child — nests inside its own `(marker, cursor]`
    /// range and outside its sibling's. This is the multi-column extension of the
    /// single-key ordering laws the traversal-skip and precedence rules rest on.
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
            let lo_cursor = stem_cursor(&marker_key(root, pair[0]));
            let hi = marker_key(root, pair[1]);
            // Neither marker is a prefix of the other (prefix-free tuples).
            assert!(
                !hi.starts_with(&lo),
                "a composite marker is a prefix of a sibling"
            );
            // lo's whole subtree — its own field and a composite-keyed branch child —
            // nests below lo's cursor, which precedes the next sibling's marker.
            let lo_field = stem_field_leaf(&marker_key(root, pair[0]), F_VALUE);
            let lo_branch = branch_child_stem(
                &marker_key(root, pair[0]),
                B_SPANS,
                &[KeyScalar::Int(9), KeyScalar::Bytes(vec![0x00, 0x00])],
            );
            for cell in [&lo_field, &lo_branch] {
                assert!(
                    lo < *cell && cell.as_slice() < lo_cursor.as_slice(),
                    "a composite entry's cell nests in its own (marker, cursor] range"
                );
            }
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

    /// The frozen id-keyed cell-key layout (FR01 §3 durability contract): the exact bytes of
    /// a marker, a field leaf, a group leaf, a branch child marker, and an index cell key,
    /// so the on-disk key grammar cannot drift silently. Each node component is its 4-byte
    /// big-endian number; the marker terminator, structural tags, and key-tuple encoding are
    /// the same frozen bytes the ordering laws rest on.
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

        // Branch child marker: <marker> 0x30 num(branch=20) enc(childKey) 0x00
        let child = KeyScalar::Int(7);
        let branch = branch_child_stem(&marker, 20, std::slice::from_ref(&child));
        let mut expected = marker.clone();
        expected.extend_from_slice(&[0x30, 0x00, 0x00, 0x00, 0x14]);
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
