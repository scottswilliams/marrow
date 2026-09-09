//! Ordinary source refusals continue every independent semantic phase whose typed
//! prerequisites exist.
//!
//! A refused signature, a duplicate declaration, or a refused body makes exactly the
//! artifacts that depend on it unavailable; every phase whose own prerequisites are
//! still available runs and reports. No image entry, index, export, test slot, or
//! dependent fact is fabricated from a missing prerequisite.

use std::sync::Arc;

use marrow_compile::{
    CompileFailure, InputRevision, ResourceLimitKind, SourceDiagnostic, analyze, check, compile,
    compile_with_tests,
};
use marrow_syntax::SourceSpan;
#[path = "common/project.rs"]
mod common_project;
use common_project::{project, project_with_ids};

/// The diagnostics `compile_with_tests` reports over a single module.
fn diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    diagnostics_over(&[("src/main.mw", source)])
}

fn diagnostics_over(files: &[(&str, &str)]) -> Vec<SourceDiagnostic> {
    reported(compile_with_tests(&project(files)))
}

fn reported(result: Result<impl std::fmt::Debug, CompileFailure>) -> Vec<SourceDiagnostic> {
    match result {
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

/// Each projection sees the same source, and every expected diagnostic names its
/// entire source construct. Run all three before comparing their results.
fn assert_diagnostic_sites(
    files: &[(&str, &str)],
    ids: Option<&[u8]>,
    expected: &[(&str, &str, &str)],
) {
    let input = Arc::new(project_with_ids(files, ids));
    let compiled = reported(compile_with_tests(&input));
    let checked = reported(check(&input));
    let snapshot = analyze(input, InputRevision::new(7))
        .unwrap_or_else(|_| panic!("fixture must produce a diagnostic snapshot"));
    let expected: Vec<_> = expected
        .iter()
        .map(|&(code, path, construct)| {
            let source = files
                .iter()
                .find(|(file, _)| *file == path)
                .expect("diagnostic source exists")
                .1;
            assert_eq!(source.matches(construct).count(), 1, "unique construct");
            let start = source.find(construct).expect("diagnostic construct exists");
            let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
            (
                code,
                path,
                SourceSpan {
                    start_byte: start,
                    end_byte: start + construct.len(),
                    line: source[..start]
                        .bytes()
                        .filter(|&byte| byte == b'\n')
                        .count() as u32
                        + 1,
                    column: (start - line_start + 1) as u32,
                },
            )
        })
        .collect();
    let projections = [
        ("compile_with_tests", compiled),
        ("check", checked),
        ("analyze", snapshot.diagnostics().to_vec()),
    ];
    let actual: Vec<_> = projections
        .iter()
        .map(|(projection, rows)| {
            let sites: Vec<_> = rows
                .iter()
                .map(|row| (row.code(), row.file().as_str(), row.span()))
                .collect();
            (*projection, sites)
        })
        .collect();
    let expected: Vec<_> = projections
        .iter()
        .map(|(projection, _)| (*projection, expected.clone()))
        .collect();
    assert_eq!(actual, expected);
}

/// Red 7. A refused function signature refuses that declaration alone.
///
/// The signature table is always built, so an unrelated body still lowers and
/// reports its own error, and constant evaluation and the value-cycle audit run in
/// their existing positions. A refused signature is a refused ledger entry rather
/// than a withheld table, so `driver`'s own unresolved call is reported beside the
/// three declaration refusals.
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
        vec![
            "check.unsupported",
            "check.unsupported",
            "check.type",
            "check.recursion",
        ],
        "the signature refusal, the constant refusal, the unrelated body's own \
         unresolved call, and the value cycle all report, in semantic order: {rows:#?}",
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

/// A body queues a generic instance before its later unresolved call refuses the body.
/// Draining completes the instance while the caller's reserved slot remains vacant.
const REFUSED_BODY_WITH_QUEUED_INSTANCE: &str = r#"module main

pub fn caller(): int {
    const queued = identity(1)
    return missing()
}

fn identity<T>(x: T): T {
    return x
}
"#;

/// A refused body withholds `CompleteDeclaredFunctionBodies` even when its queued
/// instance completes. Both production entries report the body diagnostic without
/// building an image or reaching an invariant.
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

/// Duplicate test titles leave one reserved test slot vacant and withhold
/// `CompleteDeclaredTestBodies`. The queued instance still drains; the outcome remains
/// the title conflict alone, with no image or invariant.
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
    let built = compile(&project(&[("src/main.mw", with_broken_test)])).unwrap_or_else(|failure| {
        panic!("a broken test body cannot refuse a production compile: {failure:?}")
    });
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

/// Completing a queued instance does not make the refused caller available. The
/// semantic fence returns its diagnostics before image policy or encoding.
#[test]
fn a_refused_declared_body_with_a_completed_instance_cannot_build_an_image() {
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
///
/// The two halves are separate on purpose. The 257 public functions lower cleanly and
/// mint 257 exports, which latches a real `MAX_EXPORTS` excess into the draft — a
/// program whose bodies are all refused mints nothing and would cross no image ceiling
/// at all, so the fixture would not be testing the fence. The single avalanche body
/// then overflows the diagnostic collector.
#[test]
fn a_limited_terminal_reports_its_own_bound_over_an_image_ceiling() {
    let mut source = String::from("module main\n\n");
    for index in 0..257 {
        source.push_str(&format!("pub fn f{index}(): int {{\n    return 0\n}}\n\n"));
    }
    source.push_str("pub fn avalanche(): int {\n");
    for index in 0..4200 {
        source.push_str(&format!("    const c{index} = missing()\n"));
    }
    source.push_str("    return 0\n}\n");
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

/// The settled-body byte ceiling is the one image-policy verdict taken inside the
/// semantic pass, and it is reported through the no-snapshot resource-limit arm: a body
/// refused before the stop is not carried with it. Thirty-two wide bodies whose first
/// carries a type error stop at the twenty-first retained body and report the byte
/// ceiling; the same program with sixteen wide bodies reports the type error.
#[test]
fn the_settled_body_byte_ceiling_stops_before_a_settled_refusal_is_reported() {
    fn wide(bodies: usize) -> String {
        let mut source =
            String::from("module main\n\npub fn refused(): int {\n    return \"x\"\n}\n\n");
        for index in 0..bodies {
            source.push_str(&format!("pub fn f{index}(): int {{\n    var total = 0\n"));
            for _ in 0..512 {
                source.push_str("    total += 1\n");
            }
            source.push_str("    return total\n}\n\n");
        }
        source
    }
    match compile(&project(&[("src/main.mw", &wide(32))])) {
        Err(CompileFailure::ResourceLimit(limit)) => {
            assert_eq!(limit.kind(), ResourceLimitKind::ImageBytes);
        }
        other => panic!("expected the byte ceiling, got {other:?}"),
    }
    assert_eq!(
        codes(&diagnostics(&wide(16))),
        vec!["check.type"],
        "under the ceiling the settled refusal is the outcome"
    );
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

/// The refused generic instance and the mutual cycle belong to independent call
/// components. The missing body cannot suppress either cycle member.
const DRAIN_REFUSED_MID_QUEUE: &str = r#"module main

pub fn driver(): int {
    return identity(1)
}

fn identity<T>(x: T): int {
    return missing()
}

fn ping(): int {
    return pong()
}

fn pong(): int {
    return ping()
}
"#;

#[test]
fn a_refused_instance_body_preserves_an_independent_cycle() {
    assert_diagnostic_sites(
        &[("src/main.mw", DRAIN_REFUSED_MID_QUEUE)],
        None,
        &[
            ("check.type", "src/main.mw", "missing()"),
            ("check.type", "src/main.mw", "missing()"),
            ("check.recursion", "src/main.mw", "fn ping(): int {"),
            ("check.recursion", "src/main.mw", "fn pong(): int {"),
        ],
    );
    for outcome in [
        outcome_of(compile(&project(&[(
            "src/main.mw",
            DRAIN_REFUSED_MID_QUEUE,
        )]))),
        outcome_of(compile_with_tests(&project(&[(
            "src/main.mw",
            DRAIN_REFUSED_MID_QUEUE,
        )]))),
    ] {
        assert_eq!(
            outcome,
            Outcome::Diagnostics,
            "a reserved-but-unminted instance index never reaches the encoder",
        );
    }
}

/// A refused declaration before another module's cycle leaves a hole in the
/// function domain. Both cycle members must retain their actual identities.
const REFUSED_BODY_BESIDE_AN_INDEPENDENT_CYCLE: &[(&str, &str)] = &[
    (
        "src/main.mw",
        r#"module main

pub fn driver(): int {
    return missingCall()
}
"#,
    ),
    (
        "src/other.mw",
        r#"module other

fn cycA(): int {
    return cycB()
}

fn cycB(): int {
    return cycA()
}
"#,
    ),
];

#[test]
fn a_refused_declared_body_preserves_an_independent_cycle() {
    assert_diagnostic_sites(
        REFUSED_BODY_BESIDE_AN_INDEPENDENT_CYCLE,
        None,
        &[
            ("check.type", "src/main.mw", "missingCall()"),
            ("check.recursion", "src/other.mw", "fn cycA(): int {"),
            ("check.recursion", "src/other.mw", "fn cycB(): int {"),
        ],
    );
    for outcome in [
        outcome_of(compile(&project(REFUSED_BODY_BESIDE_AN_INDEPENDENT_CYCLE))),
        outcome_of(compile_with_tests(&project(
            REFUSED_BODY_BESIDE_AN_INDEPENDENT_CYCLE,
        ))),
    ] {
        assert_eq!(
            outcome,
            Outcome::Diagnostics,
            "a reserved-but-unminted declaration index never reaches the encoder",
        );
    }
}

const GENERIC_CYCLE: &str = "module main\n\nfn spin<T>(x: T): T { return spin(x) }\npub fn driver(): int { return spin(1) }\n";
const REFUSED_BODY: &str = "\nfn broken(): int { return true }\n";
const EMPTY_TRANSACTION: &str = "\npub fn independent() { transaction {} }\n";

#[test]
fn a_complete_generic_cycle_reports_its_source() {
    assert_diagnostic_sites(
        &[("src/main.mw", GENERIC_CYCLE)],
        None,
        &[(
            "check.recursion",
            "src/main.mw",
            "fn spin<T>(x: T): T { return spin(x) }",
        )],
    );
}

#[test]
fn generic_recursion_survives_an_unrelated_body_refusal() {
    let source = format!("{GENERIC_CYCLE}{REFUSED_BODY}");
    assert_diagnostic_sites(
        &[("src/main.mw", &source)],
        None,
        &[
            ("check.type", "src/main.mw", "true"),
            (
                "check.recursion",
                "src/main.mw",
                "fn spin<T>(x: T): T { return spin(x) }",
            ),
        ],
    );
}

/// The first instance discovers another instance before its ordinary refusal.
/// Both the already-pending instance and the newly discovered one must be drained.
#[test]
fn a_refused_generic_instance_does_not_discard_pending_work() {
    let source = r#"module main

pub fn driver(): int {
    const first = bad(1)
    return spin(1)
}

fn bad<T>(x: T): int {
    const queued = after(x)
    return missing()
}

fn after<T>(x: T): T { return after(x) }
fn spin<T>(x: T): T { return spin(x) }
"#;
    assert_diagnostic_sites(
        &[("src/main.mw", source)],
        None,
        &[
            ("check.type", "src/main.mw", "missing()"),
            ("check.type", "src/main.mw", "missing()"),
            (
                "check.recursion",
                "src/main.mw",
                "fn spin<T>(x: T): T { return spin(x) }",
            ),
            (
                "check.recursion",
                "src/main.mw",
                "fn after<T>(x: T): T { return after(x) }",
            ),
        ],
    );
}

#[test]
fn an_empty_transaction_reports_its_block() {
    assert_diagnostic_sites(
        &[("src/main.mw", EMPTY_TRANSACTION)],
        None,
        &[("check.transaction_empty", "src/main.mw", "{}")],
    );
}

#[test]
fn a_refused_body_does_not_suppress_an_independent_empty_transaction() {
    let source = format!("{REFUSED_BODY}{EMPTY_TRANSACTION}");
    assert_diagnostic_sites(
        &[("src/main.mw", &source)],
        None,
        &[
            ("check.type", "src/main.mw", "true"),
            ("check.transaction_empty", "src/main.mw", "{}"),
        ],
    );
}

const COUNTER_IDS: &[u8] = b"marrow ids v0\n\
    machine-written by marrow; do not edit\n\
    id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
    id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
    id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
    id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
    id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
    id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
    high-water 0\n\
    end\n";

const UNAVAILABLE_DURABLE_CALLEE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

fn readBroken(): int? {
    const value = ^counters[1].value
    return missing()
}

pub fn dependent(): int? {
    transaction { return readBroken() }
}
"#;

#[test]
fn a_complete_durable_callee_keeps_its_transaction_nonempty() {
    let source = UNAVAILABLE_DURABLE_CALLEE.replace("return missing()", "return value");
    assert_diagnostic_sites(&[("src/main.mw", &source)], Some(COUNTER_IDS), &[]);
}

#[test]
fn an_unavailable_durable_callee_does_not_prove_an_empty_transaction() {
    assert_diagnostic_sites(
        &[("src/main.mw", UNAVAILABLE_DURABLE_CALLEE)],
        Some(COUNTER_IDS),
        &[("check.type", "src/main.mw", "missing()")],
    );
}

/// The unavailable callee cannot justify an empty-transaction diagnostic at its
/// caller. The independent empty block still has a complete effect closure.
#[test]
fn an_unavailable_durable_callee_preserves_an_independent_empty_transaction() {
    let source = format!("{UNAVAILABLE_DURABLE_CALLEE}{EMPTY_TRANSACTION}");
    assert_diagnostic_sites(
        &[("src/main.mw", &source)],
        Some(COUNTER_IDS),
        &[
            ("check.type", "src/main.mw", "missing()"),
            ("check.transaction_empty", "src/main.mw", "{}"),
        ],
    );
}

const CYCLIC_CALLEE: &str = "fn ping(): int { return pong() }\nfn pong(): int { return ping() }\npub fn dependent() { transaction { ping() } }\n";

#[test]
fn a_cycle_does_not_suppress_an_independent_empty_transaction() {
    let source = format!("{CYCLIC_CALLEE}{EMPTY_TRANSACTION}");
    assert_diagnostic_sites(
        &[("src/main.mw", &source)],
        None,
        &[
            (
                "check.recursion",
                "src/main.mw",
                "fn ping(): int { return pong() }",
            ),
            (
                "check.recursion",
                "src/main.mw",
                "fn pong(): int { return ping() }",
            ),
            ("check.transaction_empty", "src/main.mw", "{}"),
        ],
    );
}

#[test]
fn a_cycle_and_refused_body_preserve_an_independent_empty_transaction() {
    let source = format!("{REFUSED_BODY}{CYCLIC_CALLEE}{EMPTY_TRANSACTION}");
    assert_diagnostic_sites(
        &[("src/main.mw", &source)],
        None,
        &[
            ("check.type", "src/main.mw", "true"),
            (
                "check.recursion",
                "src/main.mw",
                "fn ping(): int { return pong() }",
            ),
            (
                "check.recursion",
                "src/main.mw",
                "fn pong(): int { return ping() }",
            ),
            ("check.transaction_empty", "src/main.mw", "{}"),
        ],
    );
}
