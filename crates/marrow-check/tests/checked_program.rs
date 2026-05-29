use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{MarrowType, PrimitiveType, check_project};
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
fn builds_a_module_for_a_clean_library_file() {
    let root = temp_project("program-clean", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn add(title: string): Book::Id\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert_eq!(program.modules.len(), 1, "{program:#?}");

    let module = &program.modules[0];
    assert_eq!(module.name, "shelf::books");

    assert_eq!(module.resources.len(), 1, "{:#?}", module.resources);
    assert_eq!(module.resources[0].name, "Book");

    let add = module
        .functions
        .iter()
        .find(|function| function.name == "add")
        .expect("add function");
    assert!(add.public, "{add:#?}");
    assert_eq!(add.params.len(), 1, "{:#?}", add.params);
    assert_eq!(add.params[0].name, "title");
    assert_eq!(
        add.params[0].ty,
        MarrowType::Primitive(PrimitiveType::String)
    );
    assert!(add.return_type.is_some(), "{add:#?}");
    // `add`'s body touches the `^books` saved root (allocating an id with `nextId`).
    assert!(add.touches_saved_data, "{add:#?}");
    // The body is carried into the artifact for the runtime to evaluate.
    assert!(!add.body.statements.is_empty(), "{add:#?}");
}

/// `nextId(^books)` over a single-`int` root types to `Book::Id`, so a function
/// returning it under a declared `Book::Id` return type checks clean. (`nextId`
/// is a saved-data read, so it lives in a function body, not a module const.)
/// Previously `nextId` typed to `Unknown`. The local-const annotation
/// `const id: Book::Id = nextId(^books)` likewise checks clean.
#[test]
fn next_id_types_to_the_resource_identity() {
    let root = temp_project("program-nextid-id", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn fresh(): Book::Id\n\
             \x20   const id: Book::Id = nextId(^books)\n\
             \x20   return id\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `nextId` over a composite-identity root is rejected at check time with
/// `check.next_id_requires_single_int`, so the misuse is caught before running.
#[test]
fn next_id_over_a_composite_root_is_flagged() {
    let root = temp_project("program-nextid-composite", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment at ^enrollments(studentId: string, courseId: string)\n\
             \x20   required grade: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^enrollments)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over a single non-integer (string) root is flagged the same way.
#[test]
fn next_id_over_a_string_keyed_root_is_flagged() {
    let root = temp_project("program-nextid-string", |root| {
        write(
            root,
            "src/shelf/tags.mw",
            "module shelf::tags\n\
             resource Tag at ^tags(slug: string)\n\
             \x20   required name: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^tags)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over a keyless singleton root is flagged: a singleton has no
/// generated identity (types.md:262-263).
#[test]
fn next_id_over_a_singleton_root_is_flagged() {
    let root = temp_project("program-nextid-singleton", |root| {
        write(
            root,
            "src/shelf/settings.mw",
            "module shelf::settings\n\
             resource Settings at ^settings\n\
             \x20   required theme: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^settings)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `use std::clock` lets a short-form `clock::now()` resolve and type to its
/// declared result (`instant`), just as the fully-qualified form does.
#[test]
fn short_form_std_import_resolves() {
    let root = temp_project("program-shortform-clock", |root| {
        write(
            root,
            "src/shelf/times.mw",
            "module shelf::times\n\
             use std::clock\n\
             pub fn stamp(): instant\n\
             \x20   return clock::now()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Without the import, the short-form `clock::now()` does not resolve and reports
/// `check.unresolved_call` — short-form requires the matching `use`.
#[test]
fn short_form_without_import_is_unresolved() {
    let root = temp_project("program-shortform-noimport", |root| {
        write(
            root,
            "src/shelf/times.mw",
            "module shelf::times\n\
             pub fn stamp(): instant\n\
             \x20   return clock::now()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.unresolved_call"),
        "{:#?}",
        report.diagnostics
    );
}

/// Short-form works for project modules too: `use shelf::books` lets `books::add`
/// resolve to the qualified function in that module.
#[test]
fn short_form_project_import_resolves() {
    let root = temp_project("program-shortform-project", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             pub fn make(): int\n\
             \x20   return 1\n",
        );
        write(
            root,
            "src/shelf/app.mw",
            "module shelf::app\n\
             use shelf::books\n\
             pub fn run(): int\n\
             \x20   return books::make()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_file_with_a_parse_error_contributes_no_module() {
    let root = temp_project("program-parse-error", |root| {
        // A leading tab is a lexical error, so the file parses with errors and
        // is excluded from the artifact this slice.
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\tconst X = 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(report.has_errors(), "{:#?}", report.diagnostics);
    assert!(program.modules.is_empty(), "{program:#?}");
}
