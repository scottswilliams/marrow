//! One bounded, read-only walk over every cell of a store against its admitted projection.
//!
//! The walk reads the whole key space forward, one engine page at a time, and parses the
//! cell stream in the order the layout (`physical.rs`) sorts it: each declared root or
//! branch entry family — every entry's marker, then its own field and group leaves —
//! then the managed-index families and metadata. Its retained state is one open entry,
//! fixed per-schema tables and a capped finding list; nothing grows with the number of
//! stored entries. Key-kind paths borrow at most one slice per root and branch hop from
//! the projection, so no ancestor key schema is copied per descendant. Every cell is
//! classified exactly once, and a cell that belongs to no declared node or index is a
//! typed finding rather than a skipped byte. A required leaf that is absent is found when
//! the node it belongs to closes, so a missing cell is reported as precisely as a present
//! one. Managed-index cells are checked against their source entries by point reads, and
//! every present root entry with a complete projection is checked for its index cell, so the
//! walk's engine work is one page per engine scan batch plus a bounded number of point
//! reads per index cell and per indexed entry — never a read per declared field.
//!
//! The logical content the walk sees — every cell of a declared entry family, in
//! key order — is handed to a caller-supplied [`ContentDigest`] one cell at a time, so the
//! digest is computed in the same single pass with the same bounded memory.

use std::collections::HashMap;

use marrow_store::{ReadView, StoreError};

use super::physical::{self, BelowMarker};
use super::store::{WITNESS, witness_well_formed};
use super::{
    BranchNumbering, BranchSchema, FieldSchema, GroupNumbering, GroupSchema, IndexComponentRef,
    IndexSchema, NodeNumber, RootNumbering, StoreProjection, StoreSchema,
};
use crate::codec::key::{KeyScalar, decode_key_value};
use crate::codec::value::{ScalarKind, ValueShape, decode_domain, scalar_key_matches_type};
use crate::equality::ValueDomain;

/// The most findings a report retains in full. Every finding is still counted in
/// [`AuditSummary::findings`]; the cap bounds the report's memory on a store whose every
/// cell is faulty.
pub const MAX_REPORTED_FINDINGS: usize = 256;

/// A consumer of the store's logical content stream: one call per cell of a declared
/// root or branch entry family, in key order. The digest kind and hash live with the store's
/// durability identities downstream; the kernel only streams the canonical cells.
pub trait ContentDigest {
    fn absorb(&mut self, key: &[u8], value: &[u8]);
}

/// What one finding reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFault {
    /// The key or value bytes do not parse under the layout or the declared shape.
    Undecodable,
    /// A well-formed cell names a root, field, group, branch, index, or family the
    /// projection does not declare.
    OutsideSchema,
    /// A present node lacks a required field leaf.
    RequiredMissing,
    /// An own payload leaf sits under a node with no marker.
    OrphanLeaf,
    /// A marker cell whose value is not the presence record.
    MarkerInvalid,
    /// An index cell whose source entry is absent.
    IndexOrphan,
    /// An index cell whose projected values disagree with its source entry.
    IndexStale,
    /// A present entry with a complete projection has no index cell naming it.
    IndexMissing,
    /// The commit witness cell holds bytes no witness encoding this build reads.
    WitnessInvalid,
}

/// Where a finding sits, by projection position and key path. The schema's names render
/// it; the kernel reports positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditSite {
    /// A durable node: its root position, branch path (one position per level), and the
    /// whole key path from the root key down.
    Node {
        root: u16,
        branch: Vec<u16>,
        keys: Vec<KeyScalar>,
    },
    /// One field of a node, top-level or inside a group.
    Field {
        root: u16,
        branch: Vec<u16>,
        keys: Vec<KeyScalar>,
        group: Option<u16>,
        field: u16,
    },
    /// One cell of a managed index: the index's root and position and its projected values.
    IndexCell {
        root: u16,
        index: u16,
        values: Vec<KeyScalar>,
    },
    /// The index family of a declared root under an identity the program does not declare.
    UndeclaredIndex { root: u16, id: [u8; 16] },
    /// A cell no declared node or index owns, named by its raw key.
    Cell { key: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub fault: AuditFault,
    pub site: AuditSite,
}

/// The counts the walk keeps: every cell scanned, present entries (markers), index cells,
/// and findings (including those past the cap).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditSummary {
    pub cells: u64,
    pub entries: u64,
    pub index_cells: u64,
    pub findings: u64,
}

/// The walk's result: counts and the first [`MAX_REPORTED_FINDINGS`] findings in
/// deterministic scan/closure order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport {
    pub summary: AuditSummary,
    pub findings: Vec<AuditFinding>,
}

impl AuditReport {
    /// Whether the walk found nothing wrong.
    pub fn is_clean(&self) -> bool {
        self.summary.findings == 0
    }
}

/// Walk every cell of `view` against `projection` and its `numbering`, streaming the
/// logical content to `digest`.
pub(super) fn walk<V: ReadView>(
    view: &V,
    projection: &StoreProjection,
    numbering: &[RootNumbering],
    digest: &mut dyn ContentDigest,
) -> Result<AuditReport, StoreError> {
    let tables = Tables::new(projection, numbering);
    let mut walker = Walker {
        view,
        tables: &tables,
        digest,
        frame: None,
        family_cursor: 0,
        index_cursor: 0,
        index_family_cursor: 0,
        summary: AuditSummary::default(),
        findings: Vec::new(),
    };
    // Paging is strictly after its cursor; the empty key has no preceding cursor.
    if let Some(value) = view.get(&[])? {
        walker.cell(&[], &value)?;
    }
    let mut cursor: Vec<u8> = Vec::new();
    loop {
        let page = view.scan_after(&[], &cursor)?;
        let Some((last, _)) = page.last() else {
            break;
        };
        cursor = last.clone();
        for (key, value) in &page {
            walker.cell(key, value)?;
        }
    }
    walker.close_entry()?;
    Ok(AuditReport {
        summary: walker.summary,
        findings: walker.findings,
    })
}

/// One declared field: its value shape and whether a present container must carry it.
struct FieldSlot {
    shape: ValueShape,
    required: bool,
}

/// One declared group: the suffix of its leaf namespace below the entry stem, and its
/// fields keyed by their own suffix below the group stem.
struct GroupSlot {
    suffix: Vec<u8>,
    fields: Vec<FieldSlot>,
    by_suffix: HashMap<Vec<u8>, usize>,
}

/// One declared root or branch family. The complete key path borrows the root's key
/// schema and each branch's own key schema, in order; ancestor presence is irrelevant.
struct FamilyShape<'a> {
    root: usize,
    number: NodeNumber,
    prefix: Vec<u8>,
    /// The branch positions from the root down to this node (empty for the root).
    branch_path: Vec<u16>,
    key_segments: Vec<&'a [ScalarKind]>,
    fields: Vec<FieldSlot>,
    by_suffix: HashMap<Vec<u8>, usize>,
    groups: Vec<GroupSlot>,
}

/// One projected component of a managed index, by position into the root's key tuple or
/// its top-level fields.
#[derive(Clone, Copy)]
enum Component {
    Key(usize),
    Field(usize),
}

/// One managed index of a root: its family prefix, its position, and its projection.
struct IndexShape {
    root: u16,
    position: u16,
    prefix: Vec<u8>,
    id: [u8; 16],
    components: Vec<(Component, ScalarKind)>,
}

/// One declared root: its entry-family position, the prefix every index family under it
/// shares, its cell-key number, the top-level field numbers (for index
/// reads), and its indexes' table positions.
struct RootShape {
    family: usize,
    index_family: Vec<u8>,
    number: NodeNumber,
    field_numbers: Vec<NodeNumber>,
    indexes: Vec<usize>,
}

/// The fixed tables the walk classifies against, built once from the projection.
struct Tables<'a> {
    /// Every entry family in store-number order, which is also physical prefix order.
    families: Vec<FamilyShape<'a>>,
    /// Every root in declaration order, for managed-index source identity.
    roots: Vec<RootShape>,
    /// Every index of every root, sorted by family prefix (the order their cells appear).
    indexes: Vec<IndexShape>,
    witness: Vec<u8>,
}

impl<'a> Tables<'a> {
    fn new(projection: &'a StoreProjection, numbering: &[RootNumbering]) -> Self {
        let mut families = Vec::new();
        let mut indexes = Vec::new();
        let mut roots = Vec::with_capacity(projection.roots().len());
        for (position, (schema, numbers)) in projection.roots().iter().zip(numbering).enumerate() {
            let mut own = Vec::with_capacity(schema.indexes().len());
            for (index_pos, index) in schema.indexes().iter().enumerate() {
                own.push(indexes.len());
                indexes.push(index_shape(
                    position as u16,
                    index_pos as u16,
                    numbers.root(),
                    schema,
                    index,
                ));
            }
            let family = families.len();
            let key_segments = [schema.key()];
            families.push(FamilyShape {
                root: position,
                number: numbers.root(),
                prefix: physical::entry_family_prefix(numbers.root()),
                branch_path: Vec::new(),
                key_segments: key_segments.to_vec(),
                fields: field_slots(schema.fields()),
                by_suffix: suffix_map(numbers.fields()),
                groups: group_slots(schema.groups(), numbers.groups()),
            });
            append_branches(
                &mut families,
                position,
                &[],
                &key_segments,
                schema.branches(),
                numbers.branches(),
            );
            // An index cell key is the index family byte, the root number, then the index's
            // 16-byte identity: every index family of one root shares the bytes ahead of
            // the identity.
            let any_index = physical::index_cell_key(numbers.root(), &[0; 16], &[]);
            roots.push(RootShape {
                family,
                index_family: any_index[..any_index.len() - 16].to_vec(),
                number: numbers.root(),
                field_numbers: numbers.fields().to_vec(),
                indexes: own,
            });
        }
        // Root numbers increase in declaration order, so sorting permutes indexes only
        // within each root's retained contiguous range.
        indexes.sort_by(|a, b| a.prefix.cmp(&b.prefix));
        Self {
            families,
            roots,
            indexes,
            witness: physical::meta_key(WITNESS),
        }
    }
}

fn index_shape(
    root: u16,
    position: u16,
    root_number: NodeNumber,
    schema: &StoreSchema,
    index: &IndexSchema,
) -> IndexShape {
    let components = index
        .projection()
        .iter()
        .map(|component| match component.view() {
            IndexComponentRef::Key(column) => {
                let column = usize::from(column);
                (Component::Key(column), schema.key()[column])
            }
            IndexComponentRef::Field(field) => {
                let field = usize::from(field);
                let kind = schema.fields()[field]
                    .shape()
                    .scalar_kind()
                    .expect("the schema builder admits only scalar index components");
                (Component::Field(field), kind)
            }
        })
        .collect();
    IndexShape {
        root,
        position,
        prefix: physical::index_cell_key(root_number, index.id(), &[]),
        id: *index.id(),
        components,
    }
}

fn field_slots(fields: &[FieldSchema]) -> Vec<FieldSlot> {
    fields
        .iter()
        .map(|field| FieldSlot {
            shape: field.shape().clone(),
            required: field.required(),
        })
        .collect()
}

/// The leaf suffix of each numbered field, keyed for the lookup a scanned leaf performs.
/// Every layout constructor extends the stem it is given, so the suffix a leaf carries
/// below any stem is the leaf built on an empty stem (pinned by `suffixes_extend_stems`).
fn suffix_map(numbers: &[NodeNumber]) -> HashMap<Vec<u8>, usize> {
    numbers
        .iter()
        .enumerate()
        .map(|(index, number)| (physical::stem_field_leaf(&[], *number), index))
        .collect()
}

fn group_slots(groups: &[GroupSchema], numbers: &[GroupNumbering]) -> Vec<GroupSlot> {
    groups
        .iter()
        .zip(numbers)
        .map(|(group, numbering)| GroupSlot {
            suffix: physical::group_stem(&[], numbering.number()),
            fields: field_slots(group.fields()),
            by_suffix: suffix_map(numbering.fields()),
        })
        .collect()
}

/// Append each branch family in the existing numbering's pre-order. The temporary path
/// and borrowed key slices follow the schema's bounded depth, not stored ancestors.
fn append_branches<'a>(
    families: &mut Vec<FamilyShape<'a>>,
    root: usize,
    parent_path: &[u16],
    parent_keys: &[&'a [ScalarKind]],
    branches: &'a [BranchSchema],
    numbers: &[BranchNumbering],
) {
    for (position, (branch, numbering)) in branches.iter().zip(numbers).enumerate() {
        let mut branch_path = parent_path.to_vec();
        branch_path.push(position as u16);
        let mut key_segments = parent_keys.to_vec();
        key_segments.push(branch.key());
        families.push(FamilyShape {
            root,
            number: numbering.number(),
            prefix: physical::entry_family_prefix(numbering.number()),
            branch_path: branch_path.clone(),
            key_segments: key_segments.clone(),
            fields: field_slots(branch.fields()),
            by_suffix: suffix_map(numbering.fields()),
            groups: Vec::new(),
        });
        append_branches(
            families,
            root,
            &branch_path,
            &key_segments,
            branch.branches(),
            numbering.branches(),
        );
    }
}

/// One open entry. Its presence flags follow the declared own/group width; its remaining
/// storage holds this entry's encoded/decoded keys and indexed scalar projections.
struct Frame {
    family: usize,
    stem: Vec<u8>,
    keys: Vec<KeyScalar>,
    marker: bool,
    own: Vec<bool>,
    groups: Vec<Vec<bool>>,
    /// The decoded key projection of each top-level field (root entries of an indexed root
    /// only), for the index-cell check at close.
    projected: Vec<Option<KeyScalar>>,
    any_leaf: bool,
}

struct Walker<'a, V: ReadView> {
    view: &'a V,
    tables: &'a Tables<'a>,
    digest: &'a mut dyn ContentDigest,
    frame: Option<Frame>,
    family_cursor: usize,
    index_cursor: usize,
    index_family_cursor: usize,
    summary: AuditSummary,
    findings: Vec<AuditFinding>,
}

/// Advance a monotonic cursor over `items` sorted ascending by `prefix_of` to the one
/// `key` is under, if any. An item whose prefix sorts wholly below `key` is passed for
/// good: the scan is ascending, so no later key can be under it.
fn seek_prefix<T>(
    items: &[T],
    prefix_of: impl Fn(&T) -> &[u8],
    cursor: &mut usize,
    key: &[u8],
) -> Option<usize> {
    while let Some(item) = items.get(*cursor) {
        let prefix = prefix_of(item);
        if key.starts_with(prefix) {
            return Some(*cursor);
        }
        if prefix < key {
            *cursor += 1;
        } else {
            return None;
        }
    }
    None
}

/// Decode the declared key components from the front of `bytes`, each at its declared kind,
/// returning the keys and the bytes consumed.
fn decode_keys<'a>(
    bytes: &[u8],
    kinds: impl Iterator<Item = &'a ScalarKind>,
) -> Option<(Vec<KeyScalar>, usize)> {
    let mut keys = Vec::new();
    let mut used = 0;
    for kind in kinds {
        let (column, n) = decode_key_value(bytes.get(used..)?)?;
        if !scalar_key_matches_type(&column, *kind) {
            return None;
        }
        keys.push(column);
        used += n;
    }
    Some((keys, used))
}

impl<V: ReadView> Walker<'_, V> {
    fn cell(&mut self, key: &[u8], value: &[u8]) -> Result<(), StoreError> {
        self.summary.cells += 1;
        if self
            .frame
            .as_ref()
            .is_some_and(|frame| key.starts_with(&frame.stem))
        {
            self.digest.absorb(key, value);
            self.within_node(key, value);
            Ok(())
        } else {
            self.close_entry()?;
            self.top_level(key, value)
        }
    }

    /// Classify a cell under no open entry: a declared entry family, a managed-index cell,
    /// the commit witness, or a cell outside every declared family.
    fn top_level(&mut self, key: &[u8], value: &[u8]) -> Result<(), StoreError> {
        let tables = self.tables;
        if let Some(family) = seek_prefix(
            &tables.families,
            |family| family.prefix.as_slice(),
            &mut self.family_cursor,
            key,
        ) {
            self.digest.absorb(key, value);
            self.enter_entry(family, key, value);
            return Ok(());
        }
        if let Some(index) = seek_prefix(
            &tables.indexes,
            |index| index.prefix.as_slice(),
            &mut self.index_cursor,
            key,
        ) {
            return self.index_cell(&tables.indexes[index], key, value);
        }
        if key == tables.witness.as_slice() {
            if !witness_well_formed(value) {
                self.finding(
                    AuditFault::WitnessInvalid,
                    AuditSite::Cell { key: key.to_vec() },
                );
            }
            return Ok(());
        }
        let site = match seek_prefix(
            &tables.roots,
            |root| root.index_family.as_slice(),
            &mut self.index_family_cursor,
            key,
        ) {
            Some(root) => {
                let rest = &key[tables.roots[root].index_family.len()..];
                match <[u8; 16]>::try_from(rest.get(..16).unwrap_or(&[])) {
                    Ok(id) => AuditSite::UndeclaredIndex {
                        root: root as u16,
                        id,
                    },
                    Err(_) => AuditSite::Cell { key: key.to_vec() },
                }
            }
            None => AuditSite::Cell { key: key.to_vec() },
        };
        self.finding(AuditFault::OutsideSchema, site);
        Ok(())
    }

    /// Decode the full ancestor-and-own tuple once when entering a concrete entry.
    /// Later cells share its validated marker stem; no ancestor entry is opened.
    fn enter_entry(&mut self, family: usize, key: &[u8], value: &[u8]) {
        let tables = self.tables;
        let shape = &tables.families[family];
        let root_shape = &tables.roots[shape.root];
        let kinds = shape.key_segments.iter().flat_map(|segment| segment.iter());
        let Some((keys, _)) = decode_keys(&key[shape.prefix.len()..], kinds) else {
            self.finding(
                AuditFault::Undecodable,
                AuditSite::Cell { key: key.to_vec() },
            );
            return;
        };
        let stem = physical::marker_key(shape.number, &keys);
        if !key.starts_with(&stem) {
            self.finding(
                AuditFault::Undecodable,
                AuditSite::Cell { key: key.to_vec() },
            );
            return;
        }
        let marker = key == stem.as_slice();
        self.frame = Some(Frame {
            family,
            stem,
            keys,
            marker,
            own: vec![false; shape.fields.len()],
            groups: shape
                .groups
                .iter()
                .map(|group| vec![false; group.fields.len()])
                .collect(),
            projected: if family == root_shape.family && !root_shape.indexes.is_empty() {
                vec![None; shape.fields.len()]
            } else {
                Vec::new()
            },
            any_leaf: false,
        });
        if marker {
            self.summary.entries += 1;
            if value != physical::MARKER_VALUE {
                let site = self.node_site();
                self.finding(AuditFault::MarkerInvalid, site);
            }
        } else {
            self.within_node(key, value)
        }
    }

    /// Classify a cell strictly below the current entry's marker stem.
    fn within_node(&mut self, key: &[u8], value: &[u8]) {
        let tables = self.tables;
        let frame = self.frame.as_ref().expect("a cell below an open entry");
        let shape = &tables.families[frame.family];
        let stem_len = frame.stem.len();
        let rest = &key[stem_len..];
        match physical::below_marker(&frame.stem, key) {
            BelowMarker::OwnField => {
                self.own_leaf();
                match shape.by_suffix.get(rest) {
                    Some(&field) => self.leaf(None, field, value),
                    None => self.finding(
                        AuditFault::OutsideSchema,
                        AuditSite::Cell { key: key.to_vec() },
                    ),
                }
            }
            BelowMarker::OwnGroup => {
                self.own_leaf();
                let located = shape
                    .groups
                    .iter()
                    .enumerate()
                    .find(|(_, group)| rest.starts_with(&group.suffix))
                    .and_then(|(index, group)| {
                        group
                            .by_suffix
                            .get(&rest[group.suffix.len()..])
                            .map(|&field| (index, field))
                    });
                match located {
                    Some((group, field)) => self.leaf(Some(group), field, value),
                    None => self.finding(
                        AuditFault::OutsideSchema,
                        AuditSite::Cell { key: key.to_vec() },
                    ),
                }
            }
            BelowMarker::Corrupt | BelowMarker::Foreign => {
                self.finding(
                    AuditFault::Undecodable,
                    AuditSite::Cell { key: key.to_vec() },
                );
            }
        }
    }

    /// Note an own payload leaf under the current entry; the first one under a markerless
    /// node is the orphan finding.
    fn own_leaf(&mut self) {
        let frame = self
            .frame
            .as_mut()
            .expect("a leaf belongs to an open entry");
        let first_orphan = !frame.marker && !frame.any_leaf;
        frame.any_leaf = true;
        if first_orphan {
            let site = self.node_site();
            self.finding(AuditFault::OrphanLeaf, site);
        }
    }

    /// Decode one declared leaf of the current entry and record its presence.
    fn leaf(&mut self, group: Option<usize>, field: usize, value: &[u8]) {
        let tables = self.tables;
        let frame = self
            .frame
            .as_mut()
            .expect("a leaf belongs to an open entry");
        let shape = &tables.families[frame.family];
        let slot = match group {
            Some(group) => &shape.groups[group].fields[field],
            None => &shape.fields[field],
        };
        let decoded = decode_domain(value, &slot.shape);
        match group {
            Some(group) => frame.groups[group][field] = true,
            None => {
                frame.own[field] = true;
                if let Some(projected) = frame.projected.get_mut(field) {
                    *projected = match &decoded {
                        Some(ValueDomain::Scalar(scalar)) => scalar.as_key().ok().flatten(),
                        _ => None,
                    };
                }
            }
        }
        if decoded.is_none() {
            let site = self.field_site(group, field);
            self.finding(AuditFault::Undecodable, site);
        }
    }

    /// Finish the current entry before moving to another concrete key or family.
    fn close_entry(&mut self) -> Result<(), StoreError> {
        if let Some(frame) = self.frame.take() {
            self.check_closed(&frame)?;
        }
        Ok(())
    }

    /// The checks a node admits only once every cell under it has been seen: the required
    /// leaves and index cells of a present node.
    fn check_closed(&mut self, frame: &Frame) -> Result<(), StoreError> {
        if !frame.marker {
            return Ok(());
        }
        let shape = &self.tables.families[frame.family];
        for (field, slot) in shape.fields.iter().enumerate() {
            if slot.required && !frame.own[field] {
                let site = self.site_of(frame, None, field);
                self.finding(AuditFault::RequiredMissing, site);
            }
        }
        for (group, slot) in shape.groups.iter().enumerate() {
            for (field, field_slot) in slot.fields.iter().enumerate() {
                if field_slot.required && !frame.groups[group][field] {
                    let site = self.site_of(frame, Some(group), field);
                    self.finding(AuditFault::RequiredMissing, site);
                }
            }
        }
        if frame.family == self.tables.roots[shape.root].family {
            self.check_index_cells(frame)?;
        }
        Ok(())
    }

    /// Every index of a present root entry whose projection is complete must hold the
    /// entry's index cell.
    fn check_index_cells(&mut self, frame: &Frame) -> Result<(), StoreError> {
        let tables = self.tables;
        let root = &tables.roots[tables.families[frame.family].root];
        if root.indexes.is_empty() {
            return Ok(());
        }
        let keys = &frame.keys;
        let source = physical::index_cell_value(keys);
        for &index in &root.indexes {
            let shape = &tables.indexes[index];
            let values: Option<Vec<KeyScalar>> = shape
                .components
                .iter()
                .map(|(component, _)| match component {
                    Component::Key(column) => keys.get(*column).cloned(),
                    Component::Field(field) => frame.projected.get(*field).cloned().flatten(),
                })
                .collect();
            let Some(values) = values else {
                continue;
            };
            let cell = physical::index_cell_key(root.number, &shape.id, &values);
            if self.view.get(&cell)?.as_deref() != Some(source.as_slice()) {
                self.finding(
                    AuditFault::IndexMissing,
                    AuditSite::IndexCell {
                        root: shape.root,
                        index: shape.position,
                        values,
                    },
                );
            }
        }
        Ok(())
    }

    /// Check one managed-index cell against its source entry.
    fn index_cell(
        &mut self,
        shape: &IndexShape,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), StoreError> {
        self.summary.index_cells += 1;
        let root = &self.tables.roots[usize::from(shape.root)];
        let kinds = shape.components.iter().map(|(_, kind)| kind);
        let rest = &key[shape.prefix.len()..];
        let values = match decode_keys(rest, kinds) {
            Some((values, used)) if used == rest.len() => values,
            _ => {
                self.finding(
                    AuditFault::Undecodable,
                    AuditSite::Cell { key: key.to_vec() },
                );
                return Ok(());
            }
        };
        let site = AuditSite::IndexCell {
            root: shape.root,
            index: shape.position,
            values: values.clone(),
        };
        let root_shape = &self.tables.families[root.family];
        let root_key = root_shape.key_segments[0];
        let source = physical::decode_index_source_key(value, root_key.len()).filter(|source| {
            source
                .iter()
                .zip(root_key)
                .all(|(column, kind)| scalar_key_matches_type(column, *kind))
        });
        let Some(source) = source else {
            self.finding(AuditFault::Undecodable, site);
            return Ok(());
        };
        let stem = physical::marker_key(root.number, &source);
        if self.view.get(&stem)?.is_none() {
            self.finding(AuditFault::IndexOrphan, site);
            return Ok(());
        }
        for ((component, _), projected) in shape.components.iter().zip(&values) {
            let agrees = match component {
                Component::Key(column) => source.get(*column) == Some(projected),
                Component::Field(field) => {
                    let leaf = physical::stem_field_leaf(&stem, root.field_numbers[*field]);
                    match self.view.get(&leaf)? {
                        None => false,
                        Some(bytes) => {
                            match decode_domain(&bytes, &root_shape.fields[*field].shape) {
                                Some(ValueDomain::Scalar(scalar)) => {
                                    scalar.as_key().ok().flatten().as_ref() == Some(projected)
                                }
                                // An undecodable source leaf is reported at the entry.
                                _ => true,
                            }
                        }
                    }
                }
            };
            if !agrees {
                self.finding(AuditFault::IndexStale, site);
                return Ok(());
            }
        }
        Ok(())
    }

    fn finding(&mut self, fault: AuditFault, site: AuditSite) {
        self.summary.findings += 1;
        if self.findings.len() < MAX_REPORTED_FINDINGS {
            self.findings.push(AuditFinding { fault, site });
        }
    }

    /// The node site of the current entry.
    fn node_site(&self) -> AuditSite {
        let frame = self
            .frame
            .as_ref()
            .expect("a node site names an open entry");
        let family = &self.tables.families[frame.family];
        AuditSite::Node {
            root: family.root as u16,
            branch: family.branch_path.clone(),
            keys: frame.keys.clone(),
        }
    }

    /// The field site of the current entry.
    fn field_site(&self, group: Option<usize>, field: usize) -> AuditSite {
        let frame = self
            .frame
            .as_ref()
            .expect("a field site names an open entry");
        self.site_of(frame, group, field)
    }

    fn site_of(&self, frame: &Frame, group: Option<usize>, field: usize) -> AuditSite {
        let family = &self.tables.families[frame.family];
        AuditSite::Field {
            root: family.root as u16,
            branch: family.branch_path.clone(),
            keys: frame.keys.clone(),
            group: group.map(|group| group as u16),
            field: field as u16,
        }
    }
}

#[cfg(test)]
mod tests {
    use marrow_store::{ByteEngine, CommitOutcome, MemoryEngine, WriteTxn};

    use super::*;
    use crate::codec::key::encode_key_tuple;
    use crate::codec::value::RuntimeScalar;
    use crate::durable::{
        CommitResult, CreateOutcome, DemandCoverage, Durable, DurableStore, EntryValue,
        IndexComponent, InvocationGrant, SiteTarget, StoreSchemaBuilder, number_store,
    };

    const BY_ISBN: [u8; 16] = [0xA1; 16];
    const BY_TITLE: [u8; 16] = [0xB2; 16];

    /// `^books[id: string]`: a required `title`, sparse `pages` and `isbn`, a `details` group
    /// with a sparse `pages`, a `notes[Int]` branch with a required `text` and a nested
    /// `tags[Str]` branch with a sparse `weight`, a unique index on `isbn`, and a nonunique
    /// index on `(title, id)`.
    fn projection() -> StoreProjection {
        let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
        builder.scalar_field("title", ScalarKind::Str, true);
        builder.scalar_field("pages", ScalarKind::Int, false);
        builder.scalar_field("isbn", ScalarKind::Str, false);
        builder.open_group("details");
        builder.scalar_field("pages", ScalarKind::Int, false);
        builder.close_group();
        builder.open_branch("notes", vec![ScalarKind::Int]);
        builder.scalar_field("text", ScalarKind::Str, true);
        builder.open_branch("tags", vec![ScalarKind::Str]);
        builder.scalar_field("weight", ScalarKind::Int, false);
        builder.close_branch();
        builder.close_branch();
        builder.index(BY_ISBN, true, vec![IndexComponent::field(2)]);
        builder.index(
            BY_TITLE,
            false,
            vec![IndexComponent::field(0), IndexComponent::key(0)],
        );
        let schema = builder.finish().expect("the books schema builds");
        let mut projection = StoreProjection::builder();
        projection.root(schema);
        projection.site(0, SiteTarget::whole_payload());
        projection.site(0, SiteTarget::branch_entry(vec![0]));
        projection.site(0, SiteTarget::branch_entry(vec![0, 0]));
        projection.finish().expect("the sites resolve")
    }

    fn numbers() -> RootNumbering {
        number_store(&projection()).remove(0)
    }

    fn write() -> DemandCoverage {
        DemandCoverage {
            read: true,
            write: true,
        }
    }

    fn s(text: &str) -> KeyScalar {
        KeyScalar::Str(text.into())
    }

    fn vs(text: &str) -> Option<ValueDomain> {
        Some(ValueDomain::Scalar(RuntimeScalar::Str(text.into())))
    }

    fn vi(n: i64) -> Option<ValueDomain> {
        Some(ValueDomain::Scalar(RuntimeScalar::Int(n)))
    }

    fn book(
        title: &str,
        pages: Option<i64>,
        isbn: Option<&str>,
        group_pages: Option<i64>,
    ) -> EntryValue {
        EntryValue {
            fields: vec![vs(title), pages.and_then(vi), isbn.and_then(vs)],
            groups: vec![EntryValue {
                fields: vec![group_pages.and_then(vi)],
                groups: Vec::new(),
            }],
        }
    }

    /// A store populated through the kernel's own ops: two books (one with a note and a
    /// tag under it), so every node kind and both indexes carry cells.
    fn populated() -> DurableStore<MemoryEngine> {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), projection());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn");
            let root = txn.site(0);
            let note = txn.site(1);
            let tag = txn.site(2);
            assert_eq!(
                txn.create_entry(
                    &root,
                    &[s("a")],
                    book("Alpha", Some(10), Some("111"), Some(3))
                )
                .expect("create"),
                CreateOutcome::Created
            );
            assert_eq!(
                txn.create_entry(&root, &[s("b")], book("Beta", None, None, None))
                    .expect("create"),
                CreateOutcome::Created
            );
            assert_eq!(
                txn.create_entry(
                    &note,
                    &[s("a"), KeyScalar::Int(1)],
                    EntryValue {
                        fields: vec![vs("first note")],
                        groups: Vec::new(),
                    },
                )
                .expect("create note"),
                CreateOutcome::Created
            );
            assert_eq!(
                txn.create_entry(
                    &tag,
                    &[s("a"), KeyScalar::Int(1), s("red")],
                    EntryValue {
                        fields: vec![vi(7)],
                        groups: Vec::new(),
                    },
                )
                .expect("create tag"),
                CreateOutcome::Created
            );
            assert!(matches!(txn.commit(), CommitResult::Committed));
        }
        store
    }

    /// A digest that records the cell stream, so a test can compare streams and count cells.
    #[derive(Default)]
    struct Recording {
        cells: Vec<(Vec<u8>, Vec<u8>)>,
    }

    impl ContentDigest for Recording {
        fn absorb(&mut self, key: &[u8], value: &[u8]) {
            self.cells.push((key.to_vec(), value.to_vec()));
        }
    }

    fn audit(store: &DurableStore<MemoryEngine>) -> (AuditReport, Recording) {
        let mut digest = Recording::default();
        let report = store.logical_audit(&mut digest).expect("the walk reads");
        (report, digest)
    }

    fn raw_store(
        projection: StoreProjection,
        cells: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> DurableStore<MemoryEngine> {
        let mut engine = MemoryEngine::new();
        {
            let mut txn = engine.begin().expect("begin");
            for (key, value) in cells {
                txn.put(&key, value).expect("raw cell");
            }
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        DurableStore::from_engine(engine, projection)
    }

    /// Apply raw cell edits to a populated store and reopen it under the same projection.
    fn tamper(
        store: DurableStore<MemoryEngine>,
        edit: impl FnOnce(&mut <MemoryEngine as ByteEngine>::Txn<'_>),
    ) -> DurableStore<MemoryEngine> {
        let mut engine = store.into_engine();
        {
            let mut txn = engine.begin().expect("begin");
            edit(&mut txn);
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        DurableStore::from_engine(engine, projection())
    }

    fn a_stem() -> Vec<u8> {
        physical::marker_key(numbers().root(), &[s("a")])
    }

    fn faults(report: &AuditReport) -> Vec<AuditFault> {
        report.findings.iter().map(|f| f.fault).collect()
    }

    /// Every layout constructor extends the stem it is given, so the suffix a cell carries
    /// below a stem is the same cell built on an empty stem — the identity the walk's
    /// suffix tables rest on.
    #[test]
    fn suffixes_extend_stems() {
        let stem = a_stem();
        for number in [1u32, 7, 65_535] {
            assert_eq!(
                physical::stem_field_leaf(&stem, number),
                [stem.clone(), physical::stem_field_leaf(&[], number)].concat()
            );
            assert_eq!(
                physical::group_stem(&stem, number),
                [stem.clone(), physical::group_stem(&[], number)].concat()
            );
        }
    }

    #[test]
    fn a_populated_store_audits_clean_with_a_stable_content_stream() {
        let store = populated();
        let (report, first) = audit(&store);
        assert!(report.is_clean(), "{:?}", report.findings);
        assert_eq!(
            report.summary,
            AuditSummary {
                // markers 4 + leaves: a(title,pages,isbn,group pages)=4, b(title)=1, note
                // text 1, tag weight 1; the witness and the 3 index cells are not entry cells.
                cells: 4 + 4 + 1 + 1 + 1 + 3 + 1,
                entries: 4,
                index_cells: 3,
                findings: 0,
            }
        );
        let (_, second) = audit(&store);
        assert_eq!(first.cells, second.cells, "the stream is deterministic");
        assert_eq!(first.cells.len(), 11, "entry-family cells only");
        assert!(
            first.cells.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "the stream is in key order"
        );

        // One committed write changes the stream.
        let mut store = store;
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn");
            let root = txn.site(0);
            txn.create_entry(&root, &[s("c")], book("Gamma", None, None, None))
                .expect("create");
            assert!(matches!(txn.commit(), CommitResult::Committed));
        }
        let (report, third) = audit(&store);
        assert!(report.is_clean());
        assert_ne!(first.cells, third.cells);
    }

    #[test]
    fn an_own_leaf_without_a_marker_is_an_orphan_named_by_its_node() {
        let numbers = numbers();
        let store = tamper(populated(), |txn| {
            let stem = physical::marker_key(0, &[s("z")]);
            txn.put(
                &physical::stem_field_leaf(&stem, numbers.fields()[0]),
                b"Zed".to_vec(),
            )
            .expect("put");
            txn.put(
                &physical::stem_field_leaf(&stem, numbers.fields()[1]),
                b"2".to_vec(),
            )
            .expect("second own leaf");
            let group = &numbers.groups()[0];
            txn.put(
                &physical::stem_field_leaf(
                    &physical::group_stem(&stem, group.number()),
                    group.fields()[0],
                ),
                b"3".to_vec(),
            )
            .expect("group leaf");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![AuditFinding {
                fault: AuditFault::OrphanLeaf,
                site: AuditSite::Node {
                    root: 0,
                    branch: Vec::new(),
                    keys: vec![s("z")],
                },
            }]
        );
        // The orphan's leaf still counts as a cell, never as an entry.
        assert_eq!(report.summary.entries, 4);
    }

    #[test]
    fn a_branch_leaf_without_its_marker_is_an_orphan_under_a_present_parent() {
        let numbers = numbers();
        let notes = &numbers.branches()[0];
        let store = tamper(populated(), |txn| {
            let note = physical::marker_key(notes.number(), &[s("a"), KeyScalar::Int(9)]);
            txn.put(
                &physical::stem_field_leaf(&note, notes.fields()[0]),
                b"x".to_vec(),
            )
            .expect("put");
        });
        let (report, _) = audit(&store);
        assert_eq!(faults(&report), vec![AuditFault::OrphanLeaf]);
        assert_eq!(
            report.findings[0].site,
            AuditSite::Node {
                root: 0,
                branch: vec![0],
                keys: vec![s("a"), KeyScalar::Int(9)],
            }
        );
    }

    #[test]
    fn a_dangling_unique_index_cell_and_a_stale_cell_are_distinct_findings() {
        let root = numbers().root();
        let store = tamper(populated(), |txn| {
            // An index cell for isbn "999" naming a book that does not exist.
            txn.put(
                &physical::index_cell_key(root, &BY_ISBN, &[s("999")]),
                physical::index_cell_value(&[s("nobody")]),
            )
            .expect("put");
            // An index cell for isbn "222" naming book "a", whose isbn is "111".
            txn.put(
                &physical::index_cell_key(root, &BY_ISBN, &[s("222")]),
                physical::index_cell_value(&[s("a")]),
            )
            .expect("put");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![
                AuditFinding {
                    fault: AuditFault::IndexStale,
                    site: AuditSite::IndexCell {
                        root: 0,
                        index: 0,
                        values: vec![s("222")],
                    },
                },
                AuditFinding {
                    fault: AuditFault::IndexOrphan,
                    site: AuditSite::IndexCell {
                        root: 0,
                        index: 0,
                        values: vec![s("999")],
                    },
                },
            ]
        );
    }

    #[test]
    fn an_empty_physical_key_is_reported_and_counted() {
        let baseline = audit(&populated()).0.summary.cells;
        let store = tamper(populated(), |txn| {
            txn.put(&[], b"foreign".to_vec()).expect("empty key");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![AuditFinding {
                fault: AuditFault::OutsideSchema,
                site: AuditSite::Cell { key: Vec::new() },
            }]
        );
        assert_eq!(report.summary.cells, baseline + 1);
    }

    #[test]
    fn temporal_entry_keys_must_be_in_the_language_domain() {
        for key in [KeyScalar::Date(i32::MAX), KeyScalar::Instant(i128::MAX)] {
            let mut schema = StoreSchemaBuilder::root("dated", vec![key.scalar_kind()]);
            schema.scalar_field("title", ScalarKind::Str, true);
            let mut projection = StoreProjection::builder();
            projection.root(schema.finish().expect("schema"));
            let projection = projection.finish().expect("projection");
            let numbers = number_store(&projection).remove(0);
            let stem = physical::marker_key(numbers.root(), &[key]);
            let mut engine = MemoryEngine::new();
            {
                let mut txn = engine.begin().expect("begin");
                txn.put(&stem, physical::MARKER_VALUE.to_vec())
                    .expect("marker");
                txn.put(
                    &physical::stem_field_leaf(&stem, numbers.fields()[0]),
                    b"valid title".to_vec(),
                )
                .expect("payload");
                assert_eq!(txn.commit(), CommitOutcome::Confirmed);
            }
            let store = DurableStore::from_engine(engine, projection);
            let (report, _) = audit(&store);
            assert!(faults(&report).contains(&AuditFault::Undecodable));
            assert_eq!(report.summary.cells, 2);
        }
    }

    #[test]
    fn temporal_branch_and_index_keys_use_the_same_domain_check() {
        for (valid, invalid) in [
            (KeyScalar::Date(0), KeyScalar::Date(i32::MAX)),
            (KeyScalar::Instant(0), KeyScalar::Instant(i128::MAX)),
        ] {
            let mut schema = StoreSchemaBuilder::root("dated", vec![valid.scalar_kind()]);
            schema.scalar_field("title", ScalarKind::Str, true);
            schema.open_branch("edits", vec![valid.scalar_kind()]);
            schema.scalar_field("title", ScalarKind::Str, true);
            schema.close_branch();
            schema.index(BY_ISBN, true, vec![IndexComponent::key(0)]);
            let mut projection = StoreProjection::builder();
            projection.root(schema.finish().expect("schema"));
            let projection = projection.finish().expect("projection");
            let numbers = number_store(&projection).remove(0);
            let root = numbers.root();
            let branch = &numbers.branches()[0];
            let child = physical::marker_key(branch.number(), &[valid.clone(), invalid.clone()]);
            let cases = [
                (
                    "branch",
                    vec![
                        (child.clone(), physical::MARKER_VALUE.to_vec()),
                        (
                            physical::stem_field_leaf(&child, branch.fields()[0]),
                            b"valid title".to_vec(),
                        ),
                    ],
                ),
                (
                    "index key",
                    vec![(
                        physical::index_cell_key(root, &BY_ISBN, std::slice::from_ref(&invalid)),
                        physical::index_cell_value(std::slice::from_ref(&valid)),
                    )],
                ),
                (
                    "index source",
                    vec![(
                        physical::index_cell_key(root, &BY_ISBN, std::slice::from_ref(&valid)),
                        physical::index_cell_value(std::slice::from_ref(&invalid)),
                    )],
                ),
            ];
            for (name, cells) in cases {
                let count = cells.len();
                let mut engine = MemoryEngine::new();
                {
                    let mut txn = engine.begin().expect("begin");
                    for (key, value) in cells {
                        txn.put(&key, value).expect("raw cell");
                    }
                    assert_eq!(txn.commit(), CommitOutcome::Confirmed);
                }
                let store = DurableStore::from_engine(engine, projection.clone());
                let (report, _) = audit(&store);
                assert_eq!(
                    faults(&report),
                    vec![AuditFault::Undecodable; count],
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn a_unique_index_cell_cannot_represent_two_present_entries() {
        let numbers = numbers();
        let store = tamper(populated(), |txn| {
            let stem = physical::marker_key(numbers.root(), &[s("b")]);
            txn.put(
                &physical::stem_field_leaf(&stem, numbers.fields()[2]),
                b"111".to_vec(),
            )
            .expect("duplicate indexed value");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![AuditFinding {
                fault: AuditFault::IndexMissing,
                site: AuditSite::IndexCell {
                    root: 0,
                    index: 0,
                    values: vec![s("111")],
                },
            }]
        );
    }

    #[test]
    fn an_entry_whose_row_is_absent_is_an_index_missing_finding() {
        let root = numbers().root();
        let store = tamper(populated(), |txn| {
            txn.remove(&physical::index_cell_key(
                root,
                &BY_TITLE,
                &[s("Beta"), s("b")],
            ))
            .expect("remove");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![AuditFinding {
                fault: AuditFault::IndexMissing,
                site: AuditSite::IndexCell {
                    root: 0,
                    index: 1,
                    values: vec![s("Beta"), s("b")],
                },
            }]
        );
        assert_eq!(report.summary.index_cells, 2);
    }

    #[test]
    fn a_present_entry_missing_a_required_leaf_is_named_by_its_field() {
        let numbers = numbers();
        let notes = &numbers.branches()[0];
        let store = tamper(populated(), |txn| {
            txn.remove(&physical::stem_field_leaf(&a_stem(), numbers.fields()[0]))
                .expect("remove title");
            let note = physical::marker_key(notes.number(), &[s("a"), KeyScalar::Int(1)]);
            txn.remove(&physical::stem_field_leaf(&note, notes.fields()[0]))
                .expect("remove text");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![
                AuditFinding {
                    fault: AuditFault::RequiredMissing,
                    site: AuditSite::Field {
                        root: 0,
                        branch: Vec::new(),
                        keys: vec![s("a")],
                        group: None,
                        field: 0,
                    },
                },
                AuditFinding {
                    fault: AuditFault::RequiredMissing,
                    site: AuditSite::Field {
                        root: 0,
                        branch: vec![0],
                        keys: vec![s("a"), KeyScalar::Int(1)],
                        group: None,
                        field: 0,
                    },
                },
                // Book "a" no longer has a title, so its title index cell is stale.
                AuditFinding {
                    fault: AuditFault::IndexStale,
                    site: AuditSite::IndexCell {
                        root: 0,
                        index: 1,
                        values: vec![s("Alpha"), s("a")],
                    },
                },
            ]
        );
    }

    #[test]
    fn an_undecodable_leaf_is_named_by_its_field_including_a_group_field() {
        let numbers = numbers();
        let group = &numbers.groups()[0];
        let store = tamper(populated(), |txn| {
            txn.put(
                &physical::stem_field_leaf(&a_stem(), numbers.fields()[1]),
                b"1x".to_vec(),
            )
            .expect("put pages");
            let group_stem = physical::group_stem(&a_stem(), group.number());
            txn.put(
                &physical::stem_field_leaf(&group_stem, group.fields()[0]),
                b"".to_vec(),
            )
            .expect("put group pages");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![
                AuditFinding {
                    fault: AuditFault::Undecodable,
                    site: AuditSite::Field {
                        root: 0,
                        branch: Vec::new(),
                        keys: vec![s("a")],
                        group: None,
                        field: 1,
                    },
                },
                AuditFinding {
                    fault: AuditFault::Undecodable,
                    site: AuditSite::Field {
                        root: 0,
                        branch: Vec::new(),
                        keys: vec![s("a")],
                        group: Some(0),
                        field: 0,
                    },
                },
            ]
        );
    }

    #[test]
    fn cells_outside_the_schema_are_named_by_their_raw_key() {
        let numbers = numbers();
        let foreign_field = physical::stem_field_leaf(&a_stem(), 4000);
        let foreign_branch = physical::marker_key(4001, &[s("a"), KeyScalar::Int(1)]);
        let foreign_root = physical::marker_key(4002, &[s("q")]);
        let foreign_group = physical::stem_field_leaf(&physical::group_stem(&a_stem(), 4003), 4004);
        let field_as_family = physical::marker_key(numbers.fields()[0], &[s("a")]);
        let group_as_family = physical::marker_key(numbers.groups()[0].number(), &[s("a")]);
        let branch_as_index_root =
            physical::index_cell_key(numbers.branches()[0].number(), &BY_ISBN, &[s("x")]);
        let foreign_index = physical::index_cell_key(numbers.root(), &[0xEE; 16], &[s("x")]);
        let foreign_meta = physical::meta_key("profile");
        let cells = [
            foreign_field.clone(),
            foreign_branch.clone(),
            foreign_root.clone(),
            foreign_group.clone(),
            field_as_family,
            group_as_family,
            branch_as_index_root,
            foreign_index.clone(),
            foreign_meta.clone(),
        ];
        let store = tamper(populated(), |txn| {
            for cell in &cells {
                txn.put(cell, vec![0x01]).expect("put");
            }
        });
        let (report, digest) = audit(&store);
        assert!(
            faults(&report)
                .iter()
                .all(|fault| *fault == AuditFault::OutsideSchema),
            "{:?}",
            report.findings
        );
        let mut named: Vec<Vec<u8>> = report
            .findings
            .iter()
            .filter_map(|finding| match &finding.site {
                AuditSite::Cell { key } => Some(key.clone()),
                AuditSite::UndeclaredIndex { root: 0, id } => {
                    assert_eq!(*id, [0xEE; 16], "the undeclared index is named by its id");
                    None
                }
                other => panic!("an outside-schema cell is named by its key: {other:?}"),
            })
            .collect();
        named.sort();
        let mut expected = cells.to_vec();
        expected.retain(|cell| *cell != foreign_index);
        expected.sort();
        assert_eq!(named, expected);
        // The commit witness is a declared meta cell and never a finding.
        assert_eq!(report.summary.findings, 9);
        let mut expected_digest = audit(&populated()).1.cells;
        expected_digest.push((foreign_field, vec![0x01]));
        expected_digest.push((foreign_group, vec![0x01]));
        expected_digest.sort();
        assert_eq!(
            digest.cells, expected_digest,
            "unknown families, indexes and metadata are excluded"
        );
    }

    /// The commit witness is the one meta cell the walk admits, and only in a shape the
    /// next transaction would accept.
    #[test]
    fn a_witness_of_a_foreign_shape_is_a_typed_finding() {
        let witness = physical::meta_key(WITNESS);
        let store = tamper(populated(), |txn| {
            txn.put(&witness, vec![0x02; 17]).expect("put");
        });
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![AuditFinding {
                fault: AuditFault::WitnessInvalid,
                site: AuditSite::Cell { key: witness },
            }]
        );
    }

    #[test]
    fn a_marker_with_a_foreign_value_is_invalid() {
        let store = tamper(populated(), |txn| {
            txn.put(&a_stem(), vec![0x02]).expect("put");
        });
        let (report, _) = audit(&store);
        assert_eq!(faults(&report), vec![AuditFault::MarkerInvalid]);
        assert_eq!(
            report.findings[0].site,
            AuditSite::Node {
                root: 0,
                branch: Vec::new(),
                keys: vec![s("a")],
            }
        );
    }

    /// Only concrete entry markers count; absent ancestors neither add entries nor findings.
    #[test]
    fn a_child_with_two_absent_ancestors_is_audited_in_its_own_family() {
        let numbers = numbers();
        let notes = &numbers.branches()[0];
        let tags = &notes.branches()[0];
        let store = tamper(populated(), |txn| {
            let tag = physical::marker_key(tags.number(), &[s("z"), KeyScalar::Int(1), s("blue")]);
            txn.put(&tag, physical::MARKER_VALUE.to_vec())
                .expect("put tag");
            txn.put(
                &physical::stem_field_leaf(&tag, tags.fields()[0]),
                b"5".to_vec(),
            )
            .expect("put weight");
        });
        let (report, digest) = audit(&store);
        assert!(report.is_clean(), "{:?}", report.findings);
        assert_eq!(report.summary.entries, 5, "the tag is a present entry");
        let tag = physical::marker_key(tags.number(), &[s("z"), KeyScalar::Int(1), s("blue")]);
        let child_cells: Vec<_> = digest
            .cells
            .iter()
            .filter(|(key, _)| key.starts_with(&tag))
            .cloned()
            .collect();
        assert_eq!(
            child_cells,
            vec![
                (tag.clone(), physical::MARKER_VALUE.to_vec()),
                (
                    physical::stem_field_leaf(&tag, tags.fields()[0]),
                    b"5".to_vec()
                ),
            ]
        );
        assert_eq!(digest.cells.len(), 13);
    }

    #[test]
    fn findings_past_the_cap_are_counted_but_not_retained() {
        let numbers = numbers();
        let branch = &numbers.branches()[0];
        let per_family = (MAX_REPORTED_FINDINGS + 40) / 2;
        let store = tamper(populated(), |txn| {
            for i in 0..per_family {
                let stem = physical::marker_key(0, &[s(&format!("orphan-{i:04}"))]);
                txn.put(
                    &physical::stem_field_leaf(&stem, numbers.fields()[0]),
                    b"x".to_vec(),
                )
                .expect("put");
                let child =
                    physical::marker_key(branch.number(), &[s("absent"), KeyScalar::Int(i as i64)]);
                txn.put(
                    &physical::stem_field_leaf(&child, branch.fields()[0]),
                    b"x".to_vec(),
                )
                .expect("child leaf");
            }
        });
        let (report, digest) = audit(&store);
        assert_eq!(report.summary.findings, (MAX_REPORTED_FINDINGS + 40) as u64);
        assert_eq!(report.findings.len(), MAX_REPORTED_FINDINGS);
        assert!(
            faults(&report)
                .iter()
                .all(|fault| *fault == AuditFault::OrphanLeaf)
        );
        assert_eq!(
            report.findings[per_family - 1].site,
            AuditSite::Node {
                root: 0,
                branch: Vec::new(),
                keys: vec![s(&format!("orphan-{:04}", per_family - 1))],
            }
        );
        assert_eq!(
            report.findings[per_family].site,
            AuditSite::Node {
                root: 0,
                branch: vec![0],
                keys: vec![s("absent"), KeyScalar::Int(0)],
            }
        );
        assert_eq!(
            report.findings.last().expect("capped report").site,
            AuditSite::Node {
                root: 0,
                branch: vec![0],
                keys: vec![
                    s("absent"),
                    KeyScalar::Int((MAX_REPORTED_FINDINGS - per_family - 1) as i64)
                ],
            }
        );
        assert_eq!(
            digest.cells.len(),
            11 + MAX_REPORTED_FINDINGS + 40,
            "the finding cap does not cap the content stream"
        );
    }

    /// An index cell whose value is not a whole source key tuple is undecodable, not stale.
    #[test]
    fn an_index_cell_with_a_malformed_source_key_is_undecodable() {
        let root = numbers().root();
        let store = tamper(populated(), |txn| {
            let mut value = encode_key_tuple(&[s("a")]);
            value.push(0xAB);
            txn.put(
                &physical::index_cell_key(root, &BY_ISBN, &[s("333")]),
                value,
            )
            .expect("put");
        });
        let (report, _) = audit(&store);
        assert_eq!(faults(&report), vec![AuditFault::Undecodable]);
        assert_eq!(
            report.findings[0].site,
            AuditSite::IndexCell {
                root: 0,
                index: 0,
                values: vec![s("333")],
            }
        );
    }

    #[test]
    fn family_key_segments_borrow_the_projection_through_the_admitted_depth() {
        let mut schema = StoreSchemaBuilder::root("root", vec![ScalarKind::Str, ScalarKind::Int]);
        for depth in 0..16 {
            schema.open_branch(
                format!("child{depth}"),
                vec![ScalarKind::Int, ScalarKind::Str],
            );
        }
        for _ in 0..16 {
            schema.close_branch();
        }
        let mut projection = StoreProjection::builder();
        projection.root(schema.finish().expect("sixteen branch hops"));
        let projection = projection.finish().expect("projection");
        let numbering = number_store(&projection);
        let tables = Tables::new(&projection, &numbering);
        assert_eq!(tables.families.len(), 17);
        assert_eq!(tables.roots[0].family, 0);
        assert!(
            tables
                .families
                .windows(2)
                .all(|pair| pair[0].prefix < pair[1].prefix)
        );
        let root = &projection.roots()[0];
        let mut expected = vec![root.key()];
        let mut branches = root.branches();
        for (depth, family) in tables.families.iter().enumerate() {
            assert_eq!(family.root, 0);
            assert_eq!(family.branch_path, vec![0; depth]);
            assert_eq!(family.key_segments.len(), depth + 1);
            for (borrowed, original) in family.key_segments.iter().zip(&expected) {
                assert!(
                    std::ptr::eq(*borrowed, *original),
                    "kind slices retain projection identity"
                );
            }
            if let Some(branch) = branches.first() {
                expected.push(branch.key());
                branches = branch.branches();
            }
        }
    }

    fn composite_child() -> (StoreProjection, Vec<KeyScalar>) {
        let mut schema = StoreSchemaBuilder::root("root", vec![ScalarKind::Str, ScalarKind::Int]);
        schema.open_branch("parent", vec![ScalarKind::Str, ScalarKind::Date]);
        schema.open_branch("child", vec![ScalarKind::Int, ScalarKind::Str]);
        schema.scalar_field("title", ScalarKind::Str, true);
        schema.scalar_field("count", ScalarKind::Int, false);
        schema.close_branch().close_branch();
        let mut projection = StoreProjection::builder();
        projection.root(schema.finish().expect("composite schema"));
        (
            projection.finish().expect("projection"),
            vec![
                s("root\0key"),
                KeyScalar::Int(11),
                s("ancestor\0key"),
                KeyScalar::Date(3),
                KeyScalar::Int(22),
                s("own\0key"),
            ],
        )
    }

    #[test]
    fn the_open_entry_keeps_its_complete_decoded_tuple_across_own_leaves() {
        let (projection, keys) = composite_child();
        let numbering = number_store(&projection);
        let child = &numbering[0].branches()[0].branches()[0];
        let stem = physical::marker_key(child.number(), &keys);
        let cells = vec![
            (stem.clone(), physical::MARKER_VALUE.to_vec()),
            (
                physical::stem_field_leaf(&stem, child.fields()[0]),
                b"valid".to_vec(),
            ),
            (
                physical::stem_field_leaf(&stem, child.fields()[1]),
                b"7".to_vec(),
            ),
        ];
        let store = raw_store(projection.clone(), cells.clone());
        let (report, digest) = audit(&store);
        assert!(report.is_clean(), "{:?}", report.findings);
        assert_eq!(report.summary.entries, 1, "neither ancestor needs a marker");
        assert_eq!(digest.cells, cells);

        let tables = Tables::new(&projection, &numbering);
        let engine = MemoryEngine::new();
        let view = engine.read_view().expect("view");
        let mut digest = Recording::default();
        let mut walker = Walker {
            view: &view,
            tables: &tables,
            digest: &mut digest,
            frame: None,
            family_cursor: 0,
            index_cursor: 0,
            index_family_cursor: 0,
            summary: AuditSummary::default(),
            findings: Vec::new(),
        };
        walker.cell(&cells[0].0, &cells[0].1).expect("marker");
        let frame = walker.frame.as_ref().expect("one open entry");
        assert_eq!(frame.family, 2);
        assert_eq!(frame.keys, keys);
        let decoded = frame.keys.as_ptr();
        for (key, value) in &cells[1..] {
            walker.cell(key, value).expect("own leaf");
            assert_eq!(
                walker.frame.as_ref().expect("same entry").keys.as_ptr(),
                decoded
            );
        }
        walker.close_entry().expect("close");
        assert!(walker.frame.is_none());
        assert!(walker.findings.is_empty());

        let orphan = raw_store(projection, cells[1..].to_vec());
        let (report, _) = audit(&orphan);
        assert_eq!(
            report.findings,
            vec![AuditFinding {
                fault: AuditFault::OrphanLeaf,
                site: AuditSite::Node {
                    root: 0,
                    branch: vec![0, 0],
                    keys
                },
            }]
        );
        assert_eq!(report.summary.entries, 0);
    }

    #[test]
    fn malformed_full_key_paths_are_reported_and_digested_in_the_child_family() {
        let (projection, keys) = composite_child();
        let numbering = number_store(&projection);
        let child = &numbering[0].branches()[0].branches()[0];
        let marker = physical::marker_key(child.number(), &keys);
        let mut wrong_root = keys.clone();
        wrong_root[1] = s("wrong root kind");
        let mut wrong_ancestor = keys.clone();
        wrong_ancestor[3] = s("wrong later ancestor kind");
        let mut invalid_ancestor = keys.clone();
        invalid_ancestor[3] = KeyScalar::Date(i32::MAX);
        let mut wrong_own = keys.clone();
        wrong_own[4] = s("wrong own kind");
        let mut extra = keys.clone();
        extra.push(KeyScalar::Int(99));
        let mut missing_terminator = marker.clone();
        missing_terminator.pop();
        let mut wrong_terminator = marker;
        *wrong_terminator.last_mut().expect("marker terminator") = 0x7F;
        let mut invalid_utf8 = physical::entry_family_prefix(child.number());
        invalid_utf8.extend(encode_key_tuple(&keys[..2]));
        invalid_utf8.extend([crate::codec::key::KEY_STR, 0xFE, 0, 0]);
        invalid_utf8.extend(encode_key_tuple(&keys[3..]));
        invalid_utf8.push(0);
        let cases = [
            (
                "root kind",
                physical::marker_key(child.number(), &wrong_root),
            ),
            (
                "later ancestor kind",
                physical::marker_key(child.number(), &wrong_ancestor),
            ),
            (
                "later ancestor domain",
                physical::marker_key(child.number(), &invalid_ancestor),
            ),
            ("own kind", physical::marker_key(child.number(), &wrong_own)),
            (
                "short tuple",
                physical::marker_key(child.number(), &keys[..5]),
            ),
            ("extra column", physical::marker_key(child.number(), &extra)),
            ("missing terminator", missing_terminator),
            ("wrong terminator", wrong_terminator),
            ("ancestor UTF-8", invalid_utf8),
        ];
        for (name, stem) in cases {
            let cells = vec![
                (stem.clone(), physical::MARKER_VALUE.to_vec()),
                (
                    physical::stem_field_leaf(&stem, child.fields()[0]),
                    b"valid".to_vec(),
                ),
            ];
            let store = raw_store(projection.clone(), cells.clone());
            let (report, digest) = audit(&store);
            let expected: Vec<_> = cells
                .iter()
                .map(|(key, _)| AuditFinding {
                    fault: AuditFault::Undecodable,
                    site: AuditSite::Cell { key: key.clone() },
                })
                .collect();
            assert_eq!(report.findings, expected, "{name}");
            assert_eq!(report.summary.entries, 0, "{name}");
            assert_eq!(report.summary.cells, 2, "{name}");
            assert_eq!(
                digest.cells, cells,
                "{name}: declared-family corruption remains in the digest"
            );
        }
    }

    #[test]
    fn required_fields_close_in_entry_family_and_eof_order() {
        let mut schema = StoreSchemaBuilder::root("root", vec![ScalarKind::Str]);
        schema.scalar_field("title", ScalarKind::Str, true);
        schema.open_group("group");
        schema.scalar_field("code", ScalarKind::Int, true);
        schema.close_group();
        schema.open_branch("child", vec![ScalarKind::Int]);
        schema.scalar_field("title", ScalarKind::Str, true);
        schema.close_branch();
        let mut projection = StoreProjection::builder();
        projection.root(schema.finish().expect("schema"));
        let projection = projection.finish().expect("projection");
        let numbering = number_store(&projection);
        let root = &numbering[0];
        let child = &root.branches()[0];
        let cells = vec![
            (
                physical::marker_key(root.root(), &[s("a")]),
                physical::MARKER_VALUE.to_vec(),
            ),
            (
                physical::marker_key(root.root(), &[s("b")]),
                physical::MARKER_VALUE.to_vec(),
            ),
            (
                physical::marker_key(child.number(), &[s("absent"), KeyScalar::Int(7)]),
                physical::MARKER_VALUE.to_vec(),
            ),
        ];
        let store = raw_store(projection, cells.clone());
        let (report, digest) = audit(&store);
        let mut expected = Vec::new();
        for key in [s("a"), s("b")] {
            for group in [None, Some(0)] {
                expected.push(AuditFinding {
                    fault: AuditFault::RequiredMissing,
                    site: AuditSite::Field {
                        root: 0,
                        branch: Vec::new(),
                        keys: vec![key.clone()],
                        group,
                        field: 0,
                    },
                });
            }
        }
        expected.push(AuditFinding {
            fault: AuditFault::RequiredMissing,
            site: AuditSite::Field {
                root: 0,
                branch: vec![0],
                keys: vec![s("absent"), KeyScalar::Int(7)],
                group: None,
                field: 0,
            },
        });
        assert_eq!(report.findings, expected);
        assert_eq!(report.summary.entries, 3);
        assert_eq!(digest.cells, cells);
    }

    #[test]
    fn index_sorting_preserves_multiple_root_ownership_and_declaration_positions() {
        let mut projection = StoreProjection::builder();
        for name in ["first", "second"] {
            let mut schema = StoreSchemaBuilder::root(name, vec![ScalarKind::Str]);
            schema.scalar_field("title", ScalarKind::Str, true);
            schema.open_branch("child", vec![ScalarKind::Int]);
            schema.close_branch();
            schema.index(BY_TITLE, true, vec![IndexComponent::field(0)]);
            schema.index(
                BY_ISBN,
                true,
                vec![IndexComponent::field(0), IndexComponent::key(0)],
            );
            projection.root(schema.finish().expect("schema"));
        }
        projection.site(0, SiteTarget::whole_payload());
        projection.site(1, SiteTarget::whole_payload());
        let projection = projection.finish().expect("projection");
        let numbering = number_store(&projection);
        let tables = Tables::new(&projection, &numbering);
        assert_eq!(
            tables
                .roots
                .iter()
                .map(|root| root.family)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(
            tables
                .indexes
                .iter()
                .map(|index| (index.root, index.position))
                .collect::<Vec<_>>(),
            vec![(0, 1), (0, 0), (1, 1), (1, 0)]
        );
        let mut store = DurableStore::from_engine(MemoryEngine::new(), projection.clone());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn");
            for position in 0..2 {
                let site = txn.site(position);
                assert_eq!(
                    txn.create_entry(
                        &site,
                        &[s("id")],
                        EntryValue {
                            fields: vec![vs("title")],
                            groups: Vec::new(),
                        }
                    )
                    .expect("create"),
                    CreateOutcome::Created
                );
            }
            assert!(matches!(txn.commit(), CommitResult::Committed));
        }
        let (report, _) = audit(&store);
        assert!(report.is_clean(), "{:?}", report.findings);
        assert_eq!(report.summary.index_cells, 4);
        let mut engine = store.into_engine();
        {
            let mut txn = engine.begin().expect("begin");
            txn.remove(&physical::index_cell_key(
                numbering[0].root(),
                &BY_TITLE,
                &[s("title")],
            ))
            .expect("first root's declared index zero");
            txn.remove(&physical::index_cell_key(
                numbering[1].root(),
                &BY_ISBN,
                &[s("title"), s("id")],
            ))
            .expect("second root's declared index one");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let store = DurableStore::from_engine(engine, projection);
        let (report, _) = audit(&store);
        assert_eq!(
            report.findings,
            vec![
                AuditFinding {
                    fault: AuditFault::IndexMissing,
                    site: AuditSite::IndexCell {
                        root: 0,
                        index: 0,
                        values: vec![s("title")]
                    }
                },
                AuditFinding {
                    fault: AuditFault::IndexMissing,
                    site: AuditSite::IndexCell {
                        root: 1,
                        index: 1,
                        values: vec![s("title"), s("id")]
                    }
                },
            ]
        );
        assert_eq!(report.summary.index_cells, 2);
    }
}
