//! One bounded, read-only walk over every cell of a store against its admitted projection.
//!
//! The walk reads the whole key space forward, one engine page at a time, and parses the
//! cell stream in the order the layout (`physical.rs`) sorts it: each declared root's entry
//! family — every entry's marker, then its own field leaves, its group leaves, and its
//! branch descendants nested to any depth — then the managed-index families, then the meta
//! family. Its retained state is a stack of open nodes, one frame per nesting level and so
//! bounded by [`MAX_DURABLE_DEPTH`](super::MAX_DURABLE_DEPTH), plus fixed per-schema tables
//! and a capped finding list; nothing grows with the number of cells. Every cell is
//! classified exactly once, and a cell that belongs to no declared node or index is a
//! typed finding rather than a skipped byte. A required leaf that is absent is found when
//! the node it belongs to closes, so a missing cell is reported as precisely as a present
//! one. Managed-index rows are checked against their source entries by point reads, and
//! every present root entry with a complete projection is checked for its row, so the
//! walk's engine work is one page per engine scan batch plus a bounded number of point
//! reads per index row and per indexed entry — never a read per declared field.
//!
//! The logical content the walk sees — every cell of a declared root's entry family, in
//! key order — is handed to a caller-supplied [`ContentDigest`] one cell at a time, so the
//! digest is computed in the same single pass with the same bounded memory.

use std::collections::HashMap;

use marrow_store::{ReadView, StoreError};

use super::physical::{self, BelowMarker};
use super::store::WITNESS;
use super::{
    BranchNumbering, BranchSchema, FieldSchema, GroupNumbering, GroupSchema, IndexComponentRef,
    IndexSchema, NodeNumber, RootNumbering, StoreProjection, StoreSchema,
};
use crate::codec::key::{KeyScalar, decode_key_value};
use crate::codec::value::{ScalarKind, ValueShape, decode_domain};
use crate::equality::ValueDomain;

/// The most findings a report retains in full. Every finding is still counted in
/// [`AuditSummary::findings`]; the cap bounds the report's memory on a store whose every
/// cell is faulty.
pub const MAX_REPORTED_FINDINGS: usize = 256;

/// A consumer of the store's logical content stream: one call per cell of a declared
/// root's entry family, in key order. The digest kind and hash live with the store's
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
    /// An index row whose source entry is absent.
    IndexOrphan,
    /// An index row whose projected values disagree with its source entry.
    IndexStale,
    /// A present entry with a complete projection has no index row.
    IndexMissing,
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
    /// One row of a managed index: the index's root and position and its projected values.
    IndexRow {
        root: u16,
        index: u16,
        row: Vec<KeyScalar>,
    },
    /// A cell no declared node or index owns, named by its raw key.
    Cell { key: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub fault: AuditFault,
    pub site: AuditSite,
}

/// The counts the walk keeps: every cell scanned, present entries (markers), nodes with
/// descendants but no payload, index rows, and findings (including those past the cap).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditSummary {
    pub cells: u64,
    pub entries: u64,
    pub descendant_only: u64,
    pub index_rows: u64,
    pub findings: u64,
}

/// The walk's result: the counts and the first [`MAX_REPORTED_FINDINGS`] findings in key
/// order.
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
        frames: Vec::new(),
        path: Vec::new(),
        root_cursor: 0,
        index_cursor: 0,
        summary: AuditSummary::default(),
        findings: Vec::new(),
    };
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
    walker.close_frames(0)?;
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

/// One declared branch below a node: its family suffix below the parent stem, its cell-key
/// number, and the node table row of its children.
struct BranchLink {
    suffix: Vec<u8>,
    number: NodeNumber,
    node: usize,
}

/// One node shape of a root's tree: the root entry (row 0) or a branch entry.
struct NodeShape {
    /// The branch positions from the root down to this node (empty for the root).
    branch_path: Vec<u16>,
    key: Vec<ScalarKind>,
    fields: Vec<FieldSlot>,
    by_suffix: HashMap<Vec<u8>, usize>,
    groups: Vec<GroupSlot>,
    branches: Vec<BranchLink>,
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

/// One declared root: its entry family prefix, its cell-key number, its node tree, the
/// top-level field numbers (for index reads), and its indexes' table positions.
struct RootShape {
    family: Vec<u8>,
    number: NodeNumber,
    nodes: Vec<NodeShape>,
    field_numbers: Vec<NodeNumber>,
    indexes: Vec<usize>,
}

/// The fixed tables the walk classifies against, built once from the projection.
struct Tables {
    /// Every root in declaration order, which is also family-prefix order.
    roots: Vec<RootShape>,
    /// Every index of every root, sorted by family prefix (the order their cells appear).
    indexes: Vec<IndexShape>,
    witness: Vec<u8>,
}

impl Tables {
    fn new(projection: &StoreProjection, numbering: &[RootNumbering]) -> Self {
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
            let mut nodes = vec![NodeShape {
                branch_path: Vec::new(),
                key: schema.key().to_vec(),
                fields: field_slots(schema.fields()),
                by_suffix: suffix_map(numbers.fields()),
                groups: group_slots(schema.groups(), numbers.groups()),
                branches: Vec::new(),
            }];
            nodes[0].branches =
                link_branches(&mut nodes, &[], schema.branches(), numbers.branches());
            roots.push(RootShape {
                family: physical::entry_family_prefix(numbers.root()),
                number: numbers.root(),
                nodes,
                field_numbers: numbers.fields().to_vec(),
                indexes: own,
            });
        }
        indexes.sort_by(|a, b| a.prefix.cmp(&b.prefix));
        Self {
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

/// Append one node row per branch of a level (recursively) and return the level's links.
fn link_branches(
    nodes: &mut Vec<NodeShape>,
    parent_path: &[u16],
    branches: &[BranchSchema],
    numbers: &[BranchNumbering],
) -> Vec<BranchLink> {
    let mut links = Vec::with_capacity(branches.len());
    for (position, (branch, numbering)) in branches.iter().zip(numbers).enumerate() {
        let mut branch_path = parent_path.to_vec();
        branch_path.push(position as u16);
        let node = nodes.len();
        nodes.push(NodeShape {
            branch_path: branch_path.clone(),
            key: branch.key().to_vec(),
            fields: field_slots(branch.fields()),
            by_suffix: suffix_map(numbering.fields()),
            groups: Vec::new(),
            branches: Vec::new(),
        });
        let children = link_branches(nodes, &branch_path, branch.branches(), numbering.branches());
        nodes[node].branches = children;
        links.push(BranchLink {
            suffix: physical::branch_family_prefix(&[], numbering.number()),
            number: numbering.number(),
            node,
        });
    }
    links
}

/// One open node of the walk. Its presence bitmaps are sized by the node's declared
/// fields, so a frame costs the schema's width, never the store's.
struct Frame {
    root: usize,
    node: usize,
    stem: Vec<u8>,
    /// The walker's key path length before this node's own keys were pushed.
    path_len: usize,
    marker: bool,
    own: Vec<bool>,
    groups: Vec<Vec<bool>>,
    /// The decoded key projection of each top-level field (root entries of an indexed root
    /// only), for the index-row check at close.
    projected: Vec<Option<KeyScalar>>,
    any_leaf: bool,
    any_descendant: bool,
}

/// The layer a scanned key enters a child of: a root's entry family, or a branch family
/// under the open parent frame.
enum Layer {
    Root,
    Branch {
        parent_stem: Vec<u8>,
        number: NodeNumber,
    },
}

struct Walker<'a, V: ReadView> {
    view: &'a V,
    tables: &'a Tables,
    digest: &'a mut dyn ContentDigest,
    frames: Vec<Frame>,
    path: Vec<KeyScalar>,
    root_cursor: usize,
    index_cursor: usize,
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

/// Decode `kinds.len()` key columns from the front of `bytes`, each of its declared kind,
/// returning the columns and the bytes consumed.
fn decode_columns(bytes: &[u8], kinds: &[ScalarKind]) -> Option<(Vec<KeyScalar>, usize)> {
    let mut columns = Vec::with_capacity(kinds.len());
    let mut used = 0;
    for kind in kinds {
        let (column, n) = decode_key_value(bytes.get(used..)?)?;
        if column.scalar_kind() != *kind {
            return None;
        }
        columns.push(column);
        used += n;
    }
    Some((columns, used))
}

impl<V: ReadView> Walker<'_, V> {
    fn cell(&mut self, key: &[u8], value: &[u8]) -> Result<(), StoreError> {
        self.summary.cells += 1;
        let mut depth = self.frames.len();
        while depth > 0 && !key.starts_with(&self.frames[depth - 1].stem) {
            depth -= 1;
        }
        self.close_frames(depth)?;
        if self.frames.is_empty() {
            self.top_level(key, value)
        } else {
            self.digest.absorb(key, value);
            self.within_node(key, value)
        }
    }

    /// Classify a cell under no open node: a declared root's entry, a managed-index row,
    /// the commit witness, or a cell outside every declared family.
    fn top_level(&mut self, key: &[u8], value: &[u8]) -> Result<(), StoreError> {
        let tables = self.tables;
        if let Some(root) = seek_prefix(
            &tables.roots,
            |root| root.family.as_slice(),
            &mut self.root_cursor,
            key,
        ) {
            self.digest.absorb(key, value);
            let prefix_len = tables.roots[root].family.len();
            return self.enter_layer(root, 0, prefix_len, Layer::Root, key, value);
        }
        if let Some(index) = seek_prefix(
            &tables.indexes,
            |index| index.prefix.as_slice(),
            &mut self.index_cursor,
            key,
        ) {
            return self.index_row(&tables.indexes[index], key, value);
        }
        if key != tables.witness.as_slice() {
            self.finding(
                AuditFault::OutsideSchema,
                AuditSite::Cell { key: key.to_vec() },
            );
        }
        Ok(())
    }

    /// Open the child of `layer` that `key` addresses, whose columns begin at
    /// `prefix_len`: the key's child columns are decoded to derive the child's marker
    /// stem, and the cell is that marker, a cell below it (a markerless child), or
    /// malformed.
    fn enter_layer(
        &mut self,
        root: usize,
        node: usize,
        prefix_len: usize,
        layer: Layer,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), StoreError> {
        let tables = self.tables;
        let root_shape = &tables.roots[root];
        let shape = &root_shape.nodes[node];
        let Some((columns, _)) = decode_columns(&key[prefix_len..], &shape.key) else {
            self.finding(
                AuditFault::Undecodable,
                AuditSite::Cell { key: key.to_vec() },
            );
            return Ok(());
        };
        let stem = match layer {
            Layer::Root => physical::marker_key(root_shape.number, &columns),
            Layer::Branch {
                parent_stem,
                number,
            } => physical::branch_child_stem(&parent_stem, number, &columns),
        };
        if !key.starts_with(&stem) {
            self.finding(
                AuditFault::Undecodable,
                AuditSite::Cell { key: key.to_vec() },
            );
            return Ok(());
        }
        let marker = key == stem.as_slice();
        let path_len = self.path.len();
        self.path.extend(columns);
        self.frames.push(Frame {
            root,
            node,
            stem,
            path_len,
            marker,
            own: vec![false; shape.fields.len()],
            groups: shape
                .groups
                .iter()
                .map(|group| vec![false; group.fields.len()])
                .collect(),
            projected: if node == 0 && !root_shape.indexes.is_empty() {
                vec![None; shape.fields.len()]
            } else {
                Vec::new()
            },
            any_leaf: false,
            any_descendant: false,
        });
        if marker {
            self.summary.entries += 1;
            if value != physical::MARKER_VALUE {
                let site = self.node_site();
                self.finding(AuditFault::MarkerInvalid, site);
            }
            Ok(())
        } else {
            self.within_node(key, value)
        }
    }

    /// Classify a cell strictly below the top frame's marker stem.
    fn within_node(&mut self, key: &[u8], value: &[u8]) -> Result<(), StoreError> {
        let tables = self.tables;
        let top = self.frames.len() - 1;
        let (root, node) = (self.frames[top].root, self.frames[top].node);
        let shape = &tables.roots[root].nodes[node];
        let stem_len = self.frames[top].stem.len();
        let rest = &key[stem_len..];
        match physical::below_marker(&self.frames[top].stem, key) {
            BelowMarker::OwnField => {
                self.own_leaf(top);
                match shape.by_suffix.get(rest) {
                    Some(&field) => self.leaf(top, None, field, value),
                    None => self.finding(
                        AuditFault::OutsideSchema,
                        AuditSite::Cell { key: key.to_vec() },
                    ),
                }
                Ok(())
            }
            BelowMarker::OwnGroup => {
                self.own_leaf(top);
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
                    Some((group, field)) => self.leaf(top, Some(group), field, value),
                    None => self.finding(
                        AuditFault::OutsideSchema,
                        AuditSite::Cell { key: key.to_vec() },
                    ),
                }
                Ok(())
            }
            BelowMarker::BranchDescendant => {
                self.frames[top].any_descendant = true;
                match shape
                    .branches
                    .iter()
                    .find(|link| rest.starts_with(&link.suffix))
                {
                    Some(link) => {
                        let layer = Layer::Branch {
                            parent_stem: self.frames[top].stem.clone(),
                            number: link.number,
                        };
                        self.enter_layer(
                            root,
                            link.node,
                            stem_len + link.suffix.len(),
                            layer,
                            key,
                            value,
                        )
                    }
                    None => {
                        self.finding(
                            AuditFault::OutsideSchema,
                            AuditSite::Cell { key: key.to_vec() },
                        );
                        Ok(())
                    }
                }
            }
            BelowMarker::Corrupt | BelowMarker::Foreign => {
                self.finding(
                    AuditFault::Undecodable,
                    AuditSite::Cell { key: key.to_vec() },
                );
                Ok(())
            }
        }
    }

    /// Note an own payload leaf under the top frame; the first one under a markerless
    /// node is the orphan finding.
    fn own_leaf(&mut self, top: usize) {
        let frame = &mut self.frames[top];
        let first_orphan = !frame.marker && !frame.any_leaf;
        frame.any_leaf = true;
        if first_orphan {
            let site = self.node_site();
            self.finding(AuditFault::OrphanLeaf, site);
        }
    }

    /// Decode one declared leaf of the top frame and record its presence.
    fn leaf(&mut self, top: usize, group: Option<usize>, field: usize, value: &[u8]) {
        let tables = self.tables;
        let (root, node) = (self.frames[top].root, self.frames[top].node);
        let shape = &tables.roots[root].nodes[node];
        let slot = match group {
            Some(group) => &shape.groups[group].fields[field],
            None => &shape.fields[field],
        };
        let decoded = decode_domain(value, &slot.shape);
        let frame = &mut self.frames[top];
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

    /// Close every frame above `depth`, checking each closed node.
    fn close_frames(&mut self, depth: usize) -> Result<(), StoreError> {
        while self.frames.len() > depth {
            let frame = self.frames.pop().expect("a frame above depth remains");
            self.check_closed(&frame)?;
            self.path.truncate(frame.path_len);
        }
        Ok(())
    }

    /// The checks a node admits only once every cell under it has been seen: the required
    /// leaves and index rows of a present node, or its descendant-only standing.
    fn check_closed(&mut self, frame: &Frame) -> Result<(), StoreError> {
        if !frame.marker {
            if !frame.any_leaf && frame.any_descendant {
                self.summary.descendant_only += 1;
            }
            return Ok(());
        }
        let shape = &self.tables.roots[frame.root].nodes[frame.node];
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
        if frame.node == 0 {
            self.check_rows(frame)?;
        }
        Ok(())
    }

    /// Every index of a present root entry whose projection is complete must hold the
    /// entry's row.
    fn check_rows(&mut self, frame: &Frame) -> Result<(), StoreError> {
        let tables = self.tables;
        let root = &tables.roots[frame.root];
        let keys: Vec<KeyScalar> = self.path[frame.path_len..].to_vec();
        for &index in &root.indexes {
            let shape = &tables.indexes[index];
            let row: Option<Vec<KeyScalar>> = shape
                .components
                .iter()
                .map(|(component, _)| match component {
                    Component::Key(column) => keys.get(*column).cloned(),
                    Component::Field(field) => frame.projected.get(*field).cloned().flatten(),
                })
                .collect();
            let Some(row) = row else {
                continue;
            };
            let cell = physical::index_cell_key(root.number, &shape.id, &row);
            if self.view.get(&cell)?.is_none() {
                self.finding(
                    AuditFault::IndexMissing,
                    AuditSite::IndexRow {
                        root: shape.root,
                        index: shape.position,
                        row,
                    },
                );
            }
        }
        Ok(())
    }

    /// Check one managed-index row against its source entry.
    fn index_row(
        &mut self,
        shape: &IndexShape,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), StoreError> {
        self.summary.index_rows += 1;
        let root = &self.tables.roots[usize::from(shape.root)];
        let kinds: Vec<ScalarKind> = shape.components.iter().map(|(_, kind)| *kind).collect();
        let rest = &key[shape.prefix.len()..];
        let row = match decode_columns(rest, &kinds) {
            Some((row, used)) if used == rest.len() => row,
            _ => {
                self.finding(
                    AuditFault::Undecodable,
                    AuditSite::Cell { key: key.to_vec() },
                );
                return Ok(());
            }
        };
        let site = AuditSite::IndexRow {
            root: shape.root,
            index: shape.position,
            row: row.clone(),
        };
        let root_key = &root.nodes[0].key;
        let source = physical::decode_index_source_key(value, root_key.len()).filter(|source| {
            source
                .iter()
                .zip(root_key)
                .all(|(column, kind)| column.scalar_kind() == *kind)
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
        for ((component, _), projected) in shape.components.iter().zip(&row) {
            let agrees = match component {
                Component::Key(column) => source.get(*column) == Some(projected),
                Component::Field(field) => {
                    let leaf = physical::stem_field_leaf(&stem, root.field_numbers[*field]);
                    match self.view.get(&leaf)? {
                        None => false,
                        Some(bytes) => {
                            match decode_domain(&bytes, &root.nodes[0].fields[*field].shape) {
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

    /// The node site of the top frame.
    fn node_site(&self) -> AuditSite {
        let frame = self.frames.last().expect("a node site names an open frame");
        AuditSite::Node {
            root: frame.root as u16,
            branch: self.tables.roots[frame.root].nodes[frame.node]
                .branch_path
                .clone(),
            keys: self.path.clone(),
        }
    }

    /// The field site of the top frame.
    fn field_site(&self, group: Option<usize>, field: usize) -> AuditSite {
        let frame = self
            .frames
            .last()
            .expect("a field site names an open frame");
        self.site_of(frame, group, field)
    }

    fn site_of(&self, frame: &Frame, group: Option<usize>, field: usize) -> AuditSite {
        AuditSite::Field {
            root: frame.root as u16,
            branch: self.tables.roots[frame.root].nodes[frame.node]
                .branch_path
                .clone(),
            keys: self.path.clone(),
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
            assert_eq!(
                physical::branch_family_prefix(&stem, number),
                [stem.clone(), physical::branch_family_prefix(&[], number)].concat()
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
                // text 1, tag weight 1; the witness and the 3 index rows are not entry cells.
                cells: 4 + 4 + 1 + 1 + 1 + 3 + 1,
                entries: 4,
                descendant_only: 0,
                index_rows: 3,
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
        let field = numbers().fields()[0];
        let store = tamper(populated(), |txn| {
            let stem = physical::marker_key(0, &[s("z")]);
            txn.put(&physical::stem_field_leaf(&stem, field), b"Zed".to_vec())
                .expect("put");
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
            let note = physical::branch_child_stem(&a_stem(), notes.number(), &[KeyScalar::Int(9)]);
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
    fn a_dangling_unique_index_row_and_a_stale_row_are_distinct_findings() {
        let root = numbers().root();
        let store = tamper(populated(), |txn| {
            // A row for isbn "999" naming a book that does not exist.
            txn.put(
                &physical::index_cell_key(root, &BY_ISBN, &[s("999")]),
                physical::index_cell_value(&[s("nobody")]),
            )
            .expect("put");
            // A row for isbn "222" naming book "a", whose isbn is "111".
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
                    site: AuditSite::IndexRow {
                        root: 0,
                        index: 0,
                        row: vec![s("222")],
                    },
                },
                AuditFinding {
                    fault: AuditFault::IndexOrphan,
                    site: AuditSite::IndexRow {
                        root: 0,
                        index: 0,
                        row: vec![s("999")],
                    },
                },
            ]
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
                site: AuditSite::IndexRow {
                    root: 0,
                    index: 1,
                    row: vec![s("Beta"), s("b")],
                },
            }]
        );
        assert_eq!(report.summary.index_rows, 2);
    }

    #[test]
    fn a_present_entry_missing_a_required_leaf_is_named_by_its_field() {
        let numbers = numbers();
        let notes = &numbers.branches()[0];
        let store = tamper(populated(), |txn| {
            txn.remove(&physical::stem_field_leaf(&a_stem(), numbers.fields()[0]))
                .expect("remove title");
            let note = physical::branch_child_stem(&a_stem(), notes.number(), &[KeyScalar::Int(1)]);
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
                        branch: vec![0],
                        keys: vec![s("a"), KeyScalar::Int(1)],
                        group: None,
                        field: 0,
                    },
                },
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
                // Book "a" no longer has a title, so its title row is a stale row.
                AuditFinding {
                    fault: AuditFault::IndexStale,
                    site: AuditSite::IndexRow {
                        root: 0,
                        index: 1,
                        row: vec![s("Alpha"), s("a")],
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
        let foreign_branch = physical::branch_child_stem(&a_stem(), 4001, &[KeyScalar::Int(1)]);
        let foreign_root = physical::marker_key(4002, &[s("q")]);
        let foreign_index = physical::index_cell_key(numbers.root(), &[0xEE; 16], &[s("x")]);
        let foreign_meta = physical::meta_key("profile");
        let cells = [
            foreign_field.clone(),
            foreign_branch.clone(),
            foreign_root.clone(),
            foreign_index.clone(),
            foreign_meta.clone(),
        ];
        let store = tamper(populated(), |txn| {
            for cell in &cells {
                txn.put(cell, vec![0x01]).expect("put");
            }
        });
        let (report, _) = audit(&store);
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
            .map(|finding| match &finding.site {
                AuditSite::Cell { key } => key.clone(),
                other => panic!("an outside-schema cell is named by its key: {other:?}"),
            })
            .collect();
        named.sort();
        let mut expected = cells.to_vec();
        expected.sort();
        assert_eq!(named, expected);
        // The commit witness is a declared meta cell and never a finding.
        assert_eq!(report.summary.findings, 5);
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

    /// A branch child under an absent root, and a tag under an absent note, are legitimate
    /// descendant-only nodes: counted, walked, and never a finding.
    #[test]
    fn descendant_only_nodes_are_counted_and_their_subtrees_walked() {
        let numbers = numbers();
        let notes = &numbers.branches()[0];
        let tags = &notes.branches()[0];
        let store = tamper(populated(), |txn| {
            let root_z = physical::marker_key(numbers.root(), &[s("z")]);
            let note = physical::branch_child_stem(&root_z, notes.number(), &[KeyScalar::Int(1)]);
            let tag = physical::branch_child_stem(&note, tags.number(), &[s("blue")]);
            txn.put(&tag, physical::MARKER_VALUE.to_vec())
                .expect("put tag");
            txn.put(
                &physical::stem_field_leaf(&tag, tags.fields()[0]),
                b"5".to_vec(),
            )
            .expect("put weight");
        });
        let (report, _) = audit(&store);
        assert!(report.is_clean(), "{:?}", report.findings);
        assert_eq!(
            report.summary.descendant_only, 2,
            "the root z and its note 1"
        );
        assert_eq!(report.summary.entries, 5, "the tag is a present entry");
    }

    #[test]
    fn findings_past_the_cap_are_counted_but_not_retained() {
        let field = numbers().fields()[0];
        let store = tamper(populated(), |txn| {
            for i in 0..(MAX_REPORTED_FINDINGS + 40) {
                let stem = physical::marker_key(0, &[s(&format!("orphan-{i:04}"))]);
                txn.put(&physical::stem_field_leaf(&stem, field), b"x".to_vec())
                    .expect("put");
            }
        });
        let (report, _) = audit(&store);
        assert_eq!(report.summary.findings, (MAX_REPORTED_FINDINGS + 40) as u64);
        assert_eq!(report.findings.len(), MAX_REPORTED_FINDINGS);
    }

    /// An index row whose value is not a whole source key tuple is undecodable, not stale.
    #[test]
    fn an_index_row_with_a_malformed_source_key_is_undecodable() {
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
            AuditSite::IndexRow {
                root: 0,
                index: 0,
                row: vec![s("333")],
            }
        );
    }
}
