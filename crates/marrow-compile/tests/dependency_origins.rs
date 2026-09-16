//! Compiling a project that reuses a local source dependency.
//!
//! Every case captures two trees through the one production `capture_origins` call
//! and drives the production `check`, so what is asserted is what `marrow check`
//! would report over the same two directories.

use marrow_compile::{CompileFailure, SourceDiagnostic, check};
use marrow_project::ProjectInput;

#[path = "common/project.rs"]
mod project_capture;

/// The diagnostics a check refused with. A source-triggered refusal stays a
/// diagnostic failure; any other arm is a compiler-coherence fault.
fn diagnostics(project: &ProjectInput) -> Vec<SourceDiagnostic> {
    match check(project) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:#?}"),
    }
}

const TEXT_LIBRARY: &str = r#"module text

struct Pair {
    key: string
    value: string
}

pub fn parsePair(line: string): Pair {
    return Pair(key: line, value: line)
}
"#;

/// The consuming project writes `use graphtext::text` and calls `text::parsePair`;
/// the library keeps its own unprefixed `module text` header.
#[test]
fn a_dependency_module_is_imported_under_its_alias() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::text

pub fn run(line: string): string {
  const pair = text::parsePair(line)
  return pair.key
}
"#,
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| (row.code().as_str(), row.message().to_string()))
            .collect::<Vec<_>>(),
        Vec::new(),
    );
}
