//! Ordinary source refusals continue every independent semantic phase whose typed
//! prerequisites exist.
//!
//! A refused signature, a duplicate declaration, or a refused body makes exactly the
//! artifacts that depend on it unavailable; every phase whose own prerequisites are
//! still available runs and reports. No image entry, index, export, test slot, or
//! dependent fact is fabricated from a missing prerequisite.

use marrow_compile::{
    CompileFailure, ResourceLimitKind, SourceDiagnostic, compile, compile_with_tests,
};
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

/// One program's outcome from a production entry, named so a test can say which arm it
/// requires without matching an opaque payload.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Built,
    Diagnostics,
    ResourceLimit,
    Invariant,
}

fn outcome_of(result: Result<impl std::fmt::Debug, CompileFailure>) -> Outcome {
    match result {
        Ok(_) => Outcome::Built,
        Err(CompileFailure::Diagnostics(_)) => Outcome::Diagnostics,
        Err(CompileFailure::ResourceLimit(_)) => Outcome::ResourceLimit,
        Err(CompileFailure::Invariant(_)) => Outcome::Invariant,
    }
}

/// A refused body that already queued a generic instance. The call is emitted before the
/// refusal, so an instance index is reserved that the drain will never mint.
const REFUSED_BODY_WITH_QUEUED_INSTANCE: &str = r#"module main

pub fn caller(): int {
    const queued = identity(1)
    return missing()
}

fn identity<T>(x: T): T {
    return x
}
"#;

/// Red 9. A refused body withholds `CompleteDeclaredFunctionBodies`, which gates the
/// instance drain off, so the queued instance keeps a reserved index no body ever minted.
/// That is an ordinary diagnostic outcome from both production entries — never a built
/// image, never an invariant, and never a panic. The gate is carried by the artifacts, so
/// the result does not depend on the build profile.
#[test]
fn a_refused_body_with_a_queued_instance_reports_diagnostics() {
    assert_eq!(
        outcome_of(compile(&project(&[(
            "src/main.mw",
            REFUSED_BODY_WITH_QUEUED_INSTANCE
        )]))),
        Outcome::Diagnostics,
    );
    assert_eq!(
        outcome_of(compile_with_tests(&project(&[(
            "src/main.mw",
            REFUSED_BODY_WITH_QUEUED_INSTANCE
        )]))),
        Outcome::Diagnostics,
    );
    let rows = diagnostics(REFUSED_BODY_WITH_QUEUED_INSTANCE);
    let found = codes(&rows);
    assert_eq!(
        found,
        vec!["check.type"],
        "the refused body reports its own unresolved call and nothing else: {rows:#?}",
    );
}

/// Red 10. Duplicate test titles skip a body whose reserved index stays unminted, which
/// withholds `CompleteDeclaredTestBodies` and gates the drain off while a generic
/// instance is queued. The outcome is the title conflict alone: no image, no invariant,
/// and no fabricated instance ordinal.
#[test]
fn duplicate_test_titles_with_a_queued_instance_report_the_conflict_alone() {
    let source = r#"module main

pub fn driver(): int {
    return 0
}

fn identity<T>(x: T): T {
    return x
}

test "same" {
    const queued = identity(1)
    assert driver() == 0
}

test "same" {
    assert driver() == 0
}
"#;
    let rows = diagnostics(source);
    assert_eq!(
        codes(&rows),
        vec!["check.name_conflict"],
        "the duplicate title is the only report: {rows:#?}",
    );
    assert_eq!(
        outcome_of(compile_with_tests(&project(&[("src/main.mw", source)]))),
        Outcome::Diagnostics,
    );
}

/// Red 11. A production compile excludes test bodies, so `CompleteDeclaredTestBodies`
/// holds vacuously: a project whose only refusal is inside a test still builds its
/// production image, and no test-body diagnostic appears in that compile. The same
/// project reports through `compile_with_tests`.
#[test]
fn excluded_test_bodies_leave_the_production_image_unchanged() {
    let with_broken_test = r#"module main

pub fn driver(): int {
    return 0
}

test "broken" {
    assert missing() == 0
}
"#;
    let production_only = r#"module main

pub fn driver(): int {
    return 0
}
"#;
    let built = compile(&project(&[("src/main.mw", with_broken_test)]))
        .unwrap_or_else(|failure| panic!("a broken test body cannot refuse a production compile: {failure:?}"));
    let baseline = compile(&project(&[("src/main.mw", production_only)]))
        .unwrap_or_else(|failure| panic!("the baseline compiles: {failure:?}"));
    assert_eq!(
        built.image.image_id, baseline.image.image_id,
        "excluding test bodies leaves the production image byte-identical",
    );
    assert_eq!(
        outcome_of(compile_with_tests(&project(&[(
            "src/main.mw",
            with_broken_test
        )]))),
        Outcome::Diagnostics,
        "the same project reports when test bodies are included",
    );
}

/// Red 12. A call site that named a reserved-but-never-lowered instance index cannot
/// reach `encode`: the artifacts are incomplete, so the projection returns diagnostics.
/// The proof is the fixture's outcome, not the image owner's cross-reference validation —
/// which is defense in depth and must never be what stops this program.
#[test]
fn a_reserved_but_unlowered_instance_never_reaches_the_encoder() {
    let outcome = outcome_of(compile(&project(&[(
        "src/main.mw",
        REFUSED_BODY_WITH_QUEUED_INSTANCE,
    )])));
    assert_eq!(
        outcome,
        Outcome::Diagnostics,
        "an incomplete artifact set stops before the projection, so no image-policy \
         verdict and no invariant can be reported for this program",
    );
}

/// Red 13. A diagnostic avalanche over a program that also crosses an image ceiling
/// reports its own diagnostic bound: the semantic terminal is `Limited`, which is a
/// diagnostic state, and the fence takes that strictly before the projection's verdict.
/// No image-policy kind may surface here.
#[test]
fn a_limited_terminal_reports_its_own_bound_over_an_image_ceiling() {
    let mut source = String::from("module main\n\n");
    for index in 0..4097 {
        source.push_str(&format!("fn f{index}(): int {{\n    return missing()\n}}\n\n"));
    }
    match compile(&project(&[("src/main.mw", &source)])) {
        Err(CompileFailure::ResourceLimit(limit)) => assert!(
            matches!(
                limit.kind(),
                ResourceLimitKind::DiagnosticCount | ResourceLimitKind::DiagnosticBytes
            ),
            "a diagnostic overflow reports its own bound, not an image bound: {:?}",
            limit.kind(),
        ),
        other => panic!("expected the diagnostic bound, got {other:?}"),
    }
}

/// Red 13. Every refusal in this suite is a reported one: an artifact never becomes
/// unavailable without a diagnostic to explain it, so no source program in the
/// continuation corpus reaches the `UnavailableWithoutReport` invariant.
#[test]
fn no_continuation_fixture_reaches_an_invariant() {
    let corpus = [
        REFUSED_BODY_WITH_QUEUED_INSTANCE,
        "module main\n\nfn takesUnknown(x: NoSuchType): int {\n    return 0\n}\n",
        "module main\n\nfn ping(): int {\n    return pong()\n}\n\nfn pong(): int {\n    return ping()\n}\n",
        "module main\n\nfn twice(): int {\n    return 0\n}\n\nfn twice(): int {\n    return 1\n}\n",
    ];
    for source in corpus {
        for outcome in [
            outcome_of(compile(&project(&[("src/main.mw", source)]))),
            outcome_of(compile_with_tests(&project(&[("src/main.mw", source)]))),
        ] {
            assert_eq!(
                outcome,
                Outcome::Diagnostics,
                "every refusal reports; none becomes a bare invariant: {source}",
            );
        }
    }
}
