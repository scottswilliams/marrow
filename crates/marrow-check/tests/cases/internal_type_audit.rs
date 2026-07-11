use std::path::PathBuf;

use marrow_check::tooling::{
    SourceHoverFact, SourceSavedRootCursorKind, source_hover_fact_at,
    source_saved_root_cursor_fact_at,
};
use marrow_check::{
    AnalysisSnapshot, DiagnosticPayload, InternalTypeIssueKind, Severity, build_binding_index,
};
use marrow_codes::{Catchability, Code, Family, Lifecycle, SeverityClass};

use crate::support;

fn analyze(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
    support::analyze_overlay_compiler_dev(name, files)
}

fn expected_span(source: &str, needle: &str) -> marrow_syntax::SourceSpan {
    let start_byte = source.rfind(needle).expect("needle is present");
    span_at(source, start_byte, needle.len())
}

fn span_at(source: &str, start_byte: usize, len: usize) -> marrow_syntax::SourceSpan {
    let prefix = &source[..start_byte];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, tail)| tail.len()) as u32
        + 1;
    marrow_syntax::SourceSpan {
        start_byte,
        end_byte: start_byte + len,
        line,
        column,
    }
}

fn audit_diagnostics(snapshot: &AnalysisSnapshot) -> Vec<&marrow_check::CheckDiagnostic> {
    snapshot
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::CompilerDevUnknownType.as_str())
        .collect()
}

#[test]
fn clean_keyed_root_loop_is_not_a_value_type_probe() {
    let source = "\
module books

resource Book
    title: string

store ^books(id: int): Book

pub fn printIds()
    for id in ^books
        print(id)
    const count = count(^books)
    const id = nextId(^books)
";
    let (snapshot, paths) = analyze(
        "internal-type-audit-keyed-root",
        &[("src/books.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let root_offset = source.find("^books\n").expect("loop root") + 1;
    let root = source_saved_root_cursor_fact_at(&snapshot, &paths[0], root_offset)
        .expect("saved-root cursor fact");
    assert_eq!(root.kind, SourceSavedRootCursorKind::Expression);

    assert!(
        audit_diagnostics(&snapshot).is_empty(),
        "{:#?}",
        audit_diagnostics(&snapshot),
    );
}

#[test]
fn clean_local_keys_result_reports_origin_and_propagated_unknown_positions() {
    assert_eq!(Code::CompilerDevUnknownType.family(), Family::Compiler);
    assert_eq!(
        Code::CompilerDevUnknownType.severity_class(),
        SeverityClass::Warning
    );
    assert_eq!(
        Code::CompilerDevUnknownType.catchability(),
        Catchability::NotApplicable
    );
    assert_eq!(
        Code::CompilerDevUnknownType.lifecycle(),
        Lifecycle::Internal
    );
    let source = "module m\n\nfn f(xs: sequence[int])\n    const values = keys(xs)\n    const copied = values\n";
    let (snapshot, paths) = analyze("internal-type-audit-local-keys", &[("src/m.mw", source)]);
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].span, expected_span(source, ")"));
    assert_eq!(diagnostics[1].span, expected_span(source, "values"));
    for diagnostic in diagnostics {
        assert_eq!(diagnostic.code, Code::CompilerDevUnknownType.as_str());
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.file, paths[0]);
        assert_eq!(
            diagnostic.payload,
            DiagnosticPayload::InternalTypeIssue(InternalTypeIssueKind::RecoveryUnknown),
        );
    }

    let (ordinary, _) = support::analyze_overlay(
        "internal-type-audit-local-keys-ordinary",
        &[("src/m.mw", source)],
    );
    assert!(audit_diagnostics(&ordinary).is_empty());
}

#[test]
fn discarded_recovery_call_reports_its_closing_edge() {
    let source = "module m\n\nfn f(xs: sequence[int])\n    keys(xs)\n";
    let (snapshot, paths) = analyze(
        "internal-type-audit-discarded-recovery-call",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].file, paths[0]);
    assert_eq!(diagnostics[0].span, expected_span(source, ")"));
}

#[test]
fn nested_recovery_call_is_observed_inside_a_known_outer_call() {
    let source = "module m\n\nfn f(xs: sequence[int])\n    print(keys(xs))\n";
    let (snapshot, paths) = analyze(
        "internal-type-audit-nested-recovery-call",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].file, paths[0]);
    let inner_close = source.find("))").expect("nested call closes");
    assert_eq!(diagnostics[0].span, span_at(source, inner_close, 1));
}

#[test]
fn condition_inference_observes_nested_recovery() {
    let source =
        "module m\n\nfn f(xs: sequence[int])\n    if bool(keys(xs))\n        print(\"ok\")\n";
    let (snapshot, paths) = analyze(
        "internal-type-audit-condition-recovery",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].file, paths[0]);
    let inner_close = source.find("))").expect("nested call closes");
    assert_eq!(diagnostics[0].span, span_at(source, inner_close, 1));
}

#[test]
fn local_collection_consumers_observe_nested_recovery() {
    let source = "module m\n\nfn f(xs: sequence[int])\n    const size = count(keys(xs))\n    const values = keys(xs)\n    for value in values\n        print(value)\n";
    let (snapshot, paths) = analyze(
        "internal-type-audit-collection-consumer-recovery",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 4, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file == paths[0])
    );
    let mut expected = source
        .match_indices("keys(xs)")
        .map(|(start, call)| span_at(source, start + call.len() - 1, 1))
        .collect::<Vec<_>>();
    expected.push(expected_span(source, "values"));
    expected.push(expected_span(source, "value"));
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.span)
            .collect::<Vec<_>>(),
        expected,
    );
}

#[test]
fn match_scrutinee_and_arm_bodies_share_the_recovery_trace() {
    let source = "\
module m

enum Status
    active

fn choose(value: unknown): Status
    return Status::active

fn f(xs: sequence[int])
    match choose(keys(xs))
        active
            const copied = keys(xs)
";
    let (snapshot, paths) = analyze(
        "internal-type-audit-match-recovery",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file == paths[0])
    );
    let closes = source
        .match_indices("keys(xs)")
        .map(|(start, call)| span_at(source, start + call.len() - 1, 1))
        .collect::<Vec<_>>();
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.span)
            .collect::<Vec<_>>(),
        closes,
    );
}

#[test]
fn adjacent_operator_does_not_mask_a_recovery_call_close() {
    let source = "module m\n\nfn f(xs: sequence[int])\n    keys(xs)+1\n";
    let (snapshot, paths) = analyze(
        "internal-type-audit-adjacent-operator",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].file, paths[0]);
    assert_eq!(diagnostics[0].span, expected_span(source, ")"));
}

#[test]
fn address_keys_and_range_steps_share_the_recovery_trace() {
    let source = "\
module m

fn f(xs: sequence[int])
    var values(k: int): string
    values(int(keys(xs))) = \"ok\"
    delete values(int(keys(xs)))
    for value in 1..10 by keys(xs)
        print(value)
";
    let (snapshot, paths) = analyze(
        "internal-type-audit-address-and-step-recovery",
        &[("src/m.mw", source)],
    );
    support::assert_clean(&snapshot.report);

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file == paths[0])
    );
    let expected = source
        .match_indices("keys(xs)")
        .map(|(start, call)| span_at(source, start + call.len() - 1, 1))
        .collect::<Vec<_>>();
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.span)
            .collect::<Vec<_>>(),
        expected,
    );
}

#[test]
fn audit_excludes_explicit_dynamic_no_value_and_richer_hover_owners() {
    let source = "\
module a

resource Book
    notes(noteId: string)
        text: string

store ^books(id: int): Book

fn log()
    print(\"logged\")

pub fn inspect(xs: sequence[int], value: unknown, id: int)
    print(value)
    print(\"done\")
    log()
    print(1 + 1)
    for noteId in ^books(id).notes
        print(noteId)
";
    let (snapshot, _) = analyze("internal-type-audit-exclusions", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert!(
        audit_diagnostics(&snapshot).is_empty(),
        "{:#?}",
        audit_diagnostics(&snapshot),
    );
}

#[test]
fn audit_is_suppressed_for_a_snapshot_with_user_errors() {
    let source = "\
module a

pub fn broken(): int
    return missing
";
    let (snapshot, _) = analyze("internal-type-audit-broken-source", &[("src/a.mw", source)]);
    assert!(snapshot.report.has_errors(), "{:#?}", snapshot.report);

    assert!(audit_diagnostics(&snapshot).is_empty());
}

#[test]
fn audit_includes_configured_test_files_before_restoring_the_source_program() {
    use marrow_check::{ProjectSources, analyze_project_with_compiler_dev_audit};
    use marrow_project::parse_config;

    let root = support::temp_project("internal-type-audit-configured-tests", |root| {
        support::write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"ok\")\n",
        );
        support::write(
            root,
            "tests/keys_test.mw",
            "resource Scratch\n    value: int\n\nstore ^scratch(id: int): Scratch\n\nfn copyKeys(xs: sequence[int])\n    const values = keys(xs)\n    const copied = values\n",
        );
    });
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let (source_report, source_program) = marrow_check::check_project(&root, &config)
        .expect("check source project before configured tests");
    support::assert_clean(&source_report);
    let snapshot =
        analyze_project_with_compiler_dev_audit(&root, &config, &ProjectSources::new(), None, None)
            .expect("analyze project");
    support::assert_clean(&snapshot.report);
    assert_eq!(
        snapshot.program, source_program,
        "restoring the source program must remove every test-only arena and index entry",
    );
    assert!(
        snapshot
            .files
            .iter()
            .any(|file| file.path.ends_with("tests/keys_test.mw")),
        "configured tests remain in the analysis snapshot",
    );

    let diagnostics = audit_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file.ends_with("tests/keys_test.mw")),
        "{diagnostics:#?}"
    );
    assert!(
        !snapshot
            .program
            .modules
            .iter()
            .any(|module| module.source_file == diagnostics[0].file),
        "the returned program remains source-only",
    );
}

#[test]
fn configured_test_errors_suppress_the_compiler_audit() {
    use marrow_check::{ProjectSources, analyze_project_with_compiler_dev_audit};
    use marrow_project::parse_config;

    let root = support::temp_project("internal-type-audit-configured-test-error", |root| {
        support::write(
            root,
            "src/app.mw",
            "module app\n\nfn f(xs: sequence[int])\n    const values = keys(xs)\n    const copied = values\n",
        );
        support::write(root, "tests/broken_test.mw", "const value = missing\n");
    });
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let snapshot =
        analyze_project_with_compiler_dev_audit(&root, &config, &ProjectSources::new(), None, None)
            .expect("analyze project");
    assert!(snapshot.report.has_errors(), "{:#?}", snapshot.report);
    assert!(audit_diagnostics(&snapshot).is_empty());
}

#[test]
fn audit_sorts_files_and_deduplicates_token_spans_deterministically() {
    let a_source = "\
module a

fn f(xs: sequence[int])
    const alpha = keys(xs)
    const copied = alpha
";
    let z_source = "\
module z

fn f(xs: sequence[int])
    const zeta = keys(xs)
    const copied = zeta
";
    let (snapshot, paths) = analyze(
        "internal-type-audit-order",
        &[("src/z.mw", z_source), ("src/a.mw", a_source)],
    );
    support::assert_clean(&snapshot.report);

    let issues = audit_diagnostics(&snapshot);
    assert_eq!(
        issues
            .iter()
            .map(|diagnostic| (&diagnostic.file, diagnostic.span))
            .collect::<Vec<_>>(),
        [
            (&paths[1], expected_span(a_source, ")")),
            (&paths[1], expected_span(a_source, "alpha")),
            (&paths[0], expected_span(z_source, ")")),
            (&paths[0], expected_span(z_source, "zeta")),
        ]
    );
}

#[test]
fn canonical_source_hover_fact_dispatch_covers_every_owner_before_type_fallback() {
    let source = "\
module m

resource Book
    required title: int

store ^books(id: int): Book

fn double(value: int): int
    return value + 1

pub fn inspect(id: int)
    double(id)
    print(id)
";
    let (snapshot, paths) = analyze("source-hover-fact-dispatch", &[("src/m.mw", source)]);
    support::assert_clean(&snapshot.report);
    let path = &paths[0];
    let index = build_binding_index(&snapshot);
    let fact_at = |offset| {
        source_hover_fact_at(&snapshot, &index, path, offset)
            .unwrap_or_else(|| panic!("missing hover fact at byte {offset}"))
    };

    assert!(matches!(
        fact_at(source.find("m\n").unwrap()),
        SourceHoverFact::ModulePath(_)
    ));
    assert!(matches!(
        fact_at(source.find("Book").unwrap()),
        SourceHoverFact::Schema(_)
    ));
    assert!(matches!(
        fact_at(source.find("books(id").unwrap()),
        SourceHoverFact::StoreRoot(_)
    ));
    assert!(matches!(
        fact_at(source.rfind("double(id)").unwrap()),
        SourceHoverFact::Callable(_)
    ));
    assert!(matches!(
        fact_at(source.rfind("title").unwrap()),
        SourceHoverFact::SavedPlace(_)
    ));
    assert!(matches!(
        fact_at(source.find("+").unwrap()),
        SourceHoverFact::Operator(_)
    ));
    assert!(matches!(
        fact_at(source.find("1\n").unwrap()),
        SourceHoverFact::Type(_)
    ));
}
