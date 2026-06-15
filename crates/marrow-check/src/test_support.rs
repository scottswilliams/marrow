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
//! that crate's pipeline and would drag filesystem setup into this fact-lookup
//! helper.

use std::{fs, io, path::Path};

use marrow_catalog::CatalogEntryKind;
use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};
use marrow_store::cell::CatalogId;

use crate::{
    CheckReport, CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    check_project_with_catalog, checked_saved_root_place,
};

type TestSupportResult<T> = Result<T, Box<dyn std::error::Error>>;

/// The standard single-`src`-root project config the source-driven suites check under.
pub fn test_config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: None,
        store: StoreConfig {
            backend: StoreBackend::Memory,
            data_dir: None,
        },
        tests: Vec::new(),
    }
}

/// Check the project already written under `root`, binding any accepted catalog the
/// fixture wrote to `marrow.catalog.json`, asserting it is clean, and return the checked
/// program. The file is the same committed source-tree artifact the CLI binds in
/// production; reading it through the migration parser lets a suite pin a hand-built
/// accepted catalog the source has moved away from. The caller owns the project
/// directory, so this helper carries no filesystem setup.
pub fn checked(root: &Path) -> TestSupportResult<CheckedProgram> {
    let accepted = read_fixture_catalog(root)?;
    let (report, program) = check_project_with_catalog(root, &test_config(), accepted.as_ref())?;
    ensure_clean_report(&report)?;
    Ok(program)
}

/// Check the source with no committed catalog, freeze the proposal it produced as the
/// accepted catalog under `root`, then re-check binding it. The returned program's schema
/// is fully committed, so its bound catalog ids address the store, exactly as a
/// state-establishing run leaves them after freezing the baseline. The accepted catalog
/// is also written to the fixture file so a later [`checked`] over a changed source binds
/// it, mirroring the committed `marrow.catalog.json` artifact a real project would carry
/// forward.
pub fn commit_then_check(root: &Path) -> TestSupportResult<CheckedProgram> {
    let (report, program) = check_project_with_catalog(root, &test_config(), None)?;
    ensure_clean_report(&report)?;
    let proposal = program
        .catalog
        .proposal
        .clone()
        .ok_or_else(|| io::Error::other("a catalog proposal to commit"))?;
    fs::write(root.join("marrow.catalog.json"), proposal.to_json_pretty()?)?;
    let (report, committed) = check_project_with_catalog(root, &test_config(), Some(&proposal))?;
    ensure_clean_report(&report)?;
    Ok(committed)
}

/// Read the accepted-catalog fixture file under `root`, if a suite wrote one. A missing
/// file is a first-run project with no accepted catalog.
fn read_fixture_catalog(root: &Path) -> TestSupportResult<Option<marrow_catalog::CatalogMetadata>> {
    let path = root.join("marrow.catalog.json");
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let catalog = marrow_catalog::CatalogMetadata::from_json(&json).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("fixture catalog parses at {}: {error}", path.display()),
        )
    })?;
    Ok(Some(catalog))
}

fn ensure_clean_report(report: &CheckReport) -> TestSupportResult<()> {
    if report.has_errors() {
        return Err(io::Error::other(format!(
            "checker report has errors: {:#?}",
            report.diagnostics
        ))
        .into());
    }
    Ok(())
}

/// The checked saved place rooted at `root`, ready to resolve member and index ids.
pub fn root_place(program: &CheckedProgram, root: &str) -> TestSupportResult<CheckedSavedPlace> {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| io::Error::other(format!("checked saved root place `{root}`")).into())
}

/// Unwrap the bound stable catalog id of a checked fact, naming `label` on absence.
pub fn accepted_catalog_id(id: &Option<String>, label: &str) -> TestSupportResult<String> {
    id.clone()
        .ok_or_else(|| io::Error::other(format!("accepted catalog id for `{label}`")).into())
}

/// The bound store catalog id of a committed place, ready to address store cells.
pub fn store_id_of(place: &CheckedSavedPlace) -> TestSupportResult<CatalogId> {
    CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")?)
        .map_err(|error| io::Error::other(format!("store catalog id: {error}")).into())
}

/// The bound stable catalog id of a top-level scalar field member named `name`.
pub fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> TestSupportResult<String> {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .ok_or_else(|| io::Error::other(format!("checked member `{name}`")))?;
    accepted_catalog_id(&member.catalog_id, name)
}

/// The bound stable catalog id of an index named `name` on the place.
pub fn index_catalog_id(place: &CheckedSavedPlace, name: &str) -> TestSupportResult<String> {
    let index = place
        .indexes
        .iter()
        .find(|index| index.name == name)
        .ok_or_else(|| io::Error::other(format!("checked index `{name}`")))?;
    accepted_catalog_id(&index.catalog_id, name)
}

/// A top-level group member named `group`, borrowed for its sub-members.
fn group_member<'a>(
    place: &'a CheckedSavedPlace,
    group: &str,
) -> TestSupportResult<&'a CheckedSavedMember> {
    place
        .root_members
        .iter()
        .find(|member| member.name == group && matches!(member.kind, CheckedSavedMemberKind::Group))
        .ok_or_else(|| io::Error::other(format!("checked group member `{group}`")).into())
}

/// The bound stable catalog id of a top-level group member named `group`.
pub fn group_member_catalog_id(
    place: &CheckedSavedPlace,
    group: &str,
) -> TestSupportResult<String> {
    accepted_catalog_id(&group_member(place, group)?.catalog_id, group)
}

/// The catalog id of a top-level keyed-leaf member: a `Field` that
/// carries key params, so it is the leaf its entries' values are stored under.
pub fn keyed_leaf_catalog_id(place: &CheckedSavedPlace, map: &str) -> TestSupportResult<String> {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == map
                && !member.key_params.is_empty()
                && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .ok_or_else(|| io::Error::other(format!("checked keyed-leaf member `{map}`")))?;
    accepted_catalog_id(&member.catalog_id, map)
}

/// The bound stable catalog id of a leaf named `leaf` one level inside `group`.
pub fn nested_member_catalog_id(
    place: &CheckedSavedPlace,
    group: &str,
    leaf: &str,
) -> TestSupportResult<String> {
    let member = group_member(place, group)?
        .group_members
        .iter()
        .find(|member| member.name == leaf)
        .ok_or_else(|| io::Error::other(format!("checked nested member `{group}.{leaf}`")))?;
    accepted_catalog_id(&member.catalog_id, leaf)
}

/// The catalog id of a member reached by an arbitrary name chain from the record root, each
/// segment a layer or group whose sub-members hold the next. Resolves members nested through
/// more than one keyed layer, which the single-level [`nested_member_catalog_id`] cannot reach.
pub fn deep_member_catalog_id(
    place: &CheckedSavedPlace,
    chain: &[&str],
) -> TestSupportResult<String> {
    let mut members = &place.root_members;
    let mut found = None;
    let path = chain.join(".");
    for segment in chain {
        let member = members
            .iter()
            .find(|member| member.name == *segment)
            .ok_or_else(|| io::Error::other(format!("checked nested member `{path}`")))?;
        found = Some(member);
        members = &member.group_members;
    }
    let member = found.ok_or_else(|| io::Error::other("empty member chain"))?;
    accepted_catalog_id(&member.catalog_id, &path)
}

/// The proposal-minted stable id of a brand-new resource member at the given module-qualified
/// catalog path. A member current source adds but the accepted catalog does not yet carry has
/// no bound facts id, so its identity lives only in the catalog proposal; the proposal-aware
/// presence scan keys its verdict by this id.
pub fn new_member_proposal_id(program: &CheckedProgram, path: &str) -> TestSupportResult<String> {
    Ok(program
        .catalog
        .proposal
        .as_ref()
        .ok_or_else(|| io::Error::other(format!("catalog proposal for `{path}`")))?
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::ResourceMember && entry.path == path)
        .ok_or_else(|| io::Error::other(format!("proposal entry for `{path}`")))?
        .stable_id
        .clone())
}

/// The proposal-minted stable id of the catalog entry at `path`, for any entry kind.
pub fn proposal_catalog_id(program: &CheckedProgram, path: &str) -> TestSupportResult<String> {
    Ok(program
        .catalog
        .proposal
        .as_ref()
        .ok_or_else(|| io::Error::other(format!("catalog proposal for `{path}`")))?
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| io::Error::other(format!("proposal catalog entry `{path}`")))?
        .stable_id
        .clone())
}

/// The stable catalog id the checked program bound to the enum named `name`, so a
/// hand-built accepted catalog records the identity-aware leaf token (`enum:<id>`) the
/// discharge compares against, not a source spelling.
pub fn enum_catalog_id(program: &CheckedProgram, name: &str) -> TestSupportResult<String> {
    let enum_fact = program
        .facts
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == name)
        .ok_or_else(|| io::Error::other(format!("checked enum `{name}`")))?;
    accepted_catalog_id(&enum_fact.catalog_id, name)
}

/// The stable catalog id of the enum member `enum_name::member`, so a test can seed a
/// stored enum value (its enum id plus the selected member id) the way the runtime
/// write path does.
pub fn enum_member_catalog_id(
    program: &CheckedProgram,
    enum_name: &str,
    member: &str,
) -> TestSupportResult<String> {
    let enum_id = program
        .facts
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == enum_name)
        .ok_or_else(|| io::Error::other(format!("checked enum `{enum_name}`")))?
        .id;
    let member_fact = program
        .facts
        .enum_members()
        .iter()
        .find(|member_fact| member_fact.enum_id == enum_id && member_fact.name == member)
        .ok_or_else(|| io::Error::other(format!("checked enum member `{enum_name}::{member}`")))?;
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
