//! The record build over a member ledger whose index has drifted from its occurrences.
//!
//! A record's fields are read out of the member ledger, so a read that fails must fail
//! the build: sealing a record, or a group record, from a read that silently came back
//! empty would publish a type with no fields for a resource that declares some.

use super::*;

use marrow_image::ImageDraft;
use marrow_syntax::{Declaration, parse_source};

/// The one resource `source` declares.
fn the_resource(source: &str) -> ResourceDecl {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "the fixture parses");
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .expect("the fixture declares a resource")
}

/// The registry built from `resource`, its member ledger then misaddressed.
fn misaddressed_registry(resource: &ResourceDecl, draft: &mut DraftTxn<'_>) -> TypeRegistry {
    let resources = [(
        FileRef::admitted(0),
        crate::test_file("src/main.mw").clone(),
        resource,
    )];
    let mut diagnostics = DiagnosticCollector::new();
    let mut registry = TypeRegistry::build(
        draft,
        crate::source::CapturedOrigins::of(crate::test_input()),
        &[],
        &[],
        &[],
        &[],
        &resources,
        &mut diagnostics,
        DeclarationBudget::default(),
    )
    .expect("the fixture registry stays within the ledger budget");
    assert!(
        diagnostics.finish().is_empty(),
        "the fixture is well-formed"
    );
    registry.members.misaddress_every_occurrence();
    registry
}

fn is_drift(outcome: Result<impl Sized, BuildError>) -> bool {
    matches!(
        outcome,
        Err(BuildError::Invariant(
            GenericInvariant::DeclarationIndexDrift
        ))
    )
}

#[test]
fn a_record_sealed_over_a_drifted_member_ledger_reports_the_drift() {
    let resource = the_resource("resource R {\n    a: int\n}\n");
    let mut owner = ImageDraft::new();
    let mut draft = crate::compile::admitted(&mut owner);
    let mut registry = misaddressed_registry(&resource, &mut draft);
    let record = registry.records[0].scoped_name();
    assert!(is_drift(seal_record_slots(
        &mut draft,
        &mut registry,
        0,
        &record,
        Vec::new(),
        Vec::new(),
    )));
}

#[test]
fn a_group_built_over_a_drifted_member_ledger_reports_the_drift() {
    let resource = the_resource("resource R {\n    g {\n        y: int\n    }\n}\n");
    let Some(ResourceMember::Group(group)) = resource.members.first() else {
        panic!("the fixture's first member is a group")
    };
    let mut owner = ImageDraft::new();
    let mut draft = crate::compile::admitted(&mut owner);
    let mut registry = misaddressed_registry(&resource, &mut draft);
    let record = registry.records[0].scoped_name();
    let declared = DeclarationSite {
        name: &resource.name,
        file: crate::test_file("src/main.mw"),
        at: FileRef::admitted(0),
        span: resource.span,
    };
    assert!(is_drift(build_group_leaves(
        &mut draft,
        &mut registry,
        &record,
        group,
        declared,
        &mut DiagnosticCollector::new(),
    )));
}
