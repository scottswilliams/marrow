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
use super::{EntryValue, FieldSchema, KernelFault, StoreSchema};
use crate::codec::key::KeyScalar;
use crate::codec::value::encode_value;

/// One physical cell operation a mutation implies, in apply order.
pub(super) enum CellWrite {
    /// Write `value` at cell `key`.
    Put(Vec<u8>, Vec<u8>),
    /// Remove cell `key`.
    Remove(Vec<u8>),
}

/// The consequence planner over one root's flat entry family. Borrows the schema so
/// leaf order and field names come from the one profile the store recorded.
pub(super) struct Planner<'a> {
    root: &'a str,
    schema: &'a StoreSchema,
}

impl<'a> Planner<'a> {
    pub(super) fn new(root: &'a str, schema: &'a StoreSchema) -> Self {
        Self { root, schema }
    }

    /// The marker key (marker stem) of root entry `key`: the entry's payload-presence
    /// record and the base its own leaves, branches, and cursor derive from.
    pub(super) fn marker(&self, key: &KeyScalar) -> Vec<u8> {
        physical::marker_key(self.root, key)
    }

    /// The leaf key of `field` of the node whose marker `stem` is given. The
    /// node-parametric field-leaf primitive: the root entry and every branch entry
    /// derive their field leaves through this one owner, so the marker/field topology
    /// has a single owner regardless of a node's depth.
    pub(super) fn node_field_leaf(&self, stem: &[u8], field: &str) -> Vec<u8> {
        physical::stem_field_leaf(stem, field)
    }

    /// The leaf key of `field` of root entry `key`, the root convenience over
    /// [`Self::node_field_leaf`].
    pub(super) fn field_leaf(&self, key: &KeyScalar, field: &str) -> Vec<u8> {
        self.node_field_leaf(&self.marker(key), field)
    }

    /// Every cell key of the node with marker `stem` over `fields`: its marker
    /// followed by one leaf per field, in order. The single owner of a node's own
    /// footprint enumeration — it enumerates the marker and own field leaves only,
    /// never a branch tag, so a node's keyed descendants are outside the footprint it
    /// returns.
    pub(super) fn node_cells(&self, stem: &[u8], fields: &[FieldSchema]) -> Vec<Vec<u8>> {
        let mut cells = Vec::with_capacity(1 + fields.len());
        cells.push(stem.to_vec());
        for field in fields {
            cells.push(self.node_field_leaf(stem, &field.name));
        }
        cells
    }

    /// The writes that establish the node with marker `stem` from `entry` over
    /// `fields`: its marker then one leaf per present field, in order. Descendant-only
    /// by construction — it writes the marker and the node's own leaves and nothing
    /// beneath a branch tag, so giving a descendant-only node a payload never touches
    /// its descendants. A value outside its codec range is a
    /// [`KernelFault::ValueRange`] and no partial plan is returned.
    pub(super) fn node_write(
        &self,
        stem: &[u8],
        fields: &[FieldSchema],
        entry: &EntryValue,
    ) -> Result<Vec<CellWrite>, KernelFault> {
        let mut writes = Vec::with_capacity(1 + entry.fields.len());
        writes.push(CellWrite::Put(
            stem.to_vec(),
            physical::MARKER_VALUE.to_vec(),
        ));
        for (index, slot) in entry.fields.iter().enumerate() {
            if let Some(value) = slot {
                let bytes = encode_value(value).map_err(|_| KernelFault::ValueRange)?;
                writes.push(CellWrite::Put(
                    self.node_field_leaf(stem, &fields[index].name),
                    bytes,
                ));
            }
        }
        Ok(writes)
    }

    /// The removals that erase the payload of the node with marker `stem` over
    /// `fields`: its marker and every own field-leaf cell. It enumerates only the
    /// node's own cells — never a branch tag — so a payload erase preserves the node's
    /// keyed descendants (the descendant-preserving erase law).
    pub(super) fn node_erase(&self, stem: &[u8], fields: &[FieldSchema]) -> Vec<CellWrite> {
        self.node_cells(stem, fields)
            .into_iter()
            .map(CellWrite::Remove)
            .collect()
    }

    /// The writes that establish root entry `key` from `entry`, the root convenience
    /// over [`Self::node_write`].
    pub(super) fn write_entry(
        &self,
        key: &KeyScalar,
        entry: &EntryValue,
    ) -> Result<Vec<CellWrite>, KernelFault> {
        self.node_write(&self.marker(key), &self.schema.fields, entry)
    }

    /// The removals that erase root entry `key`, the root convenience over
    /// [`Self::node_erase`].
    pub(super) fn erase_entry(&self, key: &KeyScalar) -> Vec<CellWrite> {
        self.node_erase(&self.marker(key), &self.schema.fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::value::{RuntimeScalar, ScalarKind};
    use crate::durable::FieldSchema;

    fn schema() -> StoreSchema {
        StoreSchema {
            root_name: "counters".into(),
            key: ScalarKind::Int,
            fields: vec![
                FieldSchema {
                    name: "value".into(),
                    kind: ScalarKind::Int,
                    required: true,
                },
                FieldSchema {
                    name: "label".into(),
                    kind: ScalarKind::Str,
                    required: false,
                },
            ],
            branches: Vec::new(),
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
        let planner = Planner::new(&schema.root_name, &schema);
        let key = KeyScalar::Int(1);
        let stem = planner.marker(&key);
        assert_eq!(
            planner.node_cells(&stem, &schema.fields),
            vec![
                stem.clone(),
                planner.node_field_leaf(&stem, "value"),
                planner.node_field_leaf(&stem, "label"),
            ]
        );
    }

    /// A whole-entry write is descendant-only: the marker plus one leaf per *present*
    /// field, and nothing for an absent sparse field.
    #[test]
    fn write_entry_plans_the_marker_and_only_present_leaves() {
        let schema = schema();
        let planner = Planner::new(&schema.root_name, &schema);
        let key = KeyScalar::Int(1);
        // Required value present, sparse label absent.
        let entry = EntryValue {
            fields: vec![Some(RuntimeScalar::Int(5)), None],
        };
        let ops = planner.write_entry(&key, &entry).expect("in range");
        assert_eq!(
            keys(&ops),
            vec![
                planner.marker(&key).as_slice(),
                planner.field_leaf(&key, "value").as_slice(),
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
    fn erase_entry_removes_the_marker_and_every_field_leaf() {
        let schema = schema();
        let planner = Planner::new(&schema.root_name, &schema);
        let key = KeyScalar::Int(1);
        let ops = planner.erase_entry(&key);
        assert!(ops.iter().all(|op| matches!(op, CellWrite::Remove(_))));
        assert_eq!(
            keys(&ops),
            keys_of(&planner.node_cells(&planner.marker(&key), &schema.fields)),
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
        let planner = Planner::new(&schema.root_name, &schema);
        // Any marker stem stands in for a node here (a branch entry's stem is one such
        // stem, one level below the root); the fields differ from the root's schema.
        let stem = planner.marker(&KeyScalar::Int(7));
        let fields = vec![FieldSchema {
            name: "text".into(),
            kind: ScalarKind::Str,
            required: true,
        }];
        assert_eq!(
            planner.node_cells(&stem, &fields),
            vec![stem.clone(), planner.node_field_leaf(&stem, "text")],
        );
        let entry = EntryValue {
            fields: vec![Some(RuntimeScalar::Str("hi".into()))],
        };
        let writes = planner
            .node_write(&stem, &fields, &entry)
            .expect("in range");
        assert_eq!(
            keys(&writes),
            vec![
                stem.as_slice(),
                planner.node_field_leaf(&stem, "text").as_slice(),
            ],
        );
        let erases = planner.node_erase(&stem, &fields);
        assert!(erases.iter().all(|op| matches!(op, CellWrite::Remove(_))));
        assert_eq!(keys(&erases), keys_of(&planner.node_cells(&stem, &fields)));
    }

    fn keys_of(cells: &[Vec<u8>]) -> Vec<&[u8]> {
        cells.iter().map(Vec::as_slice).collect()
    }
}
