//! The editor analysis snapshot agrees with the production compile over the full
//! stage-stop corpus, and echoes the caller revision.
//!
//! For a single-module project the snapshot and `compile_with_tests` see the same
//! diagnostics, so their sets are byte-identical; the corpus exercises each reachable
//! stage stop (parse, a structural bound, a type-instantiation limit, and an ordinary
//! semantic error), the resource-limit arm (a driven aggregate image bound), and the
//! clean arm. For a multi-module project with one parse-failed component the production
//! compile projects only the parse stage while the snapshot additionally retains the
//! independent valid component's diagnostics — the compile set is a prefix of the
//! snapshot set (shared-prefix identity), never a divergence.

use std::sync::Arc;

use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, CompileFailure, InputRevision, ResourceLimitKind,
    SourceDiagnostic, analyze, compile_with_tests,
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

/// For a single-module project the snapshot's complete diagnostic set equals the
/// production compile's diagnostics exactly, and a resource-limit fixture surfaces the
/// same aggregate bound through both. The snapshot echoes the caller revision.
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
        }
        CompileView::ResourceLimit(kind) => {
            let failure = analyze(Arc::new(project(files)), revision)
                .err()
                .unwrap_or_else(|| panic!("a resource-limit project has no snapshot: {files:?}"));
            let AnalysisFailure::ResourceLimit {
                revision: echoed,
                limit: AnalysisResourceLimit::Compile(limit),
            } = failure
            else {
                panic!("expected a compile-side resource limit for {files:?}");
            };
            assert_eq!(
                limit.kind(),
                kind,
                "same aggregate bound through both paths"
            );
            assert_eq!(echoed, revision, "the failure echoes the revision");
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
    // MAX_FUNCTIONS is 4096; declaring more exhausts the aggregate function bound with
    // no single construct at fault, so both paths surface a locationless resource limit.
    let mut source = String::new();
    for index in 0..4097 {
        source.push_str(&format!(
            "pub fn f{index}(): int {{\n    return {index}\n}}\n"
        ));
    }
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
}
