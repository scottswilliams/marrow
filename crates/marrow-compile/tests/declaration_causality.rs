//! Declared-entry causality: a declaration the compiler refused keeps its name.
//!
//! A namespace that drops a refused declaration makes every later lookup read as
//! *never declared*, so the use site reports a fabricated absence — "is not in
//! scope" — for a name the reader can see declared, and reports it once per use.
//! Under the declaration ledger a refused key answers `Refused`: the declaring
//! cause is reported once at the declaration, the first use is steered to it, and
//! later uses fail silently.
//!
//! Diagnostics are asserted by code, span, and count. The one prose assertion is
//! negative — that a refused name is never called out of scope — which is the
//! fabrication these fixtures exist to kill.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(compiled) => panic!("expected a refused declaration, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

/// Every row, as `(code, line, column)` — the typed shape a red asserts.
fn rows(diagnostics: &[SourceDiagnostic]) -> Vec<(&str, u32, u32)> {
    diagnostics
        .iter()
        .map(|row| {
            let span = row.span();
            (row.code(), span.line, span.column)
        })
        .collect()
}

fn assert_never_out_of_scope(diagnostics: &[SourceDiagnostic], name: &str) {
    for row in diagnostics {
        assert!(
            !row.message().contains(&format!("`{name}` is not in scope")),
            "`{name}` is declared in this source; no row may call it out of scope: {:#?}",
            rows(diagnostics),
        );
    }
}

/// R1 — a constant refused for a type mismatch is reported at its declaration and
/// its use is steered to that report, never called out of scope.
#[test]
fn r1_a_type_refused_constant_is_not_out_of_scope_at_its_use() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit: int = \"x\"\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "limit");
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 3, 1), ("check.type", 6, 12)],
        "the declaration reports the cause and the use is steered to it",
    );
}

/// R2 — a constant refused for a non-literal value behaves the same, and the steer
/// reuses the declaring code (`check.unsupported`), not the use site's own.
#[test]
fn r2_a_value_refused_constant_steers_with_the_declaring_code() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 1 + 2\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "limit");
    assert_eq!(
        rows(&diagnostics),
        vec![("check.unsupported", 3, 15), ("check.unsupported", 6, 12)],
        "the steer carries the declaring cause's code, so a use-site assertion \
         names the declaration's typed identity",
    );
}

/// R24 — the report is once per refused key, not once per use. Two uses of one
/// refused constant produce the declaring row and exactly one steer.
#[test]
fn r24_a_refused_constant_is_reported_once_across_many_uses() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit: int = \"x\"\n\n\
         pub fn read(): int {\n\
         \x20   const a = limit\n\
         \x20   const b = limit\n\
         \x20   const c = limit\n\
         \x20   return a + b + c\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "limit");
    assert_eq!(
        diagnostics.len(),
        2,
        "one declaring row and one steer, whatever the use count: {:#?}",
        rows(&diagnostics),
    );
    assert_eq!(rows(&diagnostics)[0], ("check.type", 3, 1));
}

/// R25 — a refused declaration still occupies its name, in both orders. The
/// duplicate check sees the refused occurrence, so the second declaration is a
/// name conflict whether the refused one came first or second.
#[test]
fn r25_a_refused_constant_occupies_its_name_when_declared_first() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 1 + 2\n\
         const limit = 5\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert!(
        diagnostics
            .iter()
            .any(|row| row.code() == "check.name_conflict"),
        "a refused declaration occupies its name, so the redeclaration conflicts: {:#?}",
        rows(&diagnostics),
    );
}

/// The sibling direction, which already held: the refused occurrence comes second.
#[test]
fn r25_a_refused_constant_occupies_its_name_when_declared_second() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 5\n\
         const limit = 1 + 2\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![("check.name_conflict", 4, 1)],
        "the accepted first declaration answers the use; only the conflict reports",
    );
}
