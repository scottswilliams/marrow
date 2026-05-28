use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::check_project;
use marrow_project::parse_config;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

#[test]
fn clean_project_has_no_diagnostics() {
    let root = temp_project("clean", |root| {
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        // A module-less file is a script and is not bound to its path.
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let report = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn reports_module_path_mismatch() {
    let root = temp_project("mismatch", |root| {
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let report = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.module_path")
        .expect("module-path diagnostic");
    assert!(
        diagnostic.message.contains("shelf::books"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.file.ends_with("books.mw"),
        "{:?}",
        diagnostic.file
    );
}

#[test]
fn surfaces_parse_diagnostics_with_file_path() {
    let root = temp_project("parse-error", |root| {
        // A tab is a lexical error in Marrow source.
        write(root, "src/bad.mw", "module bad\n\tconst X: int = 1\n");
    });
    let report = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "parse.syntax")
        .expect("parse diagnostic");
    assert!(diagnostic.file.ends_with("bad.mw"), "{:?}", diagnostic.file);
}

#[test]
fn a_dotted_stem_file_cannot_be_a_module() {
    let root = temp_project("dotted-stem", |root| {
        // `config.v2.mw` implies the module path `config.v2`, which is not a
        // valid name, so any module declaration mismatches it. Such files must
        // be scripts.
        write(root, "src/config.v2.mw", "module config\n");
    });
    let report = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.module_path")
        .expect("module-path diagnostic");
    assert!(
        diagnostic.message.contains("config.v2"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn reports_duplicate_module_across_source_roots() {
    let root = temp_project("duplicate", |root| {
        write(root, "src/shared.mw", "module shared\n");
        write(root, "lib/shared.mw", "module shared\n");
    });
    let config = parse_config(r#"{ "sourceRoots": ["src", "lib"] }"#).expect("config");

    let report = check_project(&root, &config).expect("check");
    fs::remove_dir_all(&root).ok();

    let duplicates: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "check.duplicate_module")
        .collect();
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        duplicates[0].message.contains("shared"),
        "{}",
        duplicates[0].message
    );
}

#[test]
fn distinct_modules_are_not_flagged_as_duplicates() {
    let root = temp_project("distinct", |root| {
        write(root, "src/a.mw", "module a\n");
        write(root, "src/b.mw", "module b\n");
    });
    let report = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_script_file_is_not_bound_to_its_path() {
    let root = temp_project("script", |root| {
        // No module declaration: a script, even at a nested path.
        write(root, "src/tools/migrate.mw", "fn run()\n    return\n");
    });
    let report = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}
