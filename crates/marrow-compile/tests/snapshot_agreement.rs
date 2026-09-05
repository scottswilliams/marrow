//! The editor analysis snapshot and `check` agree with the production compile over the
//! full stage-stop corpus, and the snapshot echoes the caller revision.
//!
//! For a single-module project the snapshot, `check`, and `compile_with_tests` see the
//! same diagnostics, so their sets are byte-identical; the corpus exercises each
//! reachable stage stop (parse, a structural bound, a type-instantiation limit, and an
//! ordinary semantic error), the resource-limit arm (a driven aggregate image bound), and
//! the clean arm. For a multi-module project with one parse-failed component the
//! production compile projects only the parse stage while the snapshot and `check`
//! retain the independent valid component's diagnostics — the compile set is a prefix of
//! the union (shared-prefix identity), never a divergence. `check` reports exactly the
//! snapshot's union: it is the same projection over the same drive.

use std::sync::Arc;

use marrow_compile::{
    CompileFailure, InputRevision, ResourceLimitKind, SourceDiagnostic, analyze, check,
    compile_with_tests,
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

/// The diagnostics `compile_with_tests` reports (empty for a built image), or the
/// resource-limit kind it stopped on.
enum CompileView {
    Diagnostics(Vec<SourceDiagnostic>),
    ResourceLimit(ResourceLimitKind),
}

fn compile_view(input: &ProjectInput) -> CompileView {
    match compile_with_tests(input) {
        Ok(_) => CompileView::Diagnostics(Vec::new()),
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            CompileView::Diagnostics(diagnostics.as_slice().to_vec())
        }
        Err(CompileFailure::ResourceLimit(limit)) => CompileView::ResourceLimit(limit.kind()),
        Err(CompileFailure::Invariant(_)) => panic!("no fixture triggers a compiler invariant"),
    }
}

/// The diagnostics `check` reports (empty for an encoded image), or the resource-limit
/// kind it stopped on.
fn check_view(input: &ProjectInput) -> CompileView {
    match check(input) {
        Ok(_) => CompileView::Diagnostics(Vec::new()),
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            CompileView::Diagnostics(diagnostics.as_slice().to_vec())
        }
        Err(CompileFailure::ResourceLimit(limit)) => CompileView::ResourceLimit(limit.kind()),
        Err(CompileFailure::Invariant(_)) => panic!("no fixture triggers a compiler invariant"),
    }
}

/// `check` reports exactly the snapshot's complete diagnostic set, in order.
fn assert_check_reports_the_snapshot_union(files: &[(&str, &str)], snapshot: &[SourceDiagnostic]) {
    let CompileView::Diagnostics(checked) = check_view(&project(files)) else {
        panic!("a diagnostic project is refused by check with its diagnostics: {files:?}");
    };
    assert_eq!(
        checked.as_slice(),
        snapshot,
        "check diverged from the snapshot union for {files:?}",
    );
}

/// For a single-module project the snapshot's complete diagnostic set equals the
/// production compile's diagnostics exactly, `check` reports that same set, and a
/// resource-limit fixture surfaces the same aggregate bound through compile and check
/// alike. The snapshot echoes the caller revision.
fn assert_single_module_agreement(files: &[(&str, &str)]) {
    let input = project(files);
    let revision = InputRevision::new(7);
    match compile_view(&input) {
        CompileView::Diagnostics(expected) => {
            let snapshot = analyze(Arc::new(project(files)), revision)
                .unwrap_or_else(|_| panic!("a diagnostic project yields a snapshot: {files:?}"));
            assert_eq!(
                snapshot.diagnostics(),
                expected.as_slice(),
                "snapshot diverged from compile for {files:?}",
            );
            assert_eq!(
                snapshot.revision(),
                revision,
                "the snapshot echoes the revision"
            );
            assert_check_reports_the_snapshot_union(files, snapshot.diagnostics());
        }
        // An image-policy bound is the production projection's verdict, not semantic
        // unavailability: the compile refuses with its kind while the analysis path —
        // which never encodes — yields an ordinary snapshot with no diagnostic at all.
        // `check` encodes the same test-inclusive image, so it refuses with the kind.
        CompileView::ResourceLimit(kind) => {
            let snapshot = analyze(Arc::new(project(files)), revision).unwrap_or_else(|_| {
                panic!("an image-policy bound still yields a snapshot: {files:?}")
            });
            assert_eq!(
                snapshot.diagnostics(),
                &[],
                "an image-policy bound produces no diagnostic: {files:?}",
            );
            assert_eq!(
                snapshot.revision(),
                revision,
                "the snapshot echoes the revision"
            );
            let CompileView::ResourceLimit(checked) = check_view(&project(files)) else {
                panic!("check encodes the same image and refuses the same bound: {files:?}");
            };
            assert_eq!(checked, kind);
        }
    }
}

#[test]
fn clean_project_yields_an_empty_snapshot() {
    assert_single_module_agreement(&[("src/main.mw", "pub fn f(): int {\n    return 1\n}\n")]);
}

#[test]
fn a_parse_stop_agrees() {
    assert_single_module_agreement(&[("src/main.mw", "pub fn f(: int {\n    return 1\n}\n")]);
}

#[test]
fn a_structural_bound_stop_agrees() {
    // MAX_PARAMS is 16; a 17-parameter function is refused at its declaration.
    let params: String = (0..17)
        .map(|index| format!("p{index}: int"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("pub fn f({params}): int {{\n    return 1\n}}\n");
    assert_single_module_agreement(&[("src/main.mw", &source)]);
}

#[test]
fn a_type_instantiation_limit_stop_agrees() {
    assert_single_module_agreement(&[(
        "src/main.mw",
        "struct Grow<T> {\n    next: Grow<List<T>>\n}\n\n\
         pub fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n\n\
         pub fn f(): int {\n    const ignored = deepen(1)\n    return 0\n}\n",
    )]);
}

#[test]
fn a_semantic_stop_agrees() {
    assert_single_module_agreement(&[(
        "src/main.mw",
        "pub fn f(): int {\n    return missing()\n}\n",
    )]);
}

#[test]
fn a_driven_resource_limit_agrees() {
    // Each body returns a distinct literal, so this project crosses MAX_CONSTS (1024)
    // before any other aggregate bound — the measure-core invariant pass consults the constant table
    // first. The fixture states the kind that actually fires rather than the function
    // bound it once claimed.
    let mut source = String::new();
    for index in 0..4097 {
        source.push_str(&format!(
            "pub fn f{index}(): int {{\n    return {index}\n}}\n"
        ));
    }
    let input = project(&[("src/main.mw", &source)]);
    let CompileView::ResourceLimit(kind) = compile_view(&input) else {
        panic!("an over-bound project refuses with a resource limit");
    };
    assert_eq!(
        kind,
        ResourceLimitKind::Consts,
        "the constant table is the first aggregate bound this project crosses",
    );
    assert_single_module_agreement(&[("src/main.mw", &source)]);
}

#[test]
fn the_compile_diagnostics_are_a_prefix_of_the_resilient_snapshot() {
    let files = &[
        (
            "src/broken.mw",
            "module broken\n\npub fn g(: int {\n    return 1\n}\n",
        ),
        (
            "src/valid.mw",
            "module valid\n\npub fn h(): int {\n    return missing()\n}\n",
        ),
    ];
    let input = project(files);
    let CompileView::Diagnostics(compile_diagnostics) = compile_view(&input) else {
        panic!("the broken module yields a parse diagnostic set");
    };
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("a resilient snapshot is produced past the sibling parse error");
    };

    // Shared-prefix identity: every diagnostic the production compile reports appears,
    // in order and identically, at the front of the resilient snapshot.
    assert!(
        snapshot.diagnostics().starts_with(&compile_diagnostics),
        "compile diagnostics must be a prefix of the snapshot:\ncompile: {compile_diagnostics:#?}\nsnapshot: {:#?}",
        snapshot.diagnostics(),
    );
    // The snapshot retains strictly more: the independent valid module's own diagnostic,
    // which the production compile's parse-stage projection dropped.
    assert!(
        snapshot.diagnostics().len() > compile_diagnostics.len(),
        "the snapshot must retain the valid module's diagnostics past the sibling parse error",
    );
    assert!(
        snapshot
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.file().as_str() == "src/valid.mw"),
    );
    assert_check_reports_the_snapshot_union(files, snapshot.diagnostics());
}

/// Errors in every stage across three modules — a parse error, a semantic error in an
/// ordinary body, and a semantic error in a test body — reach `check` as the one ordered
/// union the snapshot holds, with every file and stage represented; the production
/// compile projects the parse stage alone and never reaches the test body.
#[test]
fn check_reports_parse_semantic_and_test_errors_across_modules_together() {
    let files = &[
        (
            "src/broken.mw",
            "module broken\n\npub fn g(: int {\n    return 1\n}\n",
        ),
        (
            "src/body.mw",
            "module body\n\npub fn h(): int {\n    return missing()\n}\n",
        ),
        (
            "src/tested.mw",
            "module tested\n\npub fn k(): int {\n    return 1\n}\n\n\
             test \"k is one\" {\n    assert k() == \"one\"\n}\n",
        ),
    ];
    let snapshot = analyze(Arc::new(project(files)), InputRevision::new(2))
        .unwrap_or_else(|_| panic!("a resilient snapshot is produced"));
    let files_reported: Vec<&str> = snapshot
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.file().as_str())
        .collect();
    for file in ["src/broken.mw", "src/body.mw", "src/tested.mw"] {
        assert!(
            files_reported.contains(&file),
            "{file} is missing: {files_reported:?}"
        );
    }
    assert_check_reports_the_snapshot_union(files, snapshot.diagnostics());

    let CompileView::Diagnostics(compiled) = compile_view(&project(files)) else {
        panic!("the broken module yields a parse diagnostic set");
    };
    assert!(
        compiled
            .iter()
            .all(|diagnostic| diagnostic.file().as_str() == "src/broken.mw"),
        "the production compile projects the parse stage alone: {compiled:?}",
    );
}

/// Two tests with one title are a declaration refusal at the second title; `check`
/// reports it as the snapshot does and refuses the image, and so does the production
/// test compile.
#[test]
fn a_duplicate_test_title_is_refused_by_check_and_the_snapshot_alike() {
    let files = &[(
        "src/main.mw",
        "pub fn f(): int {\n    return 1\n}\n\n\
         test \"same\" {\n    assert f() == 1\n}\n\n\
         test \"same\" {\n    assert f() == 1\n}\n",
    )];
    let snapshot = analyze(Arc::new(project(files)), InputRevision::new(3))
        .unwrap_or_else(|_| panic!("a duplicate title yields a snapshot"));
    assert_eq!(
        snapshot.diagnostics().len(),
        1,
        "{:?}",
        snapshot.diagnostics()
    );
    assert_eq!(snapshot.diagnostics()[0].line(), 9);
    assert_single_module_agreement(files);
}

/// The union of stages may cross the diagnostic count ceiling no single stage crossed:
/// 2,100 modules that fail to parse beside one module of 2,100 semantic errors. The
/// production compile projects the parse stage alone and reports its rows complete;
/// the snapshot and `check` union the semantic rows in, cross the ceiling, and refuse
/// with the diagnostic count bound — never a truncated set.
#[test]
fn a_cross_stage_union_overflow_refuses_check_and_the_snapshot_alike() {
    let per_stage = 2_100usize;
    let mut sources: Vec<(String, String)> = (0..per_stage)
        .map(|index| {
            (
                format!("src/broken{index}.mw"),
                format!("module broken{index}\n\npub fn g(: int {{\n    return 1\n}}\n"),
            )
        })
        .collect();
    let mut body = String::from("module body\n\n");
    for index in 0..per_stage {
        body.push_str(&format!(
            "pub fn h{index}(): int {{\n    return \"{index}\"\n}}\n\n"
        ));
    }
    sources.push(("src/body.mw".to_string(), body));
    let files: Vec<(&str, &str)> = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();

    let CompileView::Diagnostics(parse_rows) = compile_view(&project(&files)) else {
        panic!("the production compile projects the complete parse stage");
    };
    assert_eq!(
        parse_rows.len(),
        per_stage,
        "one parse diagnostic per broken module"
    );
    match analyze(Arc::new(project(&files)), InputRevision::new(4)) {
        Err(marrow_compile::AnalysisFailure::ResourceLimit {
            limit: marrow_compile::AnalysisResourceLimit::Compile(limit),
            ..
        }) => assert_eq!(limit.kind(), ResourceLimitKind::DiagnosticCount),
        Err(_) => panic!("the union overflow is the diagnostic count bound"),
        Ok(snapshot) => panic!(
            "no snapshot is minted past the ceiling ({} rows)",
            snapshot.diagnostics().len()
        ),
    }
    let CompileView::ResourceLimit(kind) = check_view(&project(&files)) else {
        panic!("check refuses the overflowing union as a resource limit");
    };
    assert_eq!(kind, ResourceLimitKind::DiagnosticCount);
}
