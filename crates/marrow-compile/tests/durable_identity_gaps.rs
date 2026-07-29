//! Project-wide durable-identity gap ownership.
//!
//! A project can declare two store roots that reference one resource. The compiler
//! reports each missing `(kind, path)` anchor once in deterministic first-resolution
//! order while keeping every affected root identity-incomplete. Later image
//! admission rejects the repeated semantic identities; this test covers gap
//! reporting only.

use std::collections::BTreeSet;

use marrow_codes::Code;
use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::{
    CaptureLimits, CapturedFile, IdentityAnchor, IdentityKind, Manifest, ProjectInput,
};

fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn diagnostics(project: &ProjectInput) -> Vec<SourceDiagnostic> {
    match compile(project) {
        Ok(compiled) => panic!("expected missing durable identities, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

#[test]
fn shared_resource_gaps_are_reported_once_while_both_roots_remain_incomplete() {
    let source = r#"module main

resource Shared {
    required value: int
}

store ^first[id: int]: Shared
store ^second[id: int]: Shared

pub fn readFirst(id: int): int? {
    return ^first[id].value
}

pub fn readSecond(id: int): int? {
    return ^second[id].value
}
"#;
    let diagnostics = diagnostics(&project(source));
    let gaps: Vec<IdentityAnchor> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.identity.as_ref())
        .map(|gap| {
            assert!(!gap.retired, "the fixture has no retired anchors");
            gap.anchor()
        })
        .collect();

    assert_eq!(
        gaps,
        vec![
            IdentityAnchor::new(IdentityKind::Application, "."),
            IdentityAnchor::new(IdentityKind::Root, "first"),
            IdentityAnchor::new(IdentityKind::Product, "Shared"),
            IdentityAnchor::new(IdentityKind::Key, "first.id"),
            IdentityAnchor::new(IdentityKind::Field, "Shared.value"),
            IdentityAnchor::new(IdentityKind::Root, "second"),
            IdentityAnchor::new(IdentityKind::Key, "second.id"),
        ],
        "the first project-wide resolution owns each shared gap",
    );
    assert_eq!(
        gaps.iter().cloned().collect::<BTreeSet<_>>().len(),
        gaps.len(),
        "a compiler-owned identity request is never duplicated",
    );

    for root in ["first", "second"] {
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == Code::CheckType.as_str()
                    && diagnostic.message
                        == format!(
                            "`{root}` was declared but failed identity admission; see the \
                             `check.durable_identity` reports"
                        )
            }),
            "root `^{root}` must remain identity-incomplete: {diagnostics:#?}",
        );
    }
}
