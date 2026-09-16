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

/// The typed code and rendered message of every diagnostic, in compiler order.
fn codes_and_messages(project: &ProjectInput) -> Vec<(&'static str, String)> {
    diagnostics(project)
        .iter()
        .map(|row| (row.code().as_str(), row.message().to_string()))
        .collect()
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
    assert_eq!(codes_and_messages(&project), Vec::new());
}

/// A `use` whose first segment is a declared dependency reports against that
/// dependency, spelling the missing path the way the dependency's own source does.
#[test]
fn an_absent_dependency_module_names_the_dependency() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", "module main\n\nuse graphtext::missing\n")],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.import",
            "no module `missing` in the dependency `graphtext`".to_string()
        )],
    );
}

/// An alias roots a dependency's modules and names no module of its own.
#[test]
fn a_bare_alias_is_not_a_module() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", "module main\n\nuse graphtext\n")],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.import",
            "`graphtext` is a declared dependency, not a module; name one of its \
             modules, as in `graphtext::<module>`"
                .to_string()
        )],
    );
}

/// A dependency file's `module` header is checked against the path its own tree
/// spells, so the library still checks standalone; a header that spells the
/// consumer's alias-rooted path is the mismatch.
#[test]
fn a_dependency_header_is_checked_unprefixed() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", "module main\n")],
        &[("src/text.mw", "module graphtext::text\n")],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.module_path",
            "module header `graphtext::text` does not match its path; expected \
             `module text`"
                .to_string()
        )],
    );
}

/// Each tree owns its type namespace: the two `Pair` declarations are two types,
/// and a consumer names the dependency's through the alias.
#[test]
fn a_qualified_type_name_resolves_through_the_alias() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::text

struct Pair {
    left: int
}

pub fn run(line: string): string {
    const parsed: graphtext::Pair = text::parsePair(line)
    const mine = Pair(left: 1)
    if mine.left == 0 {
        return ""
    }
    return parsed.key
}
"#,
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}

/// A bare name resolves in the tree that wrote it. The consumer declares its own
/// `Pair`, so the name binds that one and the dependency's fields are not its.
#[test]
fn a_bare_type_name_does_not_reach_across_origins() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

struct Pair {
    left: int
}

pub fn run(): int {
    const p = Pair(key: "a", value: "b")
    return p.left
}
"#,
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.type", "check.type"],
    );
}

/// A qualified name whose first segment names no declared dependency is outside the
/// admitted set, not a bare name carrying a `::`.
#[test]
fn an_unknown_qualifier_names_no_type() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\npub fn run(p: nowhere::Pair): int {\n    return 1\n}\n",
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.unsupported"],
    );
}

/// A private function of a dependency is not callable from the consuming project.
#[test]
fn a_dependency_private_function_is_not_callable() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\nuse graphtext::text\n\npub fn run(line: string): string {\n    return text::secret(line)\n}\n",
        )],
        &[(
            "src/text.mw",
            "module text\n\nfn secret(line: string): string {\n    return line\n}\n",
        )],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.visibility"],
    );
}
