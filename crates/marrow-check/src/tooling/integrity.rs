use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

use marrow_catalog::{CatalogEntryKind, CatalogLifecycle};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::cell::{DataCellKey, DataCellKind};
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::{DataPathSegment, TreeStore, decode_tree_enum_member};
use marrow_store::value::decode_value;
use marrow_syntax::Diagnose;

use crate::{
    CheckedProgram, CheckedSavedIndex, CheckedSavedMember, CheckedSavedMemberKind,
    CheckedSavedPlace, StoreIndexKeySource, StoreLeafKind, StoredValueMeaning,
    checked_activation_root_places, identity_leaf_key_mismatch,
};

use super::data::StampedData;
use super::data::{
    DataRecord, checked_places, push_key, render_data_path, stored_key_mismatch,
    tooling_catalog_id, validate_member_path_node, validate_member_value_path,
    visit_data_records_in_places, visit_data_records_in_places_until,
    visit_place_record_identities_until, with_stamped_read,
};

const ORPHAN_INTEGRITY_HELP: &str =
    "run `marrow data integrity` after source-native evolution or maintenance repair";

#[derive(Clone, Copy)]
enum IntegrityProfile {
    Report,
    Activation,
}

impl IntegrityProfile {
    fn checks_dangling_refs(self) -> bool {
        matches!(self, Self::Report)
    }
}

pub fn count_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let places = checked_places(program);
    // The record and orphan passes below traverse only the data family. The derived
    // index family is verified two independent ways: a structural decode and seek
    // re-descent that catches a malformed index node, and a completeness cross-check
    // that catches a node whose damage silently truncates an index range so its
    // entries vanish from every enumeration. Both run before the data passes so an
    // index-corrupt store fails closed rather than being blessed while an index-driven
    // read under-returns. The data family's own silent truncation or value tampering is
    // caught by the structural-digest cross-check, the independent oracle a data-page
    // flip cannot move with the cells.
    store.verify_structural_digests()?;
    store.verify_index_readable()?;
    verify_index_completeness(store, &places)?;
    count_integrity_problems_in_places(store, program, &places, IntegrityProfile::Report)
}

pub fn count_activation_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let places = checked_activation_root_places(program);
    count_integrity_problems_in_places(store, program, &places, IntegrityProfile::Activation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegritySample {
    pub items_checked: usize,
    pub problems: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct IntegrityProblemSample {
    pub items_checked: usize,
    pub problems: Vec<IntegrityProblem>,
    pub truncated: bool,
}

pub fn sample_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    limit: usize,
) -> Result<IntegritySample, StoreError> {
    let mut problems = 0usize;
    let scan = sample_integrity_problem_items(store, program, limit, |_problem| {
        increment_problem_count(&mut problems)
    })?;
    Ok(IntegritySample {
        items_checked: scan.items_checked,
        problems,
        truncated: scan.truncated,
    })
}

pub fn sample_integrity_problem_details(
    store: &TreeStore,
    program: &CheckedProgram,
    limit: usize,
) -> Result<IntegrityProblemSample, StoreError> {
    let mut problems = Vec::new();
    let scan = sample_integrity_problem_items(store, program, limit, |problem| {
        problems.push(problem);
        Ok(())
    })?;
    Ok(IntegrityProblemSample {
        items_checked: scan.items_checked,
        problems,
        truncated: scan.truncated,
    })
}

pub fn stamped_integrity_problem_details(
    program: &CheckedProgram,
    store: &TreeStore,
    limit: usize,
) -> Result<StampedData<IntegrityProblemSample>, StoreError> {
    with_stamped_read(program, store, |store| {
        sample_integrity_problem_details(store, program, limit)
    })
}

fn sample_integrity_problem_items(
    store: &TreeStore,
    program: &CheckedProgram,
    limit: usize,
    mut on_problem: impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<IntegrityProblemScan, StoreError> {
    let places = checked_places(program);
    let mut budget = SampleBudget::new(limit);
    sample_record_problems(store, program, &places, &mut budget, &mut on_problem)?;
    if !budget.truncated {
        sample_incomplete_problems(store, program, &places, &mut budget, &mut on_problem)?;
    }
    if !budget.truncated {
        sample_orphan_problems(store, &places, &mut budget, &mut on_problem)?;
    }
    Ok(IntegrityProblemScan {
        items_checked: budget.used(),
        truncated: budget.truncated,
    })
}

fn sample_record_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    budget: &mut SampleBudget,
    on_problem: &mut impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let _ = visit_data_records_in_places_until(places, store, |record| {
        let ControlFlow::Continue(()) = budget.claim() else {
            return Ok(ControlFlow::Break(()));
        };
        if let Some(problem) = check_record(store, program, &record, IntegrityProfile::Report)? {
            on_problem(problem)?;
        }
        Ok(ControlFlow::Continue(()))
    })?;
    Ok(())
}

fn sample_incomplete_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    budget: &mut SampleBudget,
    on_problem: &mut impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut visit_item = || Ok(budget.claim());
    let mut report = |problem| {
        on_problem(problem)?;
        Ok(ControlFlow::Continue(()))
    };
    let _ = visit_incomplete_records_in_places_until(
        store,
        program,
        places,
        &mut visit_item,
        &mut report,
    )?;
    Ok(())
}

fn sample_orphan_problems(
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    budget: &mut SampleBudget,
    on_problem: &mut impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let schema = DeclaredSchema::from_places(places);
    store.visit_backup_cells_until(|cell| {
        let ControlFlow::Continue(()) = budget.claim() else {
            return Ok(ControlFlow::Break(()));
        };
        if let Some(problem) = schema.classify(store, cell.data_key().clone())? {
            on_problem(problem)?;
        }
        Ok(ControlFlow::Continue(()))
    })?;
    Ok(())
}

struct IntegrityProblemScan {
    items_checked: usize,
    truncated: bool,
}

struct SampleBudget {
    limit: usize,
    visited: usize,
    truncated: bool,
}

impl SampleBudget {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            visited: 0,
            truncated: false,
        }
    }

    fn claim(&mut self) -> ControlFlow<()> {
        if self.visited == self.limit {
            self.truncated = true;
            ControlFlow::Break(())
        } else {
            self.visited += 1;
            ControlFlow::Continue(())
        }
    }

    fn used(&self) -> usize {
        self.visited
    }
}

fn increment_problem_count(problems: &mut usize) -> Result<(), StoreError> {
    *problems = problems.checked_add(1).ok_or(StoreError::LimitExceeded {
        limit: "data integrity problem count",
    })?;
    Ok(())
}

fn count_integrity_problems_in_places(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    profile: IntegrityProfile,
) -> Result<(usize, usize), StoreError> {
    let mut problems = 0usize;
    let mut records = 0usize;
    visit_integrity_problems_in_places(store, program, places, profile, |outcome| {
        if outcome.is_record {
            records += 1;
        }
        if outcome.problem.is_some() {
            increment_problem_count(&mut problems)?;
        }
        Ok(())
    })?;
    Ok((records, problems))
}

#[derive(Debug, Clone)]
pub struct IntegrityOutcome {
    /// True when this outcome counts a stored declared value, false for findings
    /// that come from structure-only checks or undeclared stored cells.
    pub is_record: bool,
    pub problem: Option<IntegrityProblem>,
}

pub fn visit_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    mut visit: impl FnMut(IntegrityOutcome) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let places = checked_places(program);
    visit_integrity_problems_in_places(
        store,
        program,
        &places,
        IntegrityProfile::Report,
        &mut visit,
    )
}

fn visit_integrity_problems_in_places(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    profile: IntegrityProfile,
    mut visit: impl FnMut(IntegrityOutcome) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    visit_data_records_in_places(places, store, |record| {
        visit(IntegrityOutcome {
            is_record: true,
            problem: check_record(store, program, &record, profile)?,
        })
    })?;
    visit_incomplete_records_in_places(store, program, places, |problem| {
        visit(IntegrityOutcome {
            is_record: false,
            problem: Some(problem),
        })
    })?;
    visit_orphans_in_places(store, places, |orphan| {
        visit(IntegrityOutcome {
            is_record: false,
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
    pub incomplete: Option<IncompleteIntegrityProblem>,
    pub dangling_ref: Option<DanglingRefIntegrityProblem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteIntegrityProblem {
    pub store_catalog_id: CatalogId,
    pub record_identity: Vec<SavedKey>,
    pub parent_path: Vec<DataPathSegment>,
    pub missing_member_catalog_id: CatalogId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingRefIntegrityProblem {
    pub containing_identity: Vec<SavedKey>,
    pub field_catalog_id: CatalogId,
    pub referenced_root: String,
    pub referenced_identity: Vec<SavedKey>,
}

impl Diagnose for IntegrityProblem {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
}

fn data_problem(
    code: &'static str,
    path: String,
    message: String,
    help: Option<&'static str>,
) -> IntegrityProblem {
    IntegrityProblem {
        code,
        path,
        message,
        help,
        incomplete: None,
        dangling_ref: None,
    }
}

fn check_record(
    store: &TreeStore,
    program: &CheckedProgram,
    record: &DataRecord,
    profile: IntegrityProfile,
) -> Result<Option<IntegrityProblem>, StoreError> {
    if let Some(mismatch) = &record.key_mismatch {
        return Ok(Some(data_problem(
            "data.key_type",
            record.path.clone(),
            format!(
                "stored key is {} where the schema declares {}",
                mismatch.found.indefinite(),
                mismatch.expected.indefinite()
            ),
            None,
        )));
    }
    match &record.leaf {
        StoreLeafKind::Scalar(ty) => Ok(decode_value(record.payload.as_bytes(), *ty)
            .is_none()
            .then(|| {
                data_problem(
                    "data.decode",
                    record.path.clone(),
                    format!("stored value is not a canonical {} form", ty.name()),
                    None,
                )
            })),
        StoreLeafKind::Identity { store_root, arity } => {
            check_identity_leaf(store, program, record, store_root, *arity, profile)
        }
        StoreLeafKind::Enum { enum_id } => Ok(check_enum_leaf(program, record, *enum_id)),
    }
}

fn check_identity_leaf(
    store: &TreeStore,
    program: &CheckedProgram,
    record: &DataRecord,
    store_root: &str,
    arity: usize,
    profile: IntegrityProfile,
) -> Result<Option<IntegrityProblem>, StoreError> {
    let Some(keys) = decode_identity_payload_arity(record.payload.as_bytes(), arity) else {
        return Ok(Some(data_problem(
            "data.decode",
            record.path.clone(),
            format!("stored value is not a canonical `Id(^{store_root})` encoding"),
            None,
        )));
    };
    if let Some((expected, found)) = identity_leaf_key_mismatch(program, store_root, &keys) {
        return Ok(Some(data_problem(
            "data.key_type",
            record.path.clone(),
            format!(
                "stored `Id(^{store_root})` reference has {} key where the schema declares {}",
                found.indefinite(),
                expected.indefinite()
            ),
            None,
        )));
    }
    if !profile.checks_dangling_refs() {
        return Ok(None);
    }
    let Some(target_store) = program.facts.store_by_root(store_root) else {
        return Ok(None);
    };
    let Some(target_store_id) = tooling_catalog_id(&target_store.catalog_id, "store")? else {
        return Ok(None);
    };
    if store.data_subtree_exists(&target_store_id, &keys, &[])? {
        return Ok(None);
    }
    Ok(Some(dangling_ref_problem(record, store_root, keys)))
}

fn dangling_ref_problem(
    record: &DataRecord,
    referenced_root: &str,
    referenced_identity: Vec<SavedKey>,
) -> IntegrityProblem {
    IntegrityProblem {
        code: "data.dangling_ref",
        path: record.path.clone(),
        message: format!("stored `Id(^{referenced_root})` reference points to no saved record"),
        help: None,
        incomplete: None,
        dangling_ref: Some(DanglingRefIntegrityProblem {
            containing_identity: record.identity.clone(),
            field_catalog_id: record.field_catalog_id.clone(),
            referenced_root: referenced_root.to_string(),
            referenced_identity,
        }),
    }
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
    data_problem(
        "data.decode",
        record.path.clone(),
        format!("stored value is not a catalog-backed `{enum_name}` member"),
        None,
    )
}

fn visit_incomplete_records_in_places(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    mut report: impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut visit_item = || Ok(ControlFlow::Continue(()));
    let mut report_problem = |problem| {
        report(problem)?;
        Ok(ControlFlow::Continue(()))
    };
    let _ = visit_incomplete_records_in_places_until(
        store,
        program,
        places,
        &mut visit_item,
        &mut report_problem,
    )?;
    Ok(())
}

fn visit_incomplete_records_in_places_until(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    visit_item: &mut impl FnMut() -> Result<ControlFlow<()>, StoreError>,
    report: &mut impl FnMut(IntegrityProblem) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<()>, StoreError> {
    let accepted_members = accepted_member_ids(program);
    for place in places {
        let names = member_names(&place.root_members);
        let flow =
            visit_place_record_identities_until(place, store, &mut |place, store_id, identity| {
                let ControlFlow::Continue(()) = visit_item()? else {
                    return Ok(ControlFlow::Break(()));
                };
                if identity_has_key_mismatch(place, identity)? {
                    return Ok(ControlFlow::Continue(()));
                }
                let context = CompletenessContext {
                    store,
                    store_id,
                    root: &place.root,
                    names: &names,
                    accepted_members: &accepted_members,
                };
                let mut parent_path = Vec::new();
                visit_members_complete_until(
                    &context,
                    identity,
                    &place.root_members,
                    &mut parent_path,
                    visit_item,
                    report,
                )
            })?;
        if flow.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}

fn accepted_member_ids(program: &CheckedProgram) -> HashSet<&str> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember
                && entry.lifecycle == CatalogLifecycle::Active
        })
        .map(|entry| entry.stable_id.as_str())
        .collect()
}

fn identity_has_key_mismatch(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
) -> Result<bool, StoreError> {
    for (index, key) in identity.iter().enumerate() {
        if stored_key_mismatch(place.identity_keys[index].scalar, key)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

struct CompletenessContext<'a> {
    store: &'a TreeStore,
    store_id: &'a CatalogId,
    root: &'a str,
    names: &'a HashMap<String, String>,
    accepted_members: &'a HashSet<&'a str>,
}

fn visit_members_complete_until(
    context: &CompletenessContext<'_>,
    identity: &[SavedKey],
    members: &[CheckedSavedMember],
    parent_path: &mut Vec<DataPathSegment>,
    visit_item: &mut impl FnMut() -> Result<ControlFlow<()>, StoreError>,
    report: &mut impl FnMut(IntegrityProblem) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<()>, StoreError> {
    for member in members {
        let Some(member_id) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
            continue;
        };
        parent_path.push(DataPathSegment::Member(member_id.clone()));
        let flow = if member.key_params.is_empty() {
            match &member.kind {
                CheckedSavedMemberKind::Field { .. } => visit_field_complete_until(
                    context,
                    identity,
                    member,
                    &member_id,
                    parent_path,
                    visit_item,
                    report,
                )?,
                CheckedSavedMemberKind::Group => visit_members_complete_until(
                    context,
                    identity,
                    &member.group_members,
                    parent_path,
                    visit_item,
                    report,
                )?,
            }
        } else if matches!(member.kind, CheckedSavedMemberKind::Group) {
            visit_keyed_group_entries_until(
                context,
                identity,
                member,
                0,
                parent_path,
                visit_item,
                report,
            )?
        } else {
            ControlFlow::Continue(())
        };
        parent_path.pop();
        if flow.is_break() {
            return Ok(flow);
        }
    }
    Ok(ControlFlow::Continue(()))
}

fn visit_field_complete_until(
    context: &CompletenessContext<'_>,
    identity: &[SavedKey],
    member: &CheckedSavedMember,
    member_id: &CatalogId,
    field_path: &[DataPathSegment],
    visit_item: &mut impl FnMut() -> Result<ControlFlow<()>, StoreError>,
    report: &mut impl FnMut(IntegrityProblem) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<()>, StoreError> {
    let CheckedSavedMemberKind::Field { required } = &member.kind else {
        return Ok(ControlFlow::Continue(()));
    };
    if !*required || member.leaf.is_none() || !context.accepted_members.contains(member_id.as_str())
    {
        return Ok(ControlFlow::Continue(()));
    }
    let ControlFlow::Continue(()) = visit_item()? else {
        return Ok(ControlFlow::Break(()));
    };
    if context
        .store
        .read_data_value(context.store_id, identity, field_path)?
        .is_some()
    {
        return Ok(ControlFlow::Continue(()));
    }
    let parent_path = field_path[..field_path.len() - 1].to_vec();
    report(incomplete_problem(
        context,
        identity,
        parent_path,
        member_id.clone(),
    ))
}

fn visit_keyed_group_entries_until(
    context: &CompletenessContext<'_>,
    identity: &[SavedKey],
    member: &CheckedSavedMember,
    key_index: usize,
    parent_path: &mut Vec<DataPathSegment>,
    visit_item: &mut impl FnMut() -> Result<ControlFlow<()>, StoreError>,
    report: &mut impl FnMut(IntegrityProblem) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<()>, StoreError> {
    if key_index == member.key_params.len() {
        return visit_members_complete_until(
            context,
            identity,
            &member.group_members,
            parent_path,
            visit_item,
            report,
        );
    }

    let mut child = context
        .store
        .data_first_child(context.store_id, identity, parent_path)?;
    while let Some(key) = child {
        let next_after = key.clone();
        if stored_key_mismatch(member.key_params[key_index].scalar, &key)?.is_none() {
            let ControlFlow::Continue(()) = visit_item()? else {
                return Ok(ControlFlow::Break(()));
            };
            parent_path.push(DataPathSegment::Key(key));
            let flow = visit_keyed_group_entries_until(
                context,
                identity,
                member,
                key_index + 1,
                parent_path,
                visit_item,
                report,
            )?;
            parent_path.pop();
            if flow.is_break() {
                return Ok(flow);
            }
        }
        child =
            context
                .store
                .data_next_child(context.store_id, identity, parent_path, &next_after)?;
    }
    Ok(ControlFlow::Continue(()))
}

fn incomplete_problem(
    context: &CompletenessContext<'_>,
    identity: &[SavedKey],
    parent_path: Vec<DataPathSegment>,
    missing_member_catalog_id: CatalogId,
) -> IntegrityProblem {
    let mut full_path = parent_path.clone();
    full_path.push(DataPathSegment::Member(missing_member_catalog_id.clone()));
    IntegrityProblem {
        code: "data.incomplete",
        path: render_problem_path(context.root, identity, &full_path, context.names),
        message: "required saved member is absent".to_string(),
        help: None,
        incomplete: Some(IncompleteIntegrityProblem {
            store_catalog_id: context.store_id.clone(),
            record_identity: identity.to_vec(),
            parent_path,
            missing_member_catalog_id,
        }),
        dangling_ref: None,
    }
}

fn render_problem_path(
    root: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    names: &HashMap<String, String>,
) -> String {
    let mut text = format!("^{root}");
    for key in identity {
        push_key(&mut text, key);
    }
    render_data_path(&mut text, path, names);
    text
}

/// Count the stored cells the current source-derived schema no longer declares — the same
/// `data.orphan` cells `data integrity` reports. The source-driven inspection commands
/// (`data stats`, `data dump`) walk only declared places, so these cells are invisible to them;
/// the count lets those commands warn that their reduced output omits intact data rather than
/// under-reporting silently.
pub fn count_orphan_cells(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<usize, StoreError> {
    let places = checked_places(program);
    let mut orphans = 0usize;
    visit_orphans_in_places(store, &places, |_orphan| {
        increment_problem_count(&mut orphans)
    })?;
    Ok(orphans)
}

fn visit_orphans_in_places(
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    mut report: impl FnMut(IntegrityProblem) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let schema = DeclaredSchema::from_places(places);
    store.visit_backup_cells(|cell| {
        if let Some(problem) = schema.classify(store, cell.data_key().clone())? {
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
            roots.insert(
                store_catalog_id,
                DeclaredRoot {
                    root: place.root.clone(),
                    identity_arity: place.identity_keys.len(),
                    members: place.root_members.clone(),
                    names: member_names(&place.root_members),
                },
            );
        }
        Self { roots }
    }

    fn classify(
        &self,
        store: &TreeStore,
        key: DataCellKey,
    ) -> Result<Option<IntegrityProblem>, StoreError> {
        let path = key.path();
        let DataCellKey {
            store: data_store,
            identity,
            kind,
        } = key;
        let store_id = data_store.as_str();
        let Some(root) = self.roots.get(store_id) else {
            return Ok(Some(orphan_problem(
                render_unknown_path(),
                "a saved root the schema no longer declares",
            )));
        };
        if identity.len() != root.identity_arity {
            return Ok(Some(root.orphan(
                &identity,
                &path,
                "a saved root identity shape the schema does not declare",
            )));
        }
        let validated = match kind {
            DataCellKind::Node => return Ok(None),
            DataCellKind::PathNode { .. } => validate_member_path_node(&root.members, &path),
            DataCellKind::Leaf { .. }
            | DataCellKind::Sequence { .. }
            | DataCellKind::Value { .. } => validate_member_value_path(&root.members, &path),
        };
        if let Err(reason) = validated {
            return Ok(Some(root.orphan(&identity, &path, reason)));
        }
        if store
            .read_data_value(&data_store, &identity, &[])?
            .is_none()
        {
            return Ok(Some(root.orphan(
                &identity,
                &path,
                "a saved record identity node the store does not hold",
            )));
        }
        Ok(None)
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
        render_problem_path(&self.root, identity, path, &self.names)
    }
}

fn orphan_problem(path: String, reason: &'static str) -> IntegrityProblem {
    data_problem(
        "data.orphan",
        path,
        format!("stored data is under {reason}"),
        Some(ORPHAN_INTEGRITY_HELP),
    )
}

fn render_unknown_path() -> String {
    "<undeclared saved root>".to_string()
}

fn member_names(members: &[CheckedSavedMember]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    collect_member_names(members, &mut names);
    names
}

fn collect_member_names(members: &[CheckedSavedMember], names: &mut HashMap<String, String>) {
    for member in members {
        if let Some(catalog_id) = &member.catalog_id {
            names.insert(catalog_id.clone(), member.name.clone());
        }
        collect_member_names(&member.group_members, names);
    }
}

/// Verify both store families are complete for a store the schema describes: every
/// committed cell against its durable per-root structural digest, the committed catalog the
/// store presents against the independent `marrow.lock` witness, and the derived index
/// family by structural decode and re-descent ([`verify_index_readable`]) plus the
/// cross-check below. `recover` and `backup` run this so they fail closed on a store whose
/// cells were silently truncated or rewritten, whose committed roots were rolled back below
/// the lock, or whose index-driven reads under-return, rather than blessing or archiving it.
pub fn verify_store_completeness(
    store: &TreeStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<(), StoreError> {
    verify_store_roots_against_lock(store, lock)?;
    store.verify_structural_digests()?;
    store.verify_index_readable()?;
    verify_index_completeness(store, &checked_places(program))
}

/// Cross-check the catalog the store presents against the committed `marrow.lock`.
///
/// The per-root structural digest cannot witness a corruption that drops the anchor
/// itself: a flip in the commit metadata region that rolls the store back to its empty
/// initial state presents zero records and zero anchors, so the anchor pass visits nothing
/// and passes vacuously. The independent witness is the lock, a separate durable file that
/// records the committed accepted roots. Every active accepted root the lock records must
/// still be present in the catalog the store presents; a store that presents fewer roots
/// than the lock committed has lost durable identity to a rollback, failed closed as
/// corruption.
///
/// The check keys on the accepted-root set, not the epoch number, so a store legitimately
/// behind an ahead lock (a teammate's committed activation seeded into a fresh checkout)
/// still carries the same active roots and passes. When no committed lock exists the store
/// has no recorded baseline to contradict, which is the separate missing-lock case left to
/// the caller, not a corruption.
pub fn verify_store_roots_against_lock(
    store: &TreeStore,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<(), StoreError> {
    let Some(lock) = lock else {
        return Ok(());
    };
    let mut committed_roots = lock
        .entries
        .iter()
        .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
        .map(|entry| (entry.kind, entry.path.as_str()))
        .peekable();
    if committed_roots.peek().is_none() {
        return Ok(());
    }
    let presented: HashSet<(CatalogEntryKind, String)> = store
        .read_catalog_snapshot()?
        .map(|snapshot| {
            snapshot
                .entries
                .iter()
                .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
                .map(|entry| (entry.kind, entry.path.clone()))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if committed_roots.any(|(kind, path)| !presented.contains(&(kind, path.to_string()))) {
        return Err(StoreError::Corruption {
            message: "the store presents fewer committed roots than its lock recorded".into(),
        });
    }
    Ok(())
}

/// Cross-check every declared index against the data records that derive it.
///
/// A structural scan of the index family cannot detect entries that were silently
/// dropped from enumeration: if a damaged page makes the backend's range scan
/// truncate an index range, the missing entries are absent from both the linear
/// decode and the seek re-descent, so neither pass can miss what it never sees. The
/// data records are the independent oracle. Each record whose indexed columns are all
/// populated publishes exactly one entry per index, the same rule the runtime write
/// path applies, so the expected entry count is derivable from the data family alone.
/// A count that disagrees with the entries the index family actually enumerates is a
/// dropped or orphaned entry: backend damage, failed closed as corruption.
fn verify_index_completeness(
    store: &TreeStore,
    places: &[CheckedSavedPlace],
) -> Result<(), StoreError> {
    let mut expected: HashMap<CatalogId, usize> = HashMap::new();
    for place in places {
        let columns = index_completeness_columns(place)?;
        if columns.is_empty() {
            continue;
        }
        // Seed every resolvable index at zero so an index whose records publish no
        // entry is still cross-checked: an orphaned entry under it then reads as a
        // count above its derived zero.
        for index in &columns {
            expected.entry(index.id.clone()).or_default();
        }
        let _ = visit_place_record_identities_until(place, store, &mut |_, store_id, identity| {
            for index in &columns {
                if index.record_publishes(store, store_id, identity)? {
                    *expected.entry(index.id.clone()).or_default() += 1;
                }
            }
            Ok(ControlFlow::Continue(()))
        })?;
    }

    for (index_id, expected_count) in expected {
        let actual = enumerated_index_entry_count(store, &index_id)?;
        if actual != expected_count {
            return Err(StoreError::Corruption {
                message: "an index holds a different number of entries than its records derive"
                    .into(),
            });
        }
    }
    Ok(())
}

/// One declared index paired with the readers that resolve each of its key columns
/// from a record. An index whose key shape the cross-check cannot resolve is skipped
/// rather than counted, so a future nested or keyed-layer index column does not force
/// a false corruption; the structural pass still guards such an index's bytes.
struct IndexCompleteness {
    id: CatalogId,
    columns: Vec<IndexColumn>,
}

/// How to read one index key column from a record: an identity key by its tuple
/// position, or a top-level member cell decoded by its stored meaning. These mirror
/// the runtime index-write derivation: a record publishes an entry only when every
/// column is present, so an absent member column means no entry, never a default.
enum IndexColumn {
    Identity {
        position: usize,
    },
    Member {
        path: DataPathSegment,
        meaning: StoredValueMeaning,
    },
}

impl IndexCompleteness {
    /// Whether the record at `identity` publishes an entry for this index: every
    /// column must read a present, decodable value. A member column that is absent or
    /// whose bytes do not decode under its meaning yields no entry, the same outcome
    /// the runtime write path produces, so the expected count matches what was written.
    fn record_publishes(
        &self,
        store: &TreeStore,
        store_id: &CatalogId,
        identity: &[SavedKey],
    ) -> Result<bool, StoreError> {
        for column in &self.columns {
            match column {
                IndexColumn::Identity { position } => {
                    if identity.get(*position).is_none() {
                        return Ok(false);
                    }
                }
                IndexColumn::Member { path, meaning } => {
                    let Some(bytes) =
                        store.read_data_value(store_id, identity, std::slice::from_ref(path))?
                    else {
                        return Ok(false);
                    };
                    if meaning.stored_key(&bytes).is_none() {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
}

/// Resolve each index a place declares to its key-column readers, dropping any index
/// with a catalog id or column the cross-check cannot resolve. Every v0.1 index over
/// identity keys or top-level plain fields resolves here.
fn index_completeness_columns(
    place: &CheckedSavedPlace,
) -> Result<Vec<IndexCompleteness>, StoreError> {
    let mut indexes = Vec::new();
    for index in &place.indexes {
        let Some(index_catalog_id) = index.catalog_id.as_deref() else {
            continue;
        };
        if let Some(columns) = resolve_index_columns(place, index)? {
            indexes.push(IndexCompleteness {
                id: CatalogId::new(index_catalog_id.to_string()).map_err(|_| {
                    StoreError::Corruption {
                        message: "index catalog id is malformed".into(),
                    }
                })?,
                columns,
            });
        }
    }
    Ok(indexes)
}

fn resolve_index_columns(
    place: &CheckedSavedPlace,
    index: &CheckedSavedIndex,
) -> Result<Option<Vec<IndexColumn>>, StoreError> {
    let mut columns = Vec::with_capacity(index.keys.len());
    for key in &index.keys {
        match key.source {
            StoreIndexKeySource::IdentityKey => {
                let Some(position) = place
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)
                else {
                    return Ok(None);
                };
                columns.push(IndexColumn::Identity { position });
            }
            StoreIndexKeySource::ResourceMember(_) => {
                let Some(member) = place
                    .root_members
                    .iter()
                    .find(|member| member.name == key.name && member.is_plain_field())
                else {
                    return Ok(None);
                };
                let Some(member_catalog_id) = member.catalog_id.as_deref() else {
                    return Ok(None);
                };
                let member_id = CatalogId::new(member_catalog_id.to_string()).map_err(|_| {
                    StoreError::Corruption {
                        message: "index member catalog id is malformed".into(),
                    }
                })?;
                columns.push(IndexColumn::Member {
                    path: DataPathSegment::Member(member_id),
                    meaning: key.value_meaning.clone(),
                });
            }
        }
    }
    Ok(Some(columns))
}

/// Count the entries the index family enumerates under one index. This is the
/// possibly-truncated side of the cross-check: a silently dropped range lowers this
/// count below what the records derive, surfacing the corruption.
fn enumerated_index_entry_count(
    store: &TreeStore,
    index_id: &CatalogId,
) -> Result<usize, StoreError> {
    let mut count = 0usize;
    store.for_each_index_entry(index_id, &mut |_, _, _| {
        count = count.checked_add(1).ok_or(StoreError::LimitExceeded {
            limit: "index entry count",
        })?;
        Ok(())
    })?;
    Ok(count)
}
