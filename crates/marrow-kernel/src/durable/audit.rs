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

/// A fallible consumer of canonical entry and managed-index cells. Cells are
/// provisional until the complete audit report is clean; a later finding can
/// invalidate bytes already delivered. Commit witnesses are never delivered.
pub trait ExportSink {
    fn cell(&mut self, key: &[u8], value: &[u8]) -> std::io::Result<()>;
}

/// Export preserves the distinction between a failed store read and failed output.
#[derive(Debug)]
pub enum ExportError {
    Read(super::SessionError),
    Output(std::io::Error),
}

pub(super) enum WalkError<E> {
    Store(StoreError),
    Consumer(E),
}

impl<E> From<StoreError> for WalkError<E> {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

#[derive(Clone, Copy)]
enum CellKind {
    Entry,
    Index,
    Other,
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
    inspect(view, &Tables::new(projection, numbering), digest)
}

pub(super) fn inspect<V: ReadView>(
    view: &V,
    tables: &Tables<'_>,
    digest: &mut dyn ContentDigest,
) -> Result<AuditReport, StoreError> {
    match walk_cells(view, tables, digest, |_, _, _| {
        Ok::<_, std::convert::Infallible>(())
    }) {
        Ok(report) => Ok(report),
        Err(WalkError::Store(error)) => Err(error),
        Err(WalkError::Consumer(impossible)) => match impossible {},
    }
}

pub(super) fn export<V: ReadView>(
    view: &V,
    projection: &StoreProjection,
    numbering: &[RootNumbering],
    digest: &mut dyn ContentDigest,
    sink: &mut dyn ExportSink,
) -> Result<AuditReport, WalkError<std::io::Error>> {
    walk_cells(
        view,
        &Tables::new(projection, numbering),
        digest,
        |kind, key, value| match kind {
            CellKind::Entry | CellKind::Index => sink.cell(key, value),
            CellKind::Other => Ok(()),
        },
    )
}

fn walk_cells<V: ReadView, E>(
    view: &V,
    tables: &Tables<'_>,
    digest: &mut dyn ContentDigest,
    mut consume: impl FnMut(CellKind, &[u8], &[u8]) -> Result<(), E>,
) -> Result<AuditReport, WalkError<E>> {
    let mut walker = Walker {
        view,
        tables,
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
        let kind = walker.cell(&[], &value)?;
        consume(kind, &[], &value).map_err(WalkError::Consumer)?;
    }
    let mut cursor: Vec<u8> = Vec::new();
    loop {
        let page = view.scan_after(&[], &cursor)?;
        let Some((last, _)) = page.last() else {
            break;
        };
        cursor = last.clone();
        for (key, value) in &page {
            let kind = walker.cell(key, value)?;
            consume(kind, key, value).map_err(WalkError::Consumer)?;
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
pub(super) struct Tables<'a> {
    /// Every entry family in store-number order, which is also physical prefix order.
    families: Vec<FamilyShape<'a>>,
    /// Every root in declaration order, for managed-index source identity.
    roots: Vec<RootShape>,
    /// Every index of every root, sorted by family prefix (the order their cells appear).
    indexes: Vec<IndexShape>,
    witness: Vec<u8>,
}

impl<'a> Tables<'a> {
    pub(super) fn new(projection: &'a StoreProjection, numbering: &[RootNumbering]) -> Self {
        let mut families = Vec::new();
        let mut indexes = Vec::new();
        let mut roots = Vec::with_capacity(projection.roots().len());
        for (position, (schema, numbers)) in projection.roots().iter().zip(numbering).enumerate() {
            for (index_pos, index) in schema.indexes().iter().enumerate() {
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
                indexes: Vec::with_capacity(schema.indexes().len()),
            });
        }
        // Physical addresses need not follow declaration order. Resolve table references
        // after sorting while retaining declaration positions for semantic identities.
        families.sort_by(|a, b| a.prefix.cmp(&b.prefix));
        for (position, family) in families.iter().enumerate() {
            if family.branch_path.is_empty() {
                roots[family.root].family = position;
            }
        }
        indexes.sort_by(|a, b| a.prefix.cmp(&b.prefix));
        for (position, index) in indexes.iter().enumerate() {
            roots[usize::from(index.root)].indexes.push(position);
        }
        Self {
            families,
            roots,
            indexes,
            witness: physical::meta_key(WITNESS),
        }
    }

    pub(super) fn namespace(
        &self,
        key: &[u8],
        family_cursor: &mut usize,
        index_cursor: &mut usize,
    ) -> Option<Namespace> {
        if let Some(family) = seek_prefix(
            &self.families,
            |family| family.prefix.as_slice(),
            family_cursor,
            key,
        ) {
            return Some(Namespace::Entry(family));
        }
        seek_prefix(
            &self.indexes,
            |index| index.prefix.as_slice(),
            index_cursor,
            key,
        )
        .map(Namespace::Index)
    }
}

pub(super) enum Namespace {
    Entry(usize),
    Index(usize),
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
    fn cell(&mut self, key: &[u8], value: &[u8]) -> Result<CellKind, StoreError> {
        self.summary.cells += 1;
        if self
            .frame
            .as_ref()
            .is_some_and(|frame| key.starts_with(&frame.stem))
        {
            self.digest.absorb(key, value);
            self.within_node(key, value);
            Ok(CellKind::Entry)
        } else {
            self.close_entry()?;
            self.top_level(key, value)
        }
    }

    /// Classify a cell under no open entry: a declared entry family, a managed-index cell,
    /// the commit witness, or a cell outside every declared family.
    fn top_level(&mut self, key: &[u8], value: &[u8]) -> Result<CellKind, StoreError> {
        let tables = self.tables;
        match tables.namespace(key, &mut self.family_cursor, &mut self.index_cursor) {
            Some(Namespace::Entry(family)) => {
                self.digest.absorb(key, value);
                self.enter_entry(family, key, value);
                return Ok(CellKind::Entry);
            }
            Some(Namespace::Index(index)) => {
                self.index_cell(&tables.indexes[index], key, value)?;
                return Ok(CellKind::Index);
            }
            None => {}
        }
        if key == tables.witness.as_slice() {
            if !witness_well_formed(value) {
                self.finding(
                    AuditFault::WitnessInvalid,
                    AuditSite::Cell { key: key.to_vec() },
                );
            }
            return Ok(CellKind::Other);
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
        Ok(CellKind::Other)
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
#[path = "audit_tests.rs"]
mod tests;
