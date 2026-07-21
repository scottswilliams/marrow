//! Root-admission steering (E07-M2A): a reference to a store root whose durable identity
//! failed admission is steered to the `check.durable_identity` reports, not reported as a
//! bare unknown name. The ledger confound: an identity-less root drops from the durable
//! registry, so a `^root` reference — even from another module — read as `not in scope`,
//! misdirecting toward a typo. A genuinely undeclared root keeps the plain not-in-scope
//! message.

use marrow_compile::{CompileFailure, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// Capture a multi-file project with no `marrow.ids` ledger, so every durable identity is
/// missing and any declared store fails admission.
fn project(files: &[(&str, &str)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn diagnostics(project: &ProjectInput) -> Vec<marrow_compile::SourceDiagnostic> {
    match compile(project) {
        Ok(compiled) => panic!("expected an admission failure, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

const STORE_MODULE: &str = "module main\n\n\
     resource Member {\n\
     \x20   required email: string\n\
     }\n\n\
     store ^members[id: int]: Member\n";

/// The two-module confound: `^members` is declared in `main` but its identity fails
/// admission (no ledger), so it drops from the registry. A reference from another module
/// names the admission failure and points at the identity reports — never a bare
/// not-in-scope error, which would misdirect toward a typo.
#[test]
fn a_reference_to_an_admission_failed_root_is_steered_to_the_identity_reports() {
    let reference = "module report\n\n\
         pub fn lookup(id: int): string? {\n\
         \x20   return ^members[id].email\n\
         }\n";
    let diagnostics = diagnostics(&project(&[
        ("src/main.mw", STORE_MODULE),
        ("src/report.mw", reference),
    ]));

    let steering = diagnostics
        .iter()
        .find(|d| d.file().as_str() == "src/report.mw" && d.code == "check.type")
        .unwrap_or_else(|| panic!("expected a reference-site diagnostic, got {diagnostics:#?}"));
    assert_eq!(
        steering.message,
        "`members` was declared but failed identity admission; see the \
         `check.durable_identity` reports",
        "the reference site names the admission failure, not a bare unknown name",
    );
    assert!(
        !steering.message.contains("is not in scope"),
        "an admission-failed root must not read as an unknown name: {}",
        steering.message,
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "check.durable_identity"),
        "the primary identity gaps are still reported: {diagnostics:#?}",
    );
}

/// A single-module reference reproduces the same steering: the confound was never
/// cross-module (roots are project-wide); it was an identity-less root dropping from the
/// registry and reading as an unknown name in its own module too.
#[test]
fn the_steering_holds_within_the_declaring_module() {
    let source = "module main\n\n\
         resource Member {\n\
         \x20   required email: string\n\
         }\n\n\
         store ^members[id: int]: Member\n\n\
         pub fn lookup(id: int): string? {\n\
         \x20   return ^members[id].email\n\
         }\n";
    let diagnostics = diagnostics(&project(&[("src/main.mw", source)]));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "check.type" && d.message.contains("failed identity admission")),
        "the declaring module's own reference is steered too: {diagnostics:#?}",
    );
}

/// A genuinely undeclared root keeps the plain not-in-scope message: the steering fires
/// only for a declared root that failed admission, never for a typo.
#[test]
fn a_genuinely_undeclared_root_keeps_the_unknown_name_message() {
    let reference = "module report\n\n\
         pub fn lookup(id: int): string? {\n\
         \x20   return ^ghosts[id].email\n\
         }\n";
    let diagnostics = diagnostics(&project(&[
        ("src/main.mw", STORE_MODULE),
        ("src/report.mw", reference),
    ]));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "check.type" && d.message == "`ghosts` is not in scope"),
        "an undeclared root is a plain unknown name: {diagnostics:#?}",
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| !d.message.contains("`ghosts` was declared")),
        "an undeclared root never claims to have been declared: {diagnostics:#?}",
    );
}
