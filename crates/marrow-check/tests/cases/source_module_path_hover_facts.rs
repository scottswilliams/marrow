use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{
    SourceModulePathHoverFact, SourceProjectModuleHoverFact, SourceStandardLibraryCapability,
    SourceStandardLibraryModuleHoverFact, SourceStandardLibraryNamespaceHoverFact,
    SourceStandardLibraryOperationHoverFact, source_module_path_hover_fact_at,
};
use marrow_check::{AnalysisSnapshot, BindingIndex, build_binding_index};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, BindingIndex, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths[0].clone())
}

fn analyze_files(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn analyze_files_with_diagnostics(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceModulePathHoverFact> {
    source_module_path_hover_fact_at(snapshot, index, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn std_module_fact(module: &str) -> SourceModulePathHoverFact {
    let mut operations = marrow_schema::stdlib::all()
        .iter()
        .filter(|op| op.module == module)
        .map(|op| SourceStandardLibraryOperationHoverFact {
            name: op.op.to_string(),
            required_capability: op.requires_capability.map(std_capability),
        })
        .collect::<Vec<_>>();
    operations.sort_by(|left, right| left.name.cmp(&right.name));
    SourceModulePathHoverFact::StandardLibraryModule(SourceStandardLibraryModuleHoverFact {
        module: module.to_string(),
        operations,
    })
}

fn std_capability(
    capability: marrow_schema::stdlib::Capability,
) -> SourceStandardLibraryCapability {
    match capability {
        marrow_schema::stdlib::Capability::Clock => SourceStandardLibraryCapability::Clock,
        marrow_schema::stdlib::Capability::Context => SourceStandardLibraryCapability::Context,
        marrow_schema::stdlib::Capability::Environment => {
            SourceStandardLibraryCapability::Environment
        }
        marrow_schema::stdlib::Capability::Log => SourceStandardLibraryCapability::Log,
        marrow_schema::stdlib::Capability::Filesystem => {
            SourceStandardLibraryCapability::Filesystem
        }
    }
}

fn project_fact(snapshot: &AnalysisSnapshot, module: &str) -> SourceModulePathHoverFact {
    let module = snapshot
        .program
        .facts
        .modules()
        .iter()
        .find(|fact| fact.name == module)
        .expect("module fact");
    SourceModulePathHoverFact::ProjectModule(SourceProjectModuleHoverFact {
        module: module.name.clone(),
        source_file: module.source_file.clone(),
        span: module.span,
    })
}

#[test]
fn source_module_path_hover_fact_covers_std_namespace_and_module_prefixes() {
    let source = "\
module a

pub fn f(): int
    return std::text::length(\"abc\")
";
    let (snapshot, index, file) = analyze("source-module-hover-std-prefix", source);

    let std_modules = {
        let mut modules = marrow_schema::stdlib::all()
            .iter()
            .map(|op| op.module.to_string())
            .collect::<Vec<_>>();
        modules.sort();
        modules.dedup();
        modules
    };
    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "std::text") + 1),
        Some(SourceModulePathHoverFact::StandardLibraryNamespace(
            SourceStandardLibraryNamespaceHoverFact {
                modules: std_modules
            }
        ))
    );

    let text = offset(source, "std::text") + "std::".len();
    assert_eq!(
        fact_at(&snapshot, &index, &file, text + 1),
        Some(std_module_fact("text"))
    );

    let length = offset(source, "length(\"abc");
    assert_eq!(fact_at(&snapshot, &index, &file, length + 1), None);
}

#[test]
fn source_module_path_hover_fact_expands_imported_std_module_segments() {
    let source = "\
module a

use std::text

pub fn f(): int
    return text::length(\"abc\")
";
    let (snapshot, index, file) = analyze("source-module-hover-imported-std", source);

    let imported = offset(source, "use std::text") + "use std::".len();
    assert_eq!(
        fact_at(&snapshot, &index, &file, imported + 1),
        Some(std_module_fact("text"))
    );

    let call = offset(source, "text::length");
    assert_eq!(
        fact_at(&snapshot, &index, &file, call + 1),
        Some(std_module_fact("text"))
    );
}

#[test]
fn source_module_path_hover_fact_covers_project_module_imports_and_call_prefixes() {
    let books = "\
module shelf::books

pub fn titleOf(): string
    return \"Dune\"
";
    let app = "\
module shelf::app

use shelf::books

pub fn run(): string
    return books::titleOf()
";
    let (snapshot, index, paths) = analyze_files(
        "source-module-hover-project-prefix",
        &[("src/shelf/books.mw", books), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];
    let expected = project_fact(&snapshot, "shelf::books");

    let imported = offset(app, "use shelf::books") + "use shelf::".len();
    assert_eq!(
        fact_at(&snapshot, &index, file, imported + 1),
        Some(expected.clone())
    );

    let call = offset(app, "books::titleOf");
    assert_eq!(fact_at(&snapshot, &index, file, call + 1), Some(expected));

    let leaf = call + "books::".len();
    assert_eq!(fact_at(&snapshot, &index, file, leaf + 1), None);
}

#[test]
fn source_module_path_hover_fact_uses_project_std_module_when_operation_is_not_builtin() {
    let std_text = "\
module std::text

pub fn custom(): int
    return 1
";
    let app = "\
module app

pub fn run(): int
    return std::text::custom()
";
    let (snapshot, index, paths) = analyze_files_with_diagnostics(
        "source-module-hover-project-std",
        &[("src/std/text.mw", std_text), ("src/app.mw", app)],
    );
    let file = &paths[1];
    let expected = project_fact(&snapshot, "std::text");

    let std = offset(app, "std::text::custom");
    assert_eq!(
        fact_at(&snapshot, &index, file, std + 1),
        Some(expected.clone())
    );

    let text = std + "std::".len();
    assert_eq!(fact_at(&snapshot, &index, file, text + 1), Some(expected));
}

#[test]
fn source_module_path_hover_fact_treats_std_module_declaration_as_project_module() {
    let source = "\
module std::text

pub fn custom(): int
    return 1
";
    let (snapshot, index, paths) = analyze_files(
        "source-module-hover-std-module-declaration",
        &[("src/std/text.mw", source)],
    );
    let file = &paths[0];
    let expected = project_fact(&snapshot, "std::text");
    let text = offset(source, "module std::text") + "module std::".len();

    assert_eq!(fact_at(&snapshot, &index, file, text + 1), Some(expected));
}

#[test]
fn source_module_path_hover_fact_keeps_builtin_precedence_over_project_std_module() {
    let std_text = "\
module std::text

pub fn length(value: string): bool
    return false
";
    let app = "\
module app

pub fn run(): int
    return std::text::length(\"abc\")
";
    let (snapshot, index, paths) = analyze_files(
        "source-module-hover-std-precedence",
        &[("src/std/text.mw", std_text), ("src/app.mw", app)],
    );
    let file = &paths[1];

    let text = offset(app, "std::text") + "std::".len();
    assert_eq!(
        fact_at(&snapshot, &index, file, text + 1),
        Some(std_module_fact("text"))
    );
}

#[test]
fn source_module_path_hover_fact_does_not_steal_schema_leaves() {
    let state = "\
module shelf::state

pub enum Status
    open
";
    let app = "\
module shelf::app

use shelf::state

pub fn run(value: state::Status): bool
    return value is state::Status::open
";
    let (snapshot, index, paths) = analyze_files(
        "source-module-hover-schema-leaves",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];
    let status = offset(app, "state::Status") + "state::".len();
    assert_eq!(fact_at(&snapshot, &index, file, status + 1), None);

    let open = offset(app, "Status::open") + "Status::".len();
    assert_eq!(fact_at(&snapshot, &index, file, open + 1), None);
}
