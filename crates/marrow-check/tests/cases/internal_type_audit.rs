use std::path::{Path, PathBuf};

use marrow_check::tooling::{
    SourceHoverFact, SourceSavedRootCursorKind, source_hover_fact_at,
    source_saved_root_cursor_fact_at,
};
use marrow_check::{
    AnalysisSnapshot, InternalTypeIssue, InternalTypeIssueKind, Severity, build_binding_index,
    internal_type_issue_diagnostics, internal_type_issues,
};
use marrow_codes::{Catchability, Code, Family, Lifecycle, SeverityClass};

use crate::support;

fn analyze(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
    support::analyze_overlay(name, files)
}

fn issue_at(file: &Path, source: &str, needle: &str) -> InternalTypeIssue {
    let start_byte = source.rfind(needle).expect("needle is present");
    let prefix = &source[..start_byte];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, tail)| tail.len()) as u32
        + 1;
    InternalTypeIssue {
        file: file.to_path_buf(),
        span: marrow_syntax::SourceSpan {
            start_byte,
            end_byte: start_byte + needle.len(),
            line,
            column,
        },
        kind: InternalTypeIssueKind::RecoveryUnknown,
    }
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

    assert_eq!(internal_type_issues(&snapshot), []);
}

#[test]
fn clean_local_keys_result_reports_the_exact_unknown_type_position() {
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

    let issues = internal_type_issues(&snapshot);
    assert_eq!(issues, [issue_at(&paths[0], source, "values")]);

    let diagnostics = internal_type_issue_diagnostics(&snapshot);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, Code::CompilerDevUnknownType.as_str());
    assert_eq!(diagnostics[0].severity, Severity::Warning);
    assert_eq!(diagnostics[0].file, paths[0]);
    assert_eq!(diagnostics[0].span, issues[0].span);
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
    print(keys(xs) + 1)
    for noteId in ^books(id).notes
        print(noteId)
";
    let (snapshot, _) = analyze("internal-type-audit-exclusions", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(internal_type_issues(&snapshot), []);
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

    assert_eq!(internal_type_issues(&snapshot), []);
    assert_eq!(internal_type_issue_diagnostics(&snapshot), []);
}

#[test]
fn audit_skips_configured_test_files_absent_from_the_restored_source_program() {
    use marrow_check::{ProjectSources, analyze_project};
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
            "fn copyKeys(xs: sequence[int])\n    const values = keys(xs)\n    const copied = values\n",
        );
    });
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let snapshot = analyze_project(&root, &config, &ProjectSources::new(), None, None)
        .expect("analyze project");
    support::assert_clean(&snapshot.report);
    assert!(
        snapshot
            .files
            .iter()
            .any(|file| file.path.ends_with("tests/keys_test.mw")),
        "configured tests remain in the analysis snapshot",
    );

    assert_eq!(internal_type_issues(&snapshot), []);
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

    let issues = internal_type_issues(&snapshot);
    assert_eq!(
        issues,
        [
            issue_at(&paths[1], a_source, "alpha"),
            issue_at(&paths[0], z_source, "zeta"),
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
