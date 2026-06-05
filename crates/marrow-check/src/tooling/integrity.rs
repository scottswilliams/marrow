use std::collections::HashMap;

use marrow_store::StoreError;
use marrow_store::cell::DataCellKey;
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::{DataPathSegment, TreeStore, decode_tree_enum_member};
use marrow_store::value::decode_value;
use marrow_syntax::Diagnose;

use crate::{
    CheckedProgram, CheckedSavedMember, CheckedSavedPlace, StoreLeafKind,
    checked_activation_root_places, checked_saved_root_place, identity_leaf_key_mismatch,
};

use super::data::{DataRecord, push_key, validate_member_value_path, visit_data_records_in_places};

pub const ORPHAN_INTEGRITY_HELP: &str =
    "run `marrow data integrity` after source-native evolution or maintenance repair";

pub fn count_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let places = checked_places(program);
    count_integrity_problems_in_places(store, program, &places)
}

pub fn count_activation_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let places = checked_activation_root_places(program);
    count_integrity_problems_in_places(store, program, &places)
}

fn count_integrity_problems_in_places(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
) -> Result<(usize, usize), StoreError> {
    let mut problems = 0usize;
    let mut records = 0usize;
    visit_integrity_problems_in_places(store, program, places, |outcome| {
        if let IntegrityOutcomeKind::Record = outcome.kind {
            records += 1;
        }
        if outcome.problem.is_some() {
            problems = problems.checked_add(1).ok_or(StoreError::LimitExceeded {
                limit: "data integrity problem count",
            })?;
        }
        Ok(())
    })?;
    Ok((records, problems))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityOutcomeKind {
    Record,
    StoredCell,
}

#[derive(Debug, Clone)]
pub struct IntegrityOutcome {
    pub kind: IntegrityOutcomeKind,
    pub problem: Option<IntegrityProblem>,
}

pub fn visit_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    mut visit: impl FnMut(IntegrityOutcome) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let places = checked_places(program);
    visit_integrity_problems_in_places(store, program, &places, &mut visit)
}

fn visit_integrity_problems_in_places(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    mut visit: impl FnMut(IntegrityOutcome) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    visit_data_records_in_places(places, store, |record| {
        visit(IntegrityOutcome {
            kind: IntegrityOutcomeKind::Record,
            problem: check_record(program, &record),
        })
    })?;
    visit_orphans_in_places(store, places, |orphan| {
        visit(IntegrityOutcome {
            kind: IntegrityOutcomeKind::StoredCell,
            problem: Some(orphan),
        })
    })
}

#[derive(Debug, Clone)]
pub struct IntegrityProblem {
    pub code: &'static str,
    pub path: String,
    pub message: String,
    pub help: Option<&'static str>,
}

impl Diagnose for IntegrityProblem {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
}

fn check_record(program: &CheckedProgram, record: &DataRecord) -> Option<IntegrityProblem> {
    if let Some(mismatch) = &record.key_mismatch {
        return Some(IntegrityProblem {
            code: "data.key_type",
            path: record.path.clone(),
            message: format!(
                "stored key is a {} where the schema declares {}",
                mismatch.found.name(),
                mismatch.expected.name()
            ),
            help: None,
        });
    }
    match &record.leaf {
        StoreLeafKind::Scalar(ty) => {
            decode_value(record.payload.as_bytes(), *ty)
                .is_none()
                .then(|| IntegrityProblem {
                    code: "data.decode",
                    path: record.path.clone(),
                    message: format!("stored value is not a canonical {} form", ty.name()),
                    help: None,
                })
        }
        StoreLeafKind::Identity { store_root, arity } => {
            check_identity_leaf(program, record, store_root, *arity)
        }
        StoreLeafKind::Enum { enum_id } => check_enum_leaf(program, record, *enum_id),
    }
}

fn check_identity_leaf(
    program: &CheckedProgram,
    record: &DataRecord,
    store_root: &str,
    arity: usize,
) -> Option<IntegrityProblem> {
    let Some(keys) = decode_identity_payload_arity(record.payload.as_bytes(), arity) else {
        return Some(IntegrityProblem {
            code: "data.decode",
            path: record.path.clone(),
            message: format!("stored value is not a canonical `Id(^{store_root})` encoding"),
            help: None,
        });
    };
    identity_leaf_key_mismatch(program, store_root, &keys).map(|(expected, found)| {
        IntegrityProblem {
            code: "data.key_type",
            path: record.path.clone(),
            message: format!(
                "stored `Id(^{store_root})` reference has a {} key where the schema declares {}",
                found.name(),
                expected.name()
            ),
            help: None,
        }
    })
}

fn check_enum_leaf(
    program: &CheckedProgram,
    record: &DataRecord,
    enum_id: crate::EnumId,
) -> Option<IntegrityProblem> {
    let enum_fact = program.facts.enum_(enum_id)?;
    let stored = decode_tree_enum_member(record.payload.as_bytes()).ok();
    let Some(stored) = stored else {
        return Some(enum_decode_problem(record, &enum_fact.name));
    };
    if enum_fact.catalog_id.as_deref() != Some(stored.enum_id().as_str()) {
        return Some(enum_decode_problem(record, &enum_fact.name));
    }
    let valid_member = program.facts.enum_members().iter().any(|member| {
        member.enum_id == enum_id
            && member.catalog_id.as_deref() == Some(stored.member_id().as_str())
            && program.facts.enum_member_is_selectable(member.id)
    });
    (!valid_member).then(|| enum_decode_problem(record, &enum_fact.name))
}

fn enum_decode_problem(record: &DataRecord, enum_name: &str) -> IntegrityProblem {
    IntegrityProblem {
        code: "data.decode",
        path: record.path.clone(),
        message: format!("stored value is not a catalog-backed `{enum_name}` member"),
        help: None,
    }
}

fn checked_places(program: &CheckedProgram) -> Vec<CheckedSavedPlace> {
    program
        .facts
        .stores()
        .iter()
        .filter_map(|store| {
            checked_saved_root_place(program, &store.root, marrow_syntax::SourceSpan::default())
        })
        .collect()
}

fn visit_orphans_in_places(
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    mut report: impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let schema = DeclaredSchema::from_places(places);
    store.visit_backup_cells(|cell| {
        if let Some(problem) = schema.classify(cell.data_key().clone()) {
            report(problem)?;
        }
        Ok(())
    })
}

struct DeclaredSchema {
    roots: HashMap<String, DeclaredRoot>,
}

impl DeclaredSchema {
    fn from_places(places: &[CheckedSavedPlace]) -> Self {
        let mut roots = HashMap::new();
        for place in places {
            let Some(store_catalog_id) = place.store_catalog_id.clone() else {
                continue;
            };
            let mut names = HashMap::new();
            collect_member_names(&place.root_members, &mut names);
            roots.insert(
                store_catalog_id,
                DeclaredRoot {
                    root: place.root.clone(),
                    identity_arity: place.identity_keys.len(),
                    members: place.root_members.clone(),
                    names,
                },
            );
        }
        Self { roots }
    }

    fn classify(&self, key: DataCellKey) -> Option<IntegrityProblem> {
        let path = key.path();
        let DataCellKey {
            store,
            identity,
            kind: _,
        } = key;
        let store_id = store.as_str();
        let Some(root) = self.roots.get(store_id) else {
            return Some(orphan_problem(
                render_unknown_path(),
                "a saved root the schema no longer declares",
            ));
        };
        if identity.len() != root.identity_arity {
            return Some(root.orphan(
                &identity,
                &path,
                "a saved root identity shape the schema does not declare",
            ));
        }
        if let Err(reason) = validate_member_value_path(&root.members, &path) {
            return Some(root.orphan(&identity, &path, reason));
        }
        None
    }
}

struct DeclaredRoot {
    root: String,
    identity_arity: usize,
    members: Vec<CheckedSavedMember>,
    names: HashMap<String, String>,
}

impl DeclaredRoot {
    fn orphan(
        &self,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        reason: &'static str,
    ) -> IntegrityProblem {
        orphan_problem(self.render_path(identity, path), reason)
    }

    fn render_path(&self, identity: &[SavedKey], path: &[DataPathSegment]) -> String {
        let mut text = format!("^{}", self.root);
        for key in identity {
            push_key(&mut text, key);
        }
        render_data_path(&mut text, path, Some(&self.names));
        text
    }
}

fn orphan_problem(path: String, reason: &'static str) -> IntegrityProblem {
    IntegrityProblem {
        code: "data.orphan",
        path,
        message: format!("stored data is under {reason}"),
        help: Some(ORPHAN_INTEGRITY_HELP),
    }
}

fn render_unknown_path() -> String {
    "<undeclared saved root>".to_string()
}

fn render_unknown_member(text: &mut String) {
    text.push_str("<undeclared member>");
}

fn render_data_path(
    text: &mut String,
    path: &[DataPathSegment],
    names: Option<&HashMap<String, String>>,
) {
    for segment in path {
        match segment {
            DataPathSegment::Member(member) => {
                text.push('.');
                let id = member.as_str();
                match names.and_then(|names| names.get(id)) {
                    Some(name) => text.push_str(name),
                    None => render_unknown_member(text),
                }
            }
            DataPathSegment::Key(key) => {
                push_key(text, key);
            }
        }
    }
}

fn collect_member_names(members: &[CheckedSavedMember], names: &mut HashMap<String, String>) {
    for member in members {
        if let Some(catalog_id) = &member.catalog_id {
            names.insert(catalog_id.clone(), member.name.clone());
        }
        collect_member_names(&member.group_members, names);
    }
}
