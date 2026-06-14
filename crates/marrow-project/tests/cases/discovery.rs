use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use marrow_project::{discover_modules, discover_test_modules, parse_config};

/// A temporary project directory removed when the value is dropped.
///
/// Derefs to its root [`Path`], so it passes straight into discovery without an
/// explicit accessor, and the drop removes the directory even when an assertion
/// panics, so a failing test never leaks its temp dir.
struct TempProject {
    root: PathBuf,
}

impl Deref for TempProject {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// Create a unique temporary project directory and run `build` to populate it.
fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    TempProject { root }
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
    let config = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
        .expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    let found: Vec<(PathBuf, Option<String>)> = modules
        .iter()
        .map(|m| (m.relative_path.clone(), m.module_name.clone()))
        .collect();

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
    let config =
        parse_config(r#"{ "sourceRoots": ["src", "lib"], "store": { "backend": "memory" } }"#)
            .expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    let names: Vec<Option<String>> = modules.iter().map(|m| m.module_name.clone()).collect();

    assert_eq!(modules.len(), 2, "{modules:#?}");
    assert!(names.contains(&Some("a".to_string())));
    assert!(names.contains(&Some("b".to_string())));
}

#[test]
fn overlapping_source_roots_discover_each_file_once() {
    // Nested source roots ("src" and "src/sub") both reach src/sub/x.mw. Without
    // dedup it is discovered twice — as `sub::x` under "src" and as `x` under
    // "src/sub" — and the second relative path bogusly mismatches its own
    // declaration. The first source root's relative path (and module name) wins.
    let root = temp_project("overlapping-roots", |root| {
        write(root, "src/sub/x.mw", "module sub::x\n");
    });
    let config =
        parse_config(r#"{ "sourceRoots": ["src", "src/sub"], "store": { "backend": "memory" } }"#)
            .expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    let found: Vec<(PathBuf, Option<String>)> = modules
        .iter()
        .map(|m| (m.relative_path.clone(), m.module_name.clone()))
        .collect();

    assert_eq!(modules.len(), 1, "{modules:#?}");
    assert_eq!(
        found[0],
        (
            PathBuf::from("sub").join("x.mw"),
            Some("sub::x".to_string())
        )
    );
}

#[test]
fn an_empty_source_root_yields_no_modules() {
    // A source root that exists but holds no `.mw` files is valid, not an error.
    let root = temp_project("empty-root", |root| {
        fs::create_dir_all(root.join("src")).expect("create src");
    });
    let config = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
        .expect("config");

    let modules = discover_modules(&root, &config).expect("discover");
    assert!(modules.is_empty(), "{modules:#?}");
}

#[test]
fn errors_when_a_source_root_is_missing() {
    let root = temp_project("missing-root", |_| {});
    let config = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
        .expect("config");

    let error = discover_modules(&root, &config).expect_err("missing source root should error");
    assert_eq!(error.code, "project.source_root");
}

#[test]
fn discovers_test_files_from_a_plain_directory_recursively() {
    let root = temp_project("test-directory", |root| {
        write(root, "src/app.mw", "module app\n");
        write(root, "tests/books_test.mw", "pub fn ok()\n    return\n");
        write(root, "tests/deep/more_test.mw", "pub fn ok()\n    return\n");
        write(root, "tests/notes.txt", "ignore me");
    });
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");

    let modules = discover_test_modules(&root, &config).expect("discover tests");
    let names: Vec<Option<String>> = modules.iter().map(|m| m.module_name.clone()).collect();

    // Only `.mw` files under the configured directory, with project-relative names.
    assert_eq!(modules.len(), 2, "{modules:#?}");
    assert!(names.contains(&Some("tests::books_test".to_string())));
    assert!(names.contains(&Some("tests::deep::more_test".to_string())));
}

#[test]
fn star_test_entry_is_invalid_config() {
    let error = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests/*.mw"] }"#,
    )
    .expect_err("glob-like test entry should fail closed");

    assert_eq!(error.code, "config.invalid");
}

#[test]
fn test_paths_accept_a_bare_directory_or_file() {
    let root = temp_project("test-bare", |root| {
        write(root, "checks/a_test.mw", "pub fn ok()\n    return\n");
        write(root, "checks/deep/b_test.mw", "pub fn ok()\n    return\n");
        write(root, "smoke.mw", "pub fn ok()\n    return\n");
    });
    let config = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["checks", "smoke.mw"] }"#)
        .expect("config");

    let modules = discover_test_modules(&root, &config).expect("discover tests");
    let names: Vec<Option<String>> = modules.iter().map(|m| m.module_name.clone()).collect();

    assert_eq!(modules.len(), 3, "{modules:#?}");
    assert!(names.contains(&Some("checks::a_test".to_string())));
    assert!(names.contains(&Some("checks::deep::b_test".to_string())));
    assert!(names.contains(&Some("smoke".to_string())));
}

#[cfg(unix)]
#[test]
fn configured_test_symlink_is_not_followed() {
    let root = temp_project("test-symlink-root", |root| {
        write(root, "real_tests/smoke.mw", "pub fn ok()\n    return\n");
        std::os::unix::fs::symlink(root.join("real_tests"), root.join("tests"))
            .expect("create test symlink");
    });
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");

    let modules = discover_test_modules(&root, &config).expect("discover tests");

    assert!(modules.is_empty(), "{modules:#?}");
}

#[test]
fn a_missing_test_directory_yields_no_tests() {
    // A `tests` path that matches nothing is not an error — there are simply no
    // tests to run.
    let root = temp_project("test-missing", |root| {
        write(root, "src/app.mw", "module app\n");
    });
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");

    let modules = discover_test_modules(&root, &config).expect("discover tests");
    assert!(modules.is_empty(), "{modules:#?}");
}
