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
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, StoreLeafKind,
    checked_activation_root_places, identity_leaf_key_mismatch,
};

use super::data::{
    DataRecord, checked_places, key_mismatch, push_key, render_data_path, tooling_catalog_id,
    validate_member_value_path, visit_data_records_in_places, visit_data_records_in_places_until,
    visit_place_record_identities_until,
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

pub fn sample_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    limit: usize,
) -> Result<IntegritySample, StoreError> {
    let mut sample = IntegritySample {
        items_checked: 0,
        problems: 0,
        truncated: false,
    };
    if limit == 0 {
        sample.truncated = true;
        return Ok(sample);
    }

    let places = checked_places(program);
    let mut budget = SampleBudget::new(limit);
    sample_record_problems(store, program, &places, &mut budget, &mut sample)?;
    if !budget.truncated {
        sample_incomplete_problems(store, program, &places, &mut budget, &mut sample)?;
    }
    if !budget.truncated {
        sample_orphan_problems(store, &places, &mut budget, &mut sample)?;
    }
    sample.items_checked = budget.used();
    sample.truncated = budget.truncated;
    Ok(sample)
}

fn sample_record_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    budget: &mut SampleBudget,
    sample: &mut IntegritySample,
) -> Result<(), StoreError> {
    let flow = visit_data_records_in_places_until(places, store, |record| {
        let ControlFlow::Continue(()) = budget.claim() else {
            return Ok(ControlFlow::Break(()));
        };
        if check_record(store, program, &record, IntegrityProfile::Report)?.is_some() {
            sample.increment_problems()?;
        }
        Ok(ControlFlow::Continue(()))
    })?;
    if flow.is_break() {
        budget.truncated = true;
    }
    Ok(())
}

fn sample_incomplete_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    budget: &mut SampleBudget,
    sample: &mut IntegritySample,
) -> Result<(), StoreError> {
    let mut visit_item = || Ok(budget.claim());
    let mut report = |problem| {
        sample_problem(problem, sample)?;
        Ok(ControlFlow::Continue(()))
    };
    if visit_incomplete_records_in_places_until(
        store,
        program,
        places,
        &mut visit_item,
        &mut report,
    )?
    .is_break()
    {
        budget.truncated = true;
    }
    Ok(())
}

fn sample_orphan_problems(
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    budget: &mut SampleBudget,
    sample: &mut IntegritySample,
) -> Result<(), StoreError> {
    let schema = DeclaredSchema::from_places(places);
    store.visit_backup_cells_until(|cell| {
        let ControlFlow::Continue(()) = budget.claim() else {
            return Ok(ControlFlow::Break(()));
        };
        if let Some(problem) = schema.classify(store, cell.data_key().clone())? {
            sample_problem(problem, sample)?;
        }
        Ok(ControlFlow::Continue(()))
    })?;
    Ok(())
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

impl IntegritySample {
    fn increment_problems(&mut self) -> Result<(), StoreError> {
        self.problems = self
            .problems
            .checked_add(1)
            .ok_or(StoreError::LimitExceeded {
                limit: "data integrity problem count",
            })?;
        Ok(())
    }
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
            problems = problems.checked_add(1).ok_or(StoreError::LimitExceeded {
                limit: "data integrity problem count",
            })?;
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
                "stored key is a {} where the schema declares {}",
                mismatch.found.name(),
                mismatch.expected.name()
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
                "stored `Id(^{store_root})` reference has a {} key where the schema declares {}",
                found.name(),
                expected.name()
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
    let flow = visit_incomplete_records_in_places_until(
        store,
        program,
        places,
        &mut visit_item,
        &mut report_problem,
    )?;
    if flow.is_break() {
        return Ok(());
    }
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
                if identity_has_key_mismatch(place, identity) {
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

fn identity_has_key_mismatch(place: &CheckedSavedPlace, identity: &[SavedKey]) -> bool {
    identity
        .iter()
        .enumerate()
        .any(|(index, key)| key_mismatch(place.identity_keys[index].scalar, key).is_some())
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
        if key_mismatch(member.key_params[key_index].scalar, &key).is_none() {
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

fn sample_problem(
    _problem: IntegrityProblem,
    sample: &mut IntegritySample,
) -> Result<(), StoreError> {
    sample.increment_problems()
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
        if matches!(kind, DataCellKind::Node) {
            return Ok(None);
        }
        if let Err(reason) = validate_member_value_path(&root.members, &path) {
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
        let mut text = format!("^{}", self.root);
        for key in identity {
            push_key(&mut text, key);
        }
        render_data_path(&mut text, path, &self.names);
        text
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
