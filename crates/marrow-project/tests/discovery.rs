use std::fs;
use std::path::{Path, PathBuf};

use marrow_project::{discover_modules, parse_config};

/// Create a unique temporary project directory and run `build` to populate it.
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

#[test]
fn discovers_mw_files_with_module_names() {
    let root = temp_project("discover", |root| {
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        write(root, "src/main.mw", "fn main()\n    return\n");
        write(root, "src/notes.txt", "ignore me");
        write(
            root,
            "src/nested/deep/thing.mw",
            "module nested::deep::thing\n",
        );
    });
    let config = parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    let found: Vec<(PathBuf, Option<String>)> = modules
        .iter()
        .map(|m| (m.relative_path.clone(), m.module_name.clone()))
        .collect();

    fs::remove_dir_all(&root).ok();

    // Only `.mw` files, sorted by absolute path, with derived module names.
    assert_eq!(found.len(), 3, "{found:#?}");
    assert!(found.contains(&(PathBuf::from("main.mw"), Some("main".to_string()))));
    assert!(found.contains(&(
        PathBuf::from("shelf").join("books.mw"),
        Some("shelf::books".to_string())
    )));
    assert!(found.contains(&(
        PathBuf::from("nested").join("deep").join("thing.mw"),
        Some("nested::deep::thing".to_string())
    )));
}

#[test]
fn searches_each_configured_source_root() {
    let root = temp_project("multi-root", |root| {
        write(root, "src/a.mw", "module a\n");
        write(root, "lib/b.mw", "module b\n");
    });
    let config = parse_config(r#"{ "sourceRoots": ["src", "lib"] }"#).expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    let names: Vec<Option<String>> = modules.iter().map(|m| m.module_name.clone()).collect();

    fs::remove_dir_all(&root).ok();
    assert_eq!(modules.len(), 2, "{modules:#?}");
    assert!(names.contains(&Some("a".to_string())));
    assert!(names.contains(&Some("b".to_string())));
}

#[test]
fn an_empty_source_root_yields_no_modules() {
    // A source root that exists but holds no `.mw` files is valid, not an error.
    let root = temp_project("empty-root", |root| {
        fs::create_dir_all(root.join("src")).expect("create src");
    });
    let config = parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    fs::remove_dir_all(&root).ok();
    assert!(modules.is_empty(), "{modules:#?}");
}

#[test]
fn errors_when_a_source_root_is_missing() {
    let root = temp_project("missing-root", |_| {});
    let config = parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config");

    let error = discover_modules(&root, &config).expect_err("missing source root should error");
    fs::remove_dir_all(&root).ok();
    assert_eq!(error.code, "project.source_root");
}
