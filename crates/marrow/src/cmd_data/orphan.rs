//! Detection of stored data cells the checked schema does not declare.
//!
//! `data integrity` verifies declared cells by walking the schema, so a cell under
//! a dropped root or an undeclared member is silently skipped. This pass enumerates
//! the store's actual data-family cells and flags any whose store catalog id or
//! member catalog path the schema no longer declares — data a dropped root or field
//! left behind. Index-family cells are derived from data and are never flagged. A
//! cell key that does not decode under the tree-cell key grammar is store corruption.

use std::collections::{HashMap, HashSet};

use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedPlace, checked_saved_root_place,
};
use marrow_store::StoreError;
use marrow_store::cell::{DataCellKey, decode_data_cell_key};
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use super::inspect::push_key;

/// One stored cell the schema does not declare, or whose key does not decode.
pub(super) struct OrphanProblem {
    pub(super) code: &'static str,
    pub(super) path: String,
    pub(super) message: String,
}

/// Visit every stored data cell the checked schema does not declare, plus any cell
/// whose key does not decode, invoking `report` for each. Index-family cells are
/// derived and never reported. Bounded: `visit_backup_cells` pages internally.
pub(super) fn visit_orphans(
    store: &TreeStore,
    program: &CheckedProgram,
    mut report: impl FnMut(OrphanProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let schema = DeclaredSchema::from_program(program);
    visit_orphans_in_schema(store, &schema, &mut report)
}

pub(super) fn visit_orphans_in_places(
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    mut report: impl FnMut(OrphanProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let schema = DeclaredSchema::from_places(places);
    visit_orphans_in_schema(store, &schema, &mut report)
}

fn visit_orphans_in_schema(
    store: &TreeStore,
    schema: &DeclaredSchema,
    report: &mut impl FnMut(OrphanProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    store.visit_backup_cells(|key, _value| {
        if let Some(problem) = schema.classify(key) {
            report(problem)?;
        }
        Ok(())
    })
}

/// The store catalog ids and per-store member catalog ids the checked schema
/// declares, keyed for membership tests against decoded cell keys.
struct DeclaredSchema {
    /// Store catalog id to its saved root name, for rendering a recognizable path.
    roots: HashMap<String, String>,
    /// Store catalog id to the set of member catalog ids declared under that store.
    members: HashMap<String, MemberSet>,
}

struct MemberSet {
    ids: HashSet<String>,
    names: HashMap<String, String>,
}

impl DeclaredSchema {
    fn from_program(program: &CheckedProgram) -> Self {
        let places: Vec<_> = program
            .facts
            .stores()
            .iter()
            .filter_map(|store| {
                checked_saved_root_place(program, &store.root, marrow_syntax::SourceSpan::default())
            })
            .collect();
        Self::from_places(&places)
    }

    fn from_places(places: &[CheckedSavedPlace]) -> Self {
        let mut roots = HashMap::new();
        let mut members = HashMap::new();
        for place in places {
            let Some(store_catalog_id) = place.store_catalog_id.clone() else {
                continue;
            };
            roots.insert(store_catalog_id.clone(), place.root.clone());
            let mut set = MemberSet {
                ids: HashSet::new(),
                names: HashMap::new(),
            };
            collect_members(&place.root_members, &mut set);
            members.insert(store_catalog_id, set);
        }
        Self { roots, members }
    }

    /// Classify one stored cell key. Returns a problem when the cell is an orphan or
    /// its key does not decode, or `None` when it is a declared or index cell.
    ///
    /// Classification is catalog-id membership: a cell whose store id or any member id
    /// the schema no longer declares is an orphan, which catches data under a dropped
    /// root or field — the ADR-named orphan cases. It does not validate exact member
    /// nesting, so an exotic misnesting of declared ids is out of v0.1 scope.
    fn classify(&self, key: &[u8]) -> Option<OrphanProblem> {
        let DataCellKey {
            store,
            identity,
            path,
        } = match decode_data_cell_key(key) {
            Some(cell) => cell,
            None => {
                return Some(OrphanProblem {
                    code: "store.corruption",
                    path: render_raw_key(key),
                    message: "stored data cell key does not decode under the tree-cell key grammar"
                        .to_string(),
                });
            }
        };
        let store_id = store.as_str();
        let Some(member_set) = self.members.get(store_id) else {
            return Some(self.orphan(
                store_id,
                &identity,
                &path,
                "a saved root the schema no longer declares",
            ));
        };
        let undeclared = path
            .iter()
            .filter_map(member_segment)
            .any(|member| !member_set.ids.contains(member));
        if undeclared {
            return Some(self.orphan(
                store_id,
                &identity,
                &path,
                "a saved member the schema no longer declares",
            ));
        }
        None
    }

    fn orphan(
        &self,
        store_id: &str,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        reason: &str,
    ) -> OrphanProblem {
        OrphanProblem {
            code: "data.orphan",
            path: self.render_path(store_id, identity, path),
            message: format!("stored data is under {reason}"),
        }
    }

    /// Render a recognizable path for a stored cell: the saved root (or the store
    /// catalog id when the root is gone), the record identity, then each member by
    /// declared name where known, falling back to the catalog id.
    fn render_path(
        &self,
        store_id: &str,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> String {
        let mut text = match self.roots.get(store_id) {
            Some(root) => format!("^{root}"),
            None => format!("data:{store_id}"),
        };
        for key in identity {
            push_key(&mut text, key);
        }
        let names = self.members.get(store_id).map(|set| &set.names);
        for segment in path {
            match segment {
                DataPathSegment::Member(member) => {
                    text.push('.');
                    let id = member.as_str();
                    match names.and_then(|names| names.get(id)) {
                        Some(name) => text.push_str(name),
                        None => text.push_str(id),
                    }
                }
                DataPathSegment::Key(key) => {
                    push_key(&mut text, key);
                }
            }
        }
        text
    }
}

fn collect_members(members: &[CheckedSavedMember], set: &mut MemberSet) {
    for member in members {
        if let Some(catalog_id) = &member.catalog_id {
            set.ids.insert(catalog_id.clone());
            set.names.insert(catalog_id.clone(), member.name.clone());
        }
        collect_members(&member.group_members, set);
    }
}

fn member_segment(segment: &DataPathSegment) -> Option<&str> {
    match segment {
        DataPathSegment::Member(member) => Some(member.as_str()),
        DataPathSegment::Key(_) => None,
    }
}

fn render_raw_key(key: &[u8]) -> String {
    let mut text = String::from("0x");
    crate::push_hex(&mut text, key);
    text
}
