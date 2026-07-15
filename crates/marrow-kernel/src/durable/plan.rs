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
use super::{EntryValue, KernelFault, StoreSchema};
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

    /// The marker key of entry `key`: the entry's payload-presence record.
    pub(super) fn marker(&self, key: &KeyScalar) -> Vec<u8> {
        physical::marker_key(self.root, key)
    }

    /// The leaf key of `field` of entry `key`.
    pub(super) fn field_leaf(&self, key: &KeyScalar, field: &str) -> Vec<u8> {
        physical::field_leaf_key(self.root, key, field)
    }

    /// Every cell key of entry `key`: its marker followed by one leaf key per schema
    /// field, in schema order. The single enumeration of an entry's footprint,
    /// consumed by whole-entry read, whole-entry removal, and commit reconciliation,
    /// so those three never drift about which cells constitute an entry.
    pub(super) fn entry_cells(&self, key: &KeyScalar) -> Vec<Vec<u8>> {
        let mut cells = Vec::with_capacity(1 + self.schema.fields.len());
        cells.push(self.marker(key));
        for field in &self.schema.fields {
            cells.push(self.field_leaf(key, &field.name));
        }
        cells
    }

    /// The writes that establish entry `key` from `entry`: its marker then one leaf
    /// per present field, in schema order (descendant-only — nothing outside the
    /// entry's own subtree). A value outside its codec range is a
    /// [`KernelFault::ValueRange`] and no partial plan is returned.
    pub(super) fn write_entry(
        &self,
        key: &KeyScalar,
        entry: &EntryValue,
    ) -> Result<Vec<CellWrite>, KernelFault> {
        let mut writes = Vec::with_capacity(1 + entry.fields.len());
        writes.push(CellWrite::Put(
            self.marker(key),
            physical::MARKER_VALUE.to_vec(),
        ));
        for (index, slot) in entry.fields.iter().enumerate() {
            if let Some(value) = slot {
                let bytes = encode_value(value).map_err(|_| KernelFault::ValueRange)?;
                writes.push(CellWrite::Put(
                    self.field_leaf(key, &self.schema.fields[index].name),
                    bytes,
                ));
            }
        }
        Ok(writes)
    }

    /// The removals that erase entry `key`: its marker and every field-leaf cell.
    /// The engine offers only point removal, so the entry's cells are enumerated from
    /// the schema rather than deleting a key prefix.
    pub(super) fn erase_entry(&self, key: &KeyScalar) -> Vec<CellWrite> {
        self.entry_cells(key)
            .into_iter()
            .map(CellWrite::Remove)
            .collect()
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
        }
    }

    fn keys(ops: &[CellWrite]) -> Vec<&[u8]> {
        ops.iter()
            .map(|op| match op {
                CellWrite::Put(key, _) | CellWrite::Remove(key) => key.as_slice(),
            })
            .collect()
    }

    /// The single footprint enumeration: an entry is its marker then one leaf per
    /// schema field, in schema order.
    #[test]
    fn entry_cells_are_the_marker_then_one_leaf_per_field_in_order() {
        let schema = schema();
        let planner = Planner::new(&schema.root_name, &schema);
        let key = KeyScalar::Int(1);
        assert_eq!(
            planner.entry_cells(&key),
            vec![
                planner.marker(&key),
                planner.field_leaf(&key, "value"),
                planner.field_leaf(&key, "label"),
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
        assert_eq!(keys(&ops), keys_of(&planner.entry_cells(&key)));
    }

    fn keys_of(cells: &[Vec<u8>]) -> Vec<&[u8]> {
        cells.iter().map(Vec::as_slice).collect()
    }
}
