use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

/// A project whose `Book` resource has a unique `byTitle` index and a public
/// function, plus a private one, for both halves of explain.
fn book_project(name: &str) -> PathBuf {
    temp_project(name, |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20required title: string\n\
             \x20\x20\x20\x20pages: int\n\n\
             \x20\x20\x20\x20index byTitle(title) unique\n\n\
             pub fn add()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\n\
             fn helper(): int\n\
             \x20\x20\x20\x20return 1\n",
        );
    })
}

#[test]
fn explains_a_saved_field_path_with_its_index() {
    // `^books(1).title` resolves to the `title` field of resource `Book`, type
    // string, and it feeds the unique `byTitle` index.
    let project = book_project("explain-field");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", &dir, "^books(1).title"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("title"), "{stdout}");
    assert!(stdout.contains("Book"), "{stdout}");
    assert!(stdout.contains("string"), "{stdout}");
    assert!(stdout.contains("byTitle"), "{stdout}");
}

#[test]
fn explains_an_index_marker_path() {
    // `^books.byTitle("x")` is a generated index entry, classified as an index
    // marker (not a typed scalar leaf).
    let project = book_project("explain-index");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", &dir, "^books.byTitle(\"x\")"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.contains("index") && stdout.contains("byTitle"),
        "{stdout}"
    );
}

#[test]
fn explains_an_orphan_path() {
    // `^bogus(1).x` is under no declared root: an orphan.
    let project = book_project("explain-orphan");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", &dir, "^bogus(1).x"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("orphan"), "{stdout}");
}

#[test]
fn explains_a_saved_path_as_json() {
    let project = book_project("explain-field-json");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "^books(1).title"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "saved_path");
    assert_eq!(value["class"], "scalar");
    assert_eq!(value["type"], "string");
    assert_eq!(value["root"], "books");
    let indexes = value["indexes"].as_array().expect("indexes array");
    assert!(
        indexes.iter().any(|index| index["name"] == "byTitle"),
        "{stdout}"
    );
}

#[test]
fn explains_a_public_function_name() {
    // A public `fn` resolves: found, in module `shelf`, kind function.
    let project = book_project("explain-name-found");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "shelf::add"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "name");
    assert_eq!(value["resolution"], "found");
    assert_eq!(value["module"], "shelf");
    assert_eq!(value["resolved_kind"], "function");
}

#[test]
fn explains_a_not_visible_qualified_name() {
    // A non-pub function reached by a qualified path is not visible.
    let project = book_project("explain-name-private");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "shelf::helper"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "name");
    assert_eq!(value["resolution"], "not_visible");
}

#[test]
fn explains_a_module_qualified_resource_name() {
    // A module-qualified resource name resolves to the resource declaration.
    let project = book_project("explain-qualified-resource");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "shelf::Book"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "name");
    assert_eq!(value["resolution"], "found");
    assert_eq!(value["module"], "shelf");
    assert_eq!(value["resolved_kind"], "resource");
}

#[test]
fn explains_a_typed_reference_field() {
    // `^books(1).authorId` is a typed-reference field (`authorId: Id(^authors)`),
    // classified as an identity leaf naming the referenced store — not a scalar.
    let project = temp_project("explain-ref-field", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\n\
             resource Author\n\
             \x20\x20\x20\x20required name: string\n\n\
             store ^authors(id: int): Author\n\n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             \x20\x20\x20\x20authorId: Id(^authors)\n\n\
             store ^books(id: int): Book\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "^books(1).authorId"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "saved_path");
    assert_eq!(value["class"], "identity");
    assert_eq!(value["type"], "Id(^authors)");
    assert_eq!(value["root"], "books");
}

#[test]
fn explains_a_bare_resource_name() {
    // A bare `Book` is neither a function nor in the empty module; it falls back to
    // a project-wide resource-name lookup and resolves to resource `Book`.
    let project = book_project("explain-bare-resource");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "Book"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "name");
    assert_eq!(value["resolution"], "found");
    assert_eq!(value["module"], "shelf");
    assert_eq!(value["resolved_kind"], "resource");
}

#[test]
fn explains_an_ambiguous_bare_name() {
    // Two modules each expose a public `widget`; a bare `widget` is ambiguous and
    // names both candidate modules.
    let project = temp_project("explain-name-ambiguous", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/a.mw",
            "module a\n\npub fn widget(): int\n    return 1\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\npub fn widget(): int\n    return 2\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["explain", "--format", "json", &dir, "widget"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["resolution"], "ambiguous");
    let candidates = value["candidates"].as_array().expect("candidates array");
    let names: Vec<&str> = candidates.iter().filter_map(|c| c.as_str()).collect();
    assert!(names.contains(&"a") && names.contains(&"b"), "{stdout}");
}

#[test]
fn a_malformed_saved_path_is_a_usage_error() {
    let project = book_project("explain-bad-path");
    let dir = project.to_str().unwrap().to_string();
    // A leading `^` with no root name is malformed.
    let output = marrow(&["explain", &dir, "^"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
