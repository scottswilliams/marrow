//! Ordinary source refusals continue every independent semantic phase whose typed
//! prerequisites exist.
//!
//! A refused signature, a duplicate declaration, or a refused body makes exactly the
//! artifacts that depend on it unavailable; every phase whose own prerequisites are
//! still available runs and reports. No image entry, index, export, test slot, or
//! dependent fact is fabricated from a missing prerequisite.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile_with_tests};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

fn project(files: &[(&str, &str)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// The diagnostics `compile_with_tests` reports over a single module.
fn diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    diagnostics_over(&[("src/main.mw", source)])
}

fn diagnostics_over(files: &[(&str, &str)]) -> Vec<SourceDiagnostic> {
    match compile_with_tests(&project(files)) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(rows)) => rows.as_slice().to_vec(),
        Err(CompileFailure::ResourceLimit(limit)) => {
            panic!("fixture reached a resource limit: {:?}", limit.kind())
        }
        Err(CompileFailure::Invariant(invariant)) => {
            panic!("fixture reached a compiler invariant: {invariant:?}")
        }
    }
}

fn codes(rows: &[SourceDiagnostic]) -> Vec<&str> {
    rows.iter().map(SourceDiagnostic::code).collect()
}

/// Red 7. A refused function signature makes `FunctionRegistry` unavailable, so no
/// body is lowered — but constant evaluation and the value-cycle audit depend only on
/// `CompleteTypeRegistry`, so both still run and report, in their existing positions.
/// The base stops at the signature stage and reports the signature row alone.
#[test]
fn signature_refusal_keeps_independent_checks_runnable() {
    let rows = diagnostics(
        r#"module main

struct Loop {
    next: Loop
}

const bad = missingConst()

fn takesUnknown(x: NoSuchType): int {
    return 0
}

pub fn driver(): int {
    return missingCall()
}
"#,
    );
    assert_eq!(
        codes(&rows),
        vec!["check.unsupported", "check.unsupported", "check.recursion"],
        "the signature refusal, the constant refusal, and the value cycle all \
         report, in semantic order: {rows:#?}",
    );
}

/// Red 8a. A duplicate function name is an ordinary source refusal that leaves every
/// artifact available: bodies still lower, so the independent call cycle is reported
/// beside it. The base gates `reject_recursion` on an empty diagnostic set and reports
/// the name conflict alone.
#[test]
fn a_duplicate_function_name_does_not_suppress_an_independent_call_cycle() {
    let rows = diagnostics(
        r#"module main

fn twice(): int {
    return 0
}

fn twice(): int {
    return 1
}

fn ping(): int {
    return pong()
}

fn pong(): int {
    return ping()
}

pub fn driver(): int {
    return ping()
}
"#,
    );
    let found = codes(&rows);
    assert!(
        found.contains(&"check.name_conflict"),
        "the duplicate function name is reported: {rows:#?}",
    );
    assert!(
        found.contains(&"check.recursion"),
        "the independent call cycle is reported beside it: {rows:#?}",
    );
}

/// Red 8b. A duplicate test title skips one test body, which is a declaration
/// refusal, not a lowering refusal: the indices actually minted stay dense, so the
/// call graph over the lowered set is exact and the independent cycle is still
/// reported. The base reports the title conflict alone.
#[test]
fn a_duplicate_test_title_does_not_suppress_an_independent_call_cycle() {
    let rows = diagnostics(
        r#"module main

fn ping(): int {
    return pong()
}

fn pong(): int {
    return ping()
}

pub fn driver(): int {
    return 0
}

test "same" {
    assert driver() == 0
}

test "same" {
    assert driver() == 0
}
"#,
    );
    let found = codes(&rows);
    assert!(
        found.contains(&"check.name_conflict"),
        "the duplicate test title is reported: {rows:#?}",
    );
    assert!(
        found.contains(&"check.recursion"),
        "the independent call cycle is reported beside it: {rows:#?}",
    );
}

/// Red 8c. A module-header path mismatch is reported before any registry is built and
/// makes no artifact unavailable, so the independent call cycle is reported beside it.
#[test]
fn a_module_path_diagnostic_does_not_suppress_an_independent_call_cycle() {
    let rows = diagnostics_over(&[(
        "src/main.mw",
        r#"module wrong

fn ping(): int {
    return pong()
}

fn pong(): int {
    return ping()
}

pub fn driver(): int {
    return ping()
}
"#,
    )]);
    let found = codes(&rows);
    assert!(
        found.contains(&"check.module_path"),
        "the module path mismatch is reported: {rows:#?}",
    );
    assert!(
        found.contains(&"check.recursion"),
        "the independent call cycle is reported beside it: {rows:#?}",
    );
}
