//! The consequence planner: the single owner of how a logical mutation over the
//! durable graph decomposes into ordered physical cell operations (design §G).
//!
//! Every whole-entry mutation the kernel performs — create, replace, erase — and
//! every read or commit step that must enumerate an entry's footprint routes through
//! one [`Planner`], so the marker/leaf topology has exactly one owner. The planner is
//! pure: it maps an intent plus the store schema to an ordered [`CellWrite`] list or
//! a cell-key enumeration, and the transaction session applies the writes, so every
//! consequence of one mutation shares that session's transaction and rolls back with
//! it. Sparse structural maintenance (E03) and bounded traversal (E04) widen this one
//! owner rather than introducing a second planner.
//!
//! Two topology laws hold at the flat layer and are the shape the later lanes extend:
//!
//! - **Descendant-only create/replace.** A whole-entry write touches only the
//!   entry's own subtree — its marker and its field leaves — never a sibling or a
//!   parent cell.
//! - **Replace/erase confine to the entry's cells.** Removal enumerates exactly the
//!   marker and one leaf per schema field. A flat entry has no keyed descendants
//!   below it, so nothing beneath the key is disturbed; when a keyed descendant graph
//!   exists (E03/E04) the same enumeration is the point that must preserve it and
//!   perform finite-ancestor maintenance.

use super::physical;
use super::{EntryValue, IndexComponent, IndexSchema, KernelFault, ResolvedField, ResolvedGroup};
use crate::codec::key::KeyScalar;
use crate::codec::value::encode_domain;
use crate::equality::ValueDomain;

/// One physical cell operation a mutation implies, in apply order.
pub(super) enum CellWrite {
    /// Write `value` at cell `key`.
    Put(Vec<u8>, Vec<u8>),
    /// Remove cell `key`.
    Remove(Vec<u8>),
}

/// One physical managed-index cell operation a root entry write implies, in apply order.
/// Separate from [`CellWrite`] because a unique-index put carries a maintenance obligation
/// the pure planner cannot discharge: the session must reject a differently-owned existing
/// cell (a read the planner never performs).
pub(super) enum IndexOp {
    /// Remove the index cell at `key` (a row that left an index).
    Remove(Vec<u8>),
    /// Write `value` at non-unique index cell `key`.
    Put(Vec<u8>, Vec<u8>),
    /// Write `value` at unique index cell `key`; the session faults if the cell already
    /// holds a different source identity.
    UniquePut(Vec<u8>, Vec<u8>),
}

/// The consequence planner: the single owner of how a logical mutation over one
/// durable node decomposes into ordered physical cell operations. It is node-parametric
/// and stateless — every operation takes the node's resolved marker stem and its fields
/// explicitly, so a root entry and a branch entry share one topology owner regardless of
/// depth.
pub(super) struct Planner;

impl Planner {
    pub(super) fn new() -> Self {
        Self
    }

    /// The leaf key of `field` of the node whose marker `stem` is given. The
    /// node-parametric field-leaf primitive: the root entry and every branch entry
    /// derive their field leaves through this one owner, so the marker/field topology
    /// has a single owner regardless of a node's depth.
    pub(super) fn node_field_leaf(&self, stem: &[u8], field: physical::NodeNumber) -> Vec<u8> {
        physical::stem_field_leaf(stem, field)
    }

    /// Every cell key of the node with marker `stem` over `fields` and `groups`: its
    /// marker, one leaf per top-level field, then one leaf per field of each group under
    /// that group's namespace, all in order. The single owner of a node's own footprint
    /// enumeration — it enumerates the marker, own field leaves, and its groups' payload
    /// leaves only, never a branch tag, so a node's keyed descendants stay outside the
    /// footprint it returns while its groups (its own payload) are inside it.
    pub(super) fn node_cells(
        &self,
        stem: &[u8],
        fields: &[ResolvedField],
        groups: &[ResolvedGroup],
    ) -> Vec<Vec<u8>> {
        let mut cells = Vec::with_capacity(1 + fields.len());
        cells.push(stem.to_vec());
        for field in fields {
            cells.push(self.node_field_leaf(stem, field.number));
        }
        for group in groups {
            let group_stem = physical::group_stem(stem, group.number);
            cells.extend(self.group_cells(&group_stem, &group.fields));
        }
        cells
    }

    /// The writes that establish the node with marker `stem` from `entry` over `fields`
    /// and `groups`: its marker, then one leaf per present top-level field, then each
    /// group's present leaves, in order. Descendant-only by construction — it writes the
    /// marker and the node's own leaves (its top-level fields and its groups) and nothing
    /// beneath a branch tag, so giving a descendant-only node a payload never touches its
    /// keyed descendants. `entry.groups` aligns to `groups`. A value outside its codec
    /// range is a [`KernelFault::ValueRange`] and no partial plan is returned.
    pub(super) fn node_write(
        &self,
        stem: &[u8],
        fields: &[ResolvedField],
        groups: &[ResolvedGroup],
        entry: &EntryValue,
    ) -> Result<Vec<CellWrite>, KernelFault> {
        let mut writes = Vec::with_capacity(1 + entry.fields.len());
        writes.push(CellWrite::Put(
            stem.to_vec(),
            physical::MARKER_VALUE.to_vec(),
        ));
        for (index, slot) in entry.fields.iter().enumerate() {
            if let Some(value) = slot {
                let bytes = encode_domain(value).map_err(|_| KernelFault::ValueRange)?;
                writes.push(CellWrite::Put(
                    self.node_field_leaf(stem, fields[index].number),
                    bytes,
                ));
            }
        }
        for (group, sub) in groups.iter().zip(&entry.groups) {
            let group_stem = physical::group_stem(stem, group.number);
            writes.extend(self.group_write(&group_stem, &group.fields, sub)?);
        }
        Ok(writes)
    }

    /// The removals that erase the payload of the node with marker `stem` over `fields`
    /// and `groups`: its marker, every own field-leaf cell, and every group-leaf cell. It
    /// enumerates only the node's own cells — never a branch tag — so a payload erase
    /// preserves the node's keyed descendants (the descendant-preserving erase law) while
    /// dropping its groups' leaves (its own payload, per the exact-replacement law).
    pub(super) fn node_erase(
        &self,
        stem: &[u8],
        fields: &[ResolvedField],
        groups: &[ResolvedGroup],
    ) -> Vec<CellWrite> {
        self.node_cells(stem, fields, groups)
            .into_iter()
            .map(CellWrite::Remove)
            .collect()
    }

    /// Every leaf cell key of the group whose group stem is `group_stem` over `fields`:
    /// one leaf per field, in order, through the shared [`Self::node_field_leaf`] owner.
    /// A group is part of its entry's payload and carries no marker (its presence is the
    /// entry's), so — unlike [`Self::node_cells`] — a group's footprint is its field
    /// leaves alone. Because every cell derives from `group_stem` (`<marker> 0x28
    /// esc(group)`), the whole footprint is disjoint from the entry's top-level fields,
    /// its sibling groups, and its branch descendants. The single owner of a group's
    /// footprint enumeration.
    pub(super) fn group_cells(&self, group_stem: &[u8], fields: &[ResolvedField]) -> Vec<Vec<u8>> {
        fields
            .iter()
            .map(|field| self.node_field_leaf(group_stem, field.number))
            .collect()
    }

    /// The writes that establish the group whose group stem is `group_stem` from `value`
    /// over `fields`: one leaf per present field, in order — and no marker. Confined to
    /// the group's own leaves by construction, so a group write never touches the entry
    /// marker, a top-level field leaf, a sibling group, or a branch descendant (the
    /// group-scoped payload-only law). A value outside its codec range is a
    /// [`KernelFault::ValueRange`] and no partial plan is returned.
    pub(super) fn group_write(
        &self,
        group_stem: &[u8],
        fields: &[ResolvedField],
        value: &EntryValue,
    ) -> Result<Vec<CellWrite>, KernelFault> {
        let mut writes = Vec::with_capacity(value.fields.len());
        for (index, slot) in value.fields.iter().enumerate() {
            if let Some(value) = slot {
                let bytes = encode_domain(value).map_err(|_| KernelFault::ValueRange)?;
                writes.push(CellWrite::Put(
                    self.node_field_leaf(group_stem, fields[index].number),
                    bytes,
                ));
            }
        }
        Ok(writes)
    }

    /// The removals that erase every one of the group's own leaf cells (present or not) —
    /// and nothing else. A group carries no marker, so this removes only the field leaves
    /// under `group_stem`; the entry marker, the entry's top-level fields, its sibling
    /// groups, and its branch descendants are all preserved (the group-scoped
    /// payload-only erase law).
    pub(super) fn group_erase(
        &self,
        group_stem: &[u8],
        fields: &[ResolvedField],
    ) -> Vec<CellWrite> {
        self.group_cells(group_stem, fields)
            .into_iter()
            .map(CellWrite::Remove)
            .collect()
    }

    /// The ordered index-cell operations a root entry write implies, given the entry's key
    /// tuple and its projected field values before (`old`) and after (`new`) the write, for
    /// the root's `indexes` in stable declaration order. A row exists exactly when every
    /// projected component is present, so a field absent in a state contributes no row for
    /// it; an unchanged row emits nothing, and a changed row emits a remove of the old key
    /// then a put of the new. A unique index's put is a [`IndexOp::UniquePut`] the session
    /// enforces. This is the single owner of the source-write-to-index-cell decomposition —
    /// the pure widening of the consequence planner
    /// rather than a second maintenance path. A non-scalar or non-key-eligible projected
    /// value is [`KernelFault::Corruption`] (the verifier's eligibility rule already
    /// excludes it).
    pub(super) fn index_writes(
        &self,
        root: physical::NodeNumber,
        indexes: &[IndexSchema],
        keys: &[KeyScalar],
        old: &[Option<ValueDomain>],
        new: &[Option<ValueDomain>],
    ) -> Result<Vec<IndexOp>, KernelFault> {
        let mut ops = Vec::new();
        for index in indexes {
            let old_row = project_row(index, keys, old)?;
            let new_row = project_row(index, keys, new)?;
            if old_row == new_row {
                continue;
            }
            if let Some(row) = &old_row {
                ops.push(IndexOp::Remove(physical::index_cell_key(
                    root, &index.id, row,
                )));
            }
            if let Some(row) = &new_row {
                let cell = physical::index_cell_key(root, &index.id, row);
                let value = physical::index_cell_value(keys);
                ops.push(if index.unique {
                    IndexOp::UniquePut(cell, value)
                } else {
                    IndexOp::Put(cell, value)
                });
            }
        }
        Ok(ops)
    }
}

/// One index's projected row for an entry: the ordered projected component values — a key
/// column from `keys`, a top-level field from `fields`. `Ok(None)` when a projected field
/// is absent, so the entry contributes no row to this index; a non-scalar or
/// non-key-eligible projected value is [`KernelFault::Corruption`].
fn project_row(
    index: &IndexSchema,
    keys: &[KeyScalar],
    fields: &[Option<ValueDomain>],
) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
    let mut row = Vec::with_capacity(index.projection.len());
    for component in &index.projection {
        let key = match component {
            IndexComponent::Key(column) => keys
                .get(*column as usize)
                .cloned()
                .ok_or(KernelFault::Corruption)?,
            IndexComponent::Field(field) => {
                match fields.get(*field as usize).and_then(Option::as_ref) {
                    None => return Ok(None),
                    Some(ValueDomain::Scalar(scalar)) => scalar
                        .as_key()
                        .ok()
                        .flatten()
                        .ok_or(KernelFault::Corruption)?,
                    Some(_) => return Err(KernelFault::Corruption),
                }
            }
        };
        row.push(key);
    }
    Ok(Some(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::key::KeyScalar;
    use crate::codec::value::{RuntimeScalar, ScalarKind};
    use crate::durable::{FieldSchema, StoreSchema, number_store};
    use crate::equality::ValueDomain;
    use physical::NodeNumber;

    fn schema() -> StoreSchema {
        StoreSchema {
            root_name: "counters".into(),
            key: vec![ScalarKind::Int],
            fields: vec![
                FieldSchema::scalar("value", ScalarKind::Int, true),
                FieldSchema::scalar("label", ScalarKind::Str, false),
            ],
            branches: Vec::new(),
            groups: Vec::new(),
            indexes: Vec::new(),
        }
    }

    /// A resolved field with an arbitrary distinct number (the planner keys leaves by the
    /// number, so structural tests only need distinctness); the name is diagnostics-only.
    fn rf(number: NodeNumber, kind: ScalarKind, required: bool) -> ResolvedField {
        ResolvedField {
            number,
            name: String::new(),
            shape: crate::codec::value::ValueShape::Scalar(kind),
            required,
        }
    }

    /// The root's cell-key number and its fields/groups as the planner consumes them, from
    /// the store-wide numbering — the same numbers the store computes for this schema.
    fn resolved(schema: &StoreSchema) -> (NodeNumber, Vec<ResolvedField>, Vec<ResolvedGroup>) {
        let numbering = number_store(std::slice::from_ref(schema));
        let n = &numbering[0];
        let fields = schema
            .fields
            .iter()
            .zip(&n.fields)
            .map(|(f, num)| rf_named(*num, f))
            .collect();
        let groups = schema
            .groups
            .iter()
            .zip(&n.groups)
            .map(|(g, gn)| ResolvedGroup {
                number: gn.number,
                fields: g
                    .fields
                    .iter()
                    .zip(&gn.fields)
                    .map(|(f, num)| rf_named(*num, f))
                    .collect(),
            })
            .collect();
        (n.root, fields, groups)
    }

    fn rf_named(number: NodeNumber, field: &FieldSchema) -> ResolvedField {
        ResolvedField {
            number,
            name: field.name.clone(),
            shape: field.shape.clone(),
            required: field.required,
        }
    }

    fn keys(ops: &[CellWrite]) -> Vec<&[u8]> {
        ops.iter()
            .map(|op| match op {
                CellWrite::Put(key, _) | CellWrite::Remove(key) => key.as_slice(),
            })
            .collect()
    }

    /// The single footprint enumeration: a node is its marker then one leaf per
    /// field, in order, through the node-parametric `node_cells`.
    #[test]
    fn node_cells_are_the_marker_then_one_leaf_per_field_in_order() {
        let schema = schema();
        let (root, fields, groups) = resolved(&schema);
        let planner = Planner::new();
        let key = KeyScalar::Int(1);
        let stem = physical::marker_key(root, std::slice::from_ref(&key));
        assert_eq!(
            planner.node_cells(&stem, &fields, &groups),
            vec![
                stem.clone(),
                planner.node_field_leaf(&stem, fields[0].number),
                planner.node_field_leaf(&stem, fields[1].number),
            ]
        );
    }

    /// A whole-entry write is descendant-only: the marker plus one leaf per *present*
    /// field, and nothing for an absent sparse field.
    #[test]
    fn node_write_plans_the_marker_and_only_present_leaves() {
        let schema = schema();
        let (root, fields, groups) = resolved(&schema);
        let planner = Planner::new();
        let key = KeyScalar::Int(1);
        let stem = physical::marker_key(root, std::slice::from_ref(&key));
        // Required value present, sparse label absent.
        let entry = EntryValue {
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(5))), None],
            groups: Vec::new(),
        };
        let ops = planner
            .node_write(&stem, &fields, &groups, &entry)
            .expect("in range");
        assert_eq!(
            keys(&ops),
            vec![
                stem.as_slice(),
                planner.node_field_leaf(&stem, fields[0].number).as_slice(),
            ],
            "a whole-entry write touches only the marker and present leaves"
        );
        assert!(
            matches!(&ops[0], CellWrite::Put(_, value) if value == physical::MARKER_VALUE),
            "the first write is the payload-presence marker"
        );
    }

    /// Erase confines to the entry's own cells: a remove for the marker and every
    /// field leaf, present or not.
    #[test]
    fn node_erase_removes_the_marker_and_every_field_leaf() {
        let schema = schema();
        let (root, fields, groups) = resolved(&schema);
        let planner = Planner::new();
        let key = KeyScalar::Int(1);
        let stem = physical::marker_key(root, std::slice::from_ref(&key));
        let ops = planner.node_erase(&stem, &fields, &groups);
        assert!(ops.iter().all(|op| matches!(op, CellWrite::Remove(_))));
        assert_eq!(
            keys(&ops),
            keys_of(&planner.node_cells(&stem, &fields, &groups)),
        );
    }

    /// The node-parametric core operates on any `(stem, fields)` pair, not only the
    /// root schema — the seam a branch node reuses one level down. Given a stem and a
    /// field list distinct from the root's, `node_cells` enumerates the marker then
    /// one leaf per field, `node_write` plans the marker plus present leaves, and
    /// `node_erase` removes exactly the node's own cells, so a branch entry reuses the
    /// single cell-topology owner rather than a second structural-maintenance path.
    #[test]
    fn the_node_core_operates_on_an_arbitrary_node_stem_and_fields() {
        let schema = schema();
        let (root, _, _) = resolved(&schema);
        let planner = Planner::new();
        // Any marker stem stands in for a node here (a branch entry's stem is one such
        // stem, one level below the root); the fields differ from the root's schema.
        let stem = physical::marker_key(root, &[KeyScalar::Int(7)]);
        let text = rf(99, ScalarKind::Str, true);
        let fields = vec![text.clone()];
        assert_eq!(
            planner.node_cells(&stem, &fields, &[]),
            vec![stem.clone(), planner.node_field_leaf(&stem, text.number)],
        );
        let entry = EntryValue {
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
            groups: Vec::new(),
        };
        let writes = planner
            .node_write(&stem, &fields, &[], &entry)
            .expect("in range");
        assert_eq!(
            keys(&writes),
            vec![
                stem.as_slice(),
                planner.node_field_leaf(&stem, text.number).as_slice(),
            ],
        );
        let erases = planner.node_erase(&stem, &fields, &[]);
        assert!(erases.iter().all(|op| matches!(op, CellWrite::Remove(_))));
        assert_eq!(
            keys(&erases),
            keys_of(&planner.node_cells(&stem, &fields, &[]))
        );
    }

    fn keys_of(cells: &[Vec<u8>]) -> Vec<&[u8]> {
        cells.iter().map(Vec::as_slice).collect()
    }

    /// A group's footprint is its field leaves alone — no marker. `group_cells`
    /// enumerates one leaf per field in order under the group stem, `group_write` plans
    /// only present leaves, and `group_erase` removes exactly the group's own cells.
    #[test]
    fn group_cells_are_the_field_leaves_alone_with_no_marker() {
        let schema = schema();
        let (root, _, _) = resolved(&schema);
        let planner = Planner::new();
        let stem = physical::marker_key(root, &[KeyScalar::Int(1)]);
        // A group numbered 30 with two fields (numbers 31, 32).
        let group_stem = physical::group_stem(&stem, 30);
        let pages = rf(31, ScalarKind::Int, false);
        let language = rf(32, ScalarKind::Str, false);
        let fields = vec![pages.clone(), language.clone()];
        // The footprint is exactly the two leaves — the marker cell is absent.
        let cells = planner.group_cells(&group_stem, &fields);
        assert_eq!(
            keys_of(&cells),
            vec![
                planner
                    .node_field_leaf(&group_stem, pages.number)
                    .as_slice(),
                planner
                    .node_field_leaf(&group_stem, language.number)
                    .as_slice(),
            ]
        );
        assert!(
            !cells.contains(&group_stem),
            "a group footprint carries no marker cell"
        );

        // A write plans only the present leaf, and never a marker put.
        let value = EntryValue {
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(384))), None],
            groups: Vec::new(),
        };
        let writes = planner
            .group_write(&group_stem, &fields, &value)
            .expect("in range");
        assert_eq!(
            keys(&writes),
            vec![
                planner
                    .node_field_leaf(&group_stem, pages.number)
                    .as_slice()
            ],
            "a group write touches only present leaves"
        );

        // An erase removes exactly the group's own cells, all removals.
        let erases = planner.group_erase(&group_stem, &fields);
        assert!(erases.iter().all(|op| matches!(op, CellWrite::Remove(_))));
        assert_eq!(keys(&erases), keys_of(&cells));
    }

    /// The group-scoped payload-only law at the planner level: a group write's cell-key
    /// set is disjoint from the entry marker, the entry's top-level field leaves, a
    /// sibling group's leaves, and a branch descendant's cells — so a group replace or
    /// erase provably disturbs none of them.
    #[test]
    fn a_group_write_is_disjoint_from_siblings() {
        let schema = schema();
        let (root, fields, _) = resolved(&schema);
        let planner = Planner::new();
        let stem = physical::marker_key(root, &[KeyScalar::Int(1)]);
        let group_fields = vec![rf(31, ScalarKind::Int, false)];
        // The group under test (number 30) and a sibling group (number 40).
        let details = physical::group_stem(&stem, 30);

        let group_ops = planner.group_erase(&details, &group_fields);
        let touched: Vec<&[u8]> = keys(&group_ops);

        // Sibling cells the entry owns, none of which a group op may name.
        let siblings = vec![
            stem.clone(),                                                  // the entry marker
            planner.node_field_leaf(&stem, fields[0].number),              // a top-level field leaf
            planner.node_field_leaf(&physical::group_stem(&stem, 40), 31), // sibling group leaf
            physical::branch_child_stem(&stem, 50, &[KeyScalar::Int(7)]),  // a branch cell
        ];
        for sibling in &siblings {
            assert!(
                !touched.contains(&sibling.as_slice()),
                "a group op must not touch a sibling cell"
            );
        }
    }

    fn scalar(value: i64) -> Option<ValueDomain> {
        Some(ValueDomain::Scalar(RuntimeScalar::Int(value)))
    }

    /// The pure index decomposition: for a changed row, a remove of the old key then a put
    /// of the new, per index in stable declaration order; a unique index yields a
    /// `UniquePut`. A key column value comes from `keys`, a field value from the state.
    #[test]
    fn index_writes_emit_remove_then_put_per_changed_index_in_order() {
        let planner = Planner::new();
        // The root's cell-key number (the schema's single root numbers to 0).
        let (root, _, _) = resolved(&schema());
        // Two indexes over a root keyed by one int column, with one int field (position 0):
        // a non-unique index projecting [field 0, key 0] and a unique index projecting
        // [field 0].
        let indexes = vec![
            IndexSchema {
                id: [0xA0; 16],
                unique: false,
                projection: vec![IndexComponent::Field(0), IndexComponent::Key(0)],
            },
            IndexSchema {
                id: [0xB1; 16],
                unique: true,
                projection: vec![IndexComponent::Field(0)],
            },
        ];
        let keys = [KeyScalar::Int(7)];
        // Field 0 changes from 1 to 2: both indexes' rows move.
        let ops = planner
            .index_writes(root, &indexes, &keys, &[scalar(1)], &[scalar(2)])
            .expect("in range");

        let nonunique_old =
            physical::index_cell_key(root, &[0xA0; 16], &[KeyScalar::Int(1), KeyScalar::Int(7)]);
        let nonunique_new =
            physical::index_cell_key(root, &[0xA0; 16], &[KeyScalar::Int(2), KeyScalar::Int(7)]);
        let unique_old = physical::index_cell_key(root, &[0xB1; 16], &[KeyScalar::Int(1)]);
        let unique_new = physical::index_cell_key(root, &[0xB1; 16], &[KeyScalar::Int(2)]);
        let value = physical::index_cell_value(&keys);

        // Stable order: the first index's remove+put precede the second's; the unique
        // index's put is a UniquePut.
        assert_eq!(ops.len(), 4);
        assert!(matches!(&ops[0], IndexOp::Remove(k) if *k == nonunique_old));
        assert!(matches!(&ops[1], IndexOp::Put(k, v) if *k == nonunique_new && *v == value));
        assert!(matches!(&ops[2], IndexOp::Remove(k) if *k == unique_old));
        assert!(matches!(&ops[3], IndexOp::UniquePut(k, v) if *k == unique_new && *v == value));
    }

    /// A row exists only when every projected field is present: an absent projected field
    /// contributes no row, so a create with that field absent emits nothing for its index.
    #[test]
    fn an_absent_projected_field_contributes_no_row() {
        let planner = Planner::new();
        let (root, _, _) = resolved(&schema());
        let indexes = vec![IndexSchema {
            id: [0xA0; 16],
            unique: false,
            projection: vec![IndexComponent::Field(0), IndexComponent::Key(0)],
        }];
        let keys = [KeyScalar::Int(7)];
        // Create (old all-absent) with the projected field absent: no row.
        let ops = planner
            .index_writes(root, &indexes, &keys, &[None], &[None])
            .expect("in range");
        assert!(ops.is_empty());
    }
}
