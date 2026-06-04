use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{check_project, resolve::resolve_store_by_root};
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
fn store_resolver_returns_store_module_and_resource_context() {
    let root = temp_project("store-resolver-context", |root| {
        write(
            root,
            "src/catalog.mw",
            "module catalog\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             store ^archivedBooks(id: int): Book\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let books = resolve_store_by_root(&program, "books").expect("books store");
    let archived = resolve_store_by_root(&program, "archivedBooks").expect("archived store");

    assert_eq!(books.module.name, "catalog");
    assert_eq!(books.store.root, "books");
    assert_eq!(books.resource.name, "Book");
    assert_eq!(books.store.identity_type().to_string(), "Id(^books)");
    assert_eq!(archived.module.name, "catalog");
    assert_eq!(archived.store.root, "archivedBooks");
    assert_eq!(archived.resource.name, "Book");
    assert_eq!(
        archived.store.identity_type().to_string(),
        "Id(^archivedBooks)"
    );
}

#[test]
fn store_indexes_are_checked_facts_not_resource_members() {
    let root = temp_project("store-index-facts", |root| {
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title, id)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("shelf").expect("shelf module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    let store = program
        .facts
        .store_id(module, "books")
        .expect("books store");
    assert_eq!(program.facts.store(store).resource, resource);
    assert_eq!(program.facts.store_indexes().len(), 1);
    let index = &program.facts.store_indexes()[0];
    assert_eq!(index.store, store);
    assert_eq!(index.name, "byTitle");
    assert!(
        program
            .facts
            .resource_members()
            .iter()
            .all(|member| member.name != "byTitle"),
        "{:#?}",
        program.facts.resource_members()
    );
}
