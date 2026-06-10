//! Test-only helpers that query a checked program's facts, gated behind the
//! `test-support` feature so they never enter a normal or release build.
//!
//! These are the single owner of the fact-lookup family the discharge, apply, and
//! CLI-evolution suites share: resolving the bound stable catalog id of a saved
//! member, index, enum, or proposal entry from the checked facts, building a
//! [`CheckedProgram`] from a project already written under a root, and asserting a
//! report carries a diagnostic code. Every concept they name is a marrow-check type,
//! so sharing them needs no new dependency. Helpers that put data on disk through a
//! temporary directory stay in each crate's own test support, since they belong to
//! that crate's pipeline and would drag in a tempfile dependency this crate does not
//! carry.

use std::path::Path;

use marrow_catalog::CatalogEntryKind;
use marrow_project::ProjectConfig;
use marrow_store::cell::CatalogId;

use crate::{
    CheckReport, CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    check_project_with_catalog, checked_saved_root_place,
};

/// The standard single-`src`-root project config the source-driven suites check
/// under, with the well-known accepted-catalog file name.
pub fn test_config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: None,
        store: None,
        tests: Vec::new(),
        accepted_catalog: "marrow.catalog.json".into(),
    }
}

/// Check the project already written under `root`, binding any accepted catalog the
/// fixture wrote to `marrow.catalog.json`, asserting it is clean, and return the checked
/// program. The catalog file is a test fixture spelling of the accepted snapshot the
/// engine-resident store holds in production; reading it through the migration parser
/// lets a suite pin a hand-built accepted catalog the source has moved away from. The
/// caller owns the project directory, so this helper carries no filesystem setup.
pub fn checked(root: &Path) -> CheckedProgram {
    let accepted = read_fixture_catalog(root);
    let (report, program) =
        check_project_with_catalog(root, &test_config(), accepted.as_ref()).expect("check project");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

/// Check the source with no committed catalog, freeze the proposal it produced as the
/// accepted catalog under `root`, then re-check binding it. The returned program's schema
/// is fully committed, so its bound catalog ids address the store, exactly as a
/// state-establishing run leaves them after freezing the baseline. The accepted catalog
/// is also written to the fixture file so a later [`checked`] over a changed source binds
/// it, mirroring the engine-resident snapshot a real project would carry forward.
pub fn commit_then_check(root: &Path) -> CheckedProgram {
    let (report, program) =
        check_project_with_catalog(root, &test_config(), None).expect("check for commit");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program
        .catalog
        .proposal
        .clone()
        .expect("a catalog proposal to commit");
    std::fs::write(root.join("marrow.catalog.json"), proposal.to_json_pretty())
        .expect("freeze fixture catalog");
    let (report, committed) =
        check_project_with_catalog(root, &test_config(), Some(&proposal)).expect("re-check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    committed
}

/// Read the accepted-catalog fixture file under `root`, if a suite wrote one. A missing
/// file is a first-run project with no accepted catalog.
fn read_fixture_catalog(root: &Path) -> Option<marrow_catalog::CatalogMetadata> {
    let path = root.join("marrow.catalog.json");
    let json = std::fs::read_to_string(path).ok()?;
    Some(marrow_catalog::CatalogMetadata::from_json(&json).expect("fixture catalog parses"))
}

/// The checked saved place rooted at `root`, ready to resolve member and index ids.
pub fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

/// Unwrap the bound stable catalog id of a checked fact, naming `label` on absence.
pub fn accepted_catalog_id(id: &Option<String>, label: &str) -> String {
    id.clone()
        .unwrap_or_else(|| panic!("accepted catalog id for `{label}`"))
}

/// The bound store catalog id of a committed place, ready to address store cells.
pub fn store_id_of(place: &CheckedSavedPlace) -> CatalogId {
    CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")).expect("store catalog id")
}

/// The bound stable catalog id of a top-level scalar field member named `name`.
pub fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"));
    accepted_catalog_id(&member.catalog_id, name)
}

/// The bound stable catalog id of an index named `name` on the place.
pub fn index_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let index = place
        .indexes
        .iter()
        .find(|index| index.name == name)
        .unwrap_or_else(|| panic!("checked index `{name}`"));
    accepted_catalog_id(&index.catalog_id, name)
}

/// A top-level group member named `group`, borrowed for its sub-members.
fn group_member<'a>(place: &'a CheckedSavedPlace, group: &str) -> &'a CheckedSavedMember {
    place
        .root_members
        .iter()
        .find(|member| member.name == group && matches!(member.kind, CheckedSavedMemberKind::Group))
        .unwrap_or_else(|| panic!("checked group member `{group}`"))
}

/// The bound stable catalog id of a top-level group member named `group`.
pub fn group_member_catalog_id(place: &CheckedSavedPlace, group: &str) -> String {
    accepted_catalog_id(&group_member(place, group).catalog_id, group)
}

/// The catalog id of a top-level keyed-leaf-layer (`map[K, V]`) member: a `Field` that
/// carries key params, so it is the leaf its entries' values are stored under.
pub fn keyed_leaf_catalog_id(place: &CheckedSavedPlace, map: &str) -> String {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == map
                && !member.key_params.is_empty()
                && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked keyed-leaf member `{map}`"));
    accepted_catalog_id(&member.catalog_id, map)
}

/// The bound stable catalog id of a leaf named `leaf` one level inside `group`.
pub fn nested_member_catalog_id(place: &CheckedSavedPlace, group: &str, leaf: &str) -> String {
    let member = group_member(place, group)
        .group_members
        .iter()
        .find(|member| member.name == leaf)
        .unwrap_or_else(|| panic!("checked nested member `{group}.{leaf}`"));
    accepted_catalog_id(&member.catalog_id, leaf)
}

/// The catalog id of a member reached by an arbitrary name chain from the record root, each
/// segment a layer or group whose sub-members hold the next. Resolves members nested through
/// more than one keyed layer, which the single-level [`nested_member_catalog_id`] cannot reach.
pub fn deep_member_catalog_id(place: &CheckedSavedPlace, chain: &[&str]) -> String {
    let mut members = &place.root_members;
    let mut found = None;
    for segment in chain {
        let member = members
            .iter()
            .find(|member| member.name == *segment)
            .unwrap_or_else(|| panic!("checked nested member `{}`", chain.join(".")));
        found = Some(member);
        members = &member.group_members;
    }
    let member = found.unwrap_or_else(|| panic!("empty member chain"));
    accepted_catalog_id(&member.catalog_id, &chain.join("."))
}

/// The proposal-minted stable id of a brand-new resource member at the given module-qualified
/// catalog path. A member current source adds but the accepted catalog does not yet carry has
/// no bound facts id, so its identity lives only in the catalog proposal; the proposal-aware
/// presence scan keys its verdict by this id.
pub fn new_member_proposal_id(program: &CheckedProgram, path: &str) -> String {
    program
        .catalog
        .proposal
        .as_ref()
        .expect("a catalog proposal")
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::ResourceMember && entry.path == path)
        .unwrap_or_else(|| panic!("proposal entry for `{path}`"))
        .stable_id
        .clone()
}

/// The proposal-minted stable id of the catalog entry at `path`, for any entry kind.
pub fn proposal_catalog_id(program: &CheckedProgram, path: &str) -> String {
    program
        .catalog
        .proposal
        .as_ref()
        .expect("catalog proposal")
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("proposal catalog entry `{path}`"))
        .stable_id
        .clone()
}

/// The stable catalog id the checked program bound to the enum named `name`, so a
/// hand-built accepted catalog records the identity-aware leaf token (`enum:<id>`) the
/// discharge compares against, not a source spelling.
pub fn enum_catalog_id(program: &CheckedProgram, name: &str) -> String {
    let enum_fact = program
        .facts
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == name)
        .unwrap_or_else(|| panic!("checked enum `{name}`"));
    accepted_catalog_id(&enum_fact.catalog_id, name)
}

/// The stable catalog id of the enum member `enum_name::member`, so a test can seed a
/// stored enum value (its enum id plus the selected member id) the way the runtime
/// write path does.
pub fn enum_member_catalog_id(program: &CheckedProgram, enum_name: &str, member: &str) -> String {
    let enum_id = program
        .facts
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == enum_name)
        .unwrap_or_else(|| panic!("checked enum `{enum_name}`"))
        .id;
    let member_fact = program
        .facts
        .enum_members()
        .iter()
        .find(|member_fact| member_fact.enum_id == enum_id && member_fact.name == member)
        .unwrap_or_else(|| panic!("checked enum member `{enum_name}::{member}`"));
    accepted_catalog_id(&member_fact.catalog_id, member)
}

/// Assert `report` carries a diagnostic whose code is `code`, dumping every
/// diagnostic on failure. The single oracle the runtime and CLI checker-reject
/// helpers delegate to.
pub fn assert_checker_code(report: &CheckReport, code: &str) {
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code),
        "expected checker diagnostic {code}, got {:#?}",
        report.diagnostics
    );
}
