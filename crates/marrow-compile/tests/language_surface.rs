//! Written forms whose meaning is fixed by lowering: bracket lookup and assignment,
//! the integer-bound value builtins, and the `require ... else` guard.
//!
//! Each form is exercised through the production `compile` path and asserted by typed
//! diagnostic code, span, or encoded image bytes.

use marrow_compile::{CompileFailure, Compiled, SourceDiagnostic, compile};
use marrow_project::ProjectInput;

use marrow_test_programs::project as project_capture;

#[path = "language_surface/bracket_lookup.rs"]
mod bracket_lookup;
#[path = "language_surface/int_bounds.rs"]
mod int_bounds;
#[path = "language_surface/require_guard.rs"]
mod require_guard;

fn project(source: &str) -> ProjectInput {
    project_capture::project(&[("src/main.mw", source)])
}

fn compile_ok(source: &str) -> Compiled {
    compile(&project(source)).unwrap_or_else(|failure| {
        panic!("expected a clean compile, got {failure:#?}");
    })
}

/// The diagnostics of a refused source. A source-triggered failure stays a diagnostic
/// set: neither an aggregate resource limit nor a producer invariant is reachable from
/// the shapes these fixtures write.
fn compile_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the program compiled"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("source-triggered failures must remain diagnostics, got {other:#?}"),
    }
}

fn wrap(body: &str) -> String {
    format!("module main\n\n{body}\n")
}
