use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{
    SourceModulePathDefinitionFact, source_module_path_definition_fact_at,
};
use marrow_check::{AnalysisSnapshot, BindingIndex, build_binding_index};

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
) -> Option<SourceModulePathDefinitionFact> {
    source_module_path_definition_fact_at(snapshot, index, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn project_definition(snapshot: &AnalysisSnapshot, module: &str) -> SourceModulePathDefinitionFact {
    let module = snapshot
        .program
        .facts
        .modules()
        .iter()
        .find(|fact| fact.name == module)
        .expect("module fact");
    SourceModulePathDefinitionFact {
        module: module.name.clone(),
        source_file: module.source_file.clone(),
        span: module.span,
    }
}

#[test]
fn source_module_path_definition_fact_covers_project_call_prefixes() {
    let std_custom = "\
module std::custom

pub fn tick(): int
    return 1
";
    let foo = "\
module foo::foo

pub fn title(): string
    return \"nested\"
";
    let books = "\
module shelf::books

pub fn titleOf(): string
    return \"Dune\"
";
    let app = "\
module app

pub fn run(): int
    const _first = std::custom::tick()
    const _second = foo::foo::title()
    const _third = shelf::books::titleOf()
    return _first
";
    let (snapshot, index, paths) = analyze_files_with_diagnostics(
        "source-module-definition-project-prefixes",
        &[
            ("src/std/custom.mw", std_custom),
            ("src/foo/foo.mw", foo),
            ("src/shelf/books.mw", books),
            ("src/app.mw", app),
        ],
    );
    let file = &paths[3];

    let custom = offset(app, "std::custom::tick") + "std::".len();
    assert_eq!(
        fact_at(&snapshot, &index, file, custom + 1),
        Some(project_definition(&snapshot, "std::custom"))
    );

    let second_foo = offset(app, "foo::foo::title") + "foo::".len();
    assert_eq!(
        fact_at(&snapshot, &index, file, second_foo + 1),
        Some(project_definition(&snapshot, "foo::foo"))
    );

    let first_foo = offset(app, "foo::foo::title");
    assert_eq!(fact_at(&snapshot, &index, file, first_foo + 1), None);

    let books = offset(app, "shelf::books::titleOf") + "shelf::".len();
    assert_eq!(
        fact_at(&snapshot, &index, file, books + 1),
        Some(project_definition(&snapshot, "shelf::books"))
    );

    let shelf = offset(app, "shelf::books::titleOf");
    assert_eq!(fact_at(&snapshot, &index, file, shelf + 1), None);
}

#[test]
fn source_module_path_definition_fact_excludes_std_library_and_schema_leaves() {
    let state = "\
module shelf::state

pub enum Status
    open
";
    let app = "\
module shelf::app

use shelf::state

pub fn run(value: state::Status): bool
    const _now = std::clock::now()
    return value is state::Status::open
";
    let (snapshot, index, paths) = analyze_files(
        "source-module-definition-exclusions",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];

    let clock = offset(app, "std::clock::now") + "std::".len();
    assert_eq!(fact_at(&snapshot, &index, file, clock + 1), None);

    let status = offset(app, "state::Status") + "state::".len();
    assert_eq!(fact_at(&snapshot, &index, file, status + 1), None);

    let open = offset(app, "Status::open") + "Status::".len();
    assert_eq!(fact_at(&snapshot, &index, file, open + 1), None);
}

#[test]
fn source_module_path_definition_fact_returns_none_for_binding_index_import_alias() {
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
        "source-module-definition-binding-index-alias",
        &[("src/shelf/books.mw", books), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];
    let alias = offset(app, "books::titleOf");

    assert!(index.definition(file, alias + 1).is_some());
    assert_eq!(fact_at(&snapshot, &index, file, alias + 1), None);
}
