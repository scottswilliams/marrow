mod support;

use std::collections::BTreeSet;

use marrow_check::{CheckedBody, CheckedExpr, CheckedProgram, CheckedStmt, StoreIndexId};
use marrow_check::{MarrowType, WriteFallibilityFact, check_project, check_tests_program};
use marrow_project::parse_config;
use marrow_schema::ReturnPresence;
use marrow_store::value::ScalarType;

use support::{config, temp_project, write};

#[test]
fn builds_a_module_for_a_clean_library_file() {
    let root = temp_project("program-clean", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

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
    assert_eq!(add.params[0].ty, MarrowType::Primitive(ScalarType::Str));
    assert!(add.return_type.is_some(), "{add:#?}");
    assert!(
        add.runtime_body()
            .is_some_and(|body| !body.statements().is_empty()),
        "{add:#?}"
    );
}

#[test]
#[should_panic(expected = "checked program is missing captured durable source renderings")]
fn manually_assembled_non_empty_program_cannot_claim_source_digest() {
    let root = temp_project("program-manual-digest", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let manual = CheckedProgram::from_modules(program.modules.clone());
    let _ = manual.source_digest();
}

#[test]
#[should_panic(
    expected = "checked program is missing captured durable source renderings for module `books`"
)]
fn test_program_finalization_does_not_mask_manual_source_digest() {
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let root = temp_project("program-manual-test-digest", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n",
        );
        write(root, "tests/smoke.mw", "fn smoke()\n    var x = 1\n");
    });
    let (report, program) = check_project(&root, &cfg).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let manual = CheckedProgram::from_modules(program.modules.clone());
    let (test_report, combined) = check_tests_program(&root, &cfg, manual).expect("check tests");
    assert!(!test_report.has_errors(), "{:#?}", test_report.diagnostics);
    let _ = combined.source_digest();
}

#[test]
fn checked_functions_do_not_carry_source_bodies() {
    let root = temp_project("program-runtime-rebuilds-executables", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             pub fn main(): int\n\
             \x20   return 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let runtime = program.runtime();
    let body = runtime.modules()[0].functions()[0]
        .body()
        .expect("runtime body");

    assert_eq!(
        body.statements().len(),
        1,
        "runtime() must consume the checked executable body"
    );
}

#[test]
fn function_descriptors_preserve_return_presence() {
    let root = temp_project("program-return-presence", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             pub fn maybe_title(): maybe string\n\
             \x20   return absent\n\n\
             pub fn title(): string\n\
             \x20   return \"present\"\n\n\
             pub fn log()\n\
             \x20   print(\"void\")\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let module = &program.modules[0];
    let maybe = module
        .functions
        .iter()
        .find(|function| function.name == "maybe_title")
        .expect("maybe function");
    let title = module
        .functions
        .iter()
        .find(|function| function.name == "title")
        .expect("title function");
    let log = module
        .functions
        .iter()
        .find(|function| function.name == "log")
        .expect("log function");

    assert_eq!(maybe.return_presence, ReturnPresence::MaybePresent);
    assert_eq!(title.return_presence, ReturnPresence::Always);
    assert_eq!(log.return_presence, ReturnPresence::Always);
    assert!(maybe.return_type.is_some(), "{maybe:#?}");
    assert!(title.return_type.is_some(), "{title:#?}");
    assert!(log.return_type.is_none(), "{log:#?}");

    let runtime = program.runtime();
    let runtime_module = &runtime.modules()[0];
    let runtime_maybe = runtime_module
        .functions()
        .iter()
        .find(|function| function.name == "maybe_title")
        .expect("runtime maybe function");
    let runtime_title = runtime_module
        .functions()
        .iter()
        .find(|function| function.name == "title")
        .expect("runtime title function");
    let runtime_log = runtime_module
        .functions()
        .iter()
        .find(|function| function.name == "log")
        .expect("runtime log function");

    assert_eq!(runtime_maybe.return_presence, ReturnPresence::MaybePresent);
    assert_eq!(runtime_title.return_presence, ReturnPresence::Always);
    assert_eq!(runtime_log.return_presence, ReturnPresence::Always);
    assert!(runtime_log.return_type.is_none(), "{runtime_log:#?}");
}

#[test]
fn write_fallibility_marks_unique_conflict_assignments() {
    let root = temp_project("program-write-fallibility-unique", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   required isbn: string\n\
             \x20   title: string\n\
             \x20   author: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             \x20   index byTitle(title) unique\n\
             pub fn write(id: Id(^books), book: Book, title: string, author: string)\n\
             \x20   ^books(id) = book\n\
             \x20   ^books(id).title = title\n\
             \x20   ^books(id).author = author\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let by_isbn = store_index_id(&program, "byIsbn");
    let by_title = store_index_id(&program, "byTitle");
    let body = function_body(&program, "write");
    let statements = body.statements();

    assert_eq!(
        assign_fallibility(&statements[0]),
        &WriteFallibilityFact::UniqueConflict(BTreeSet::from([by_isbn, by_title]))
    );
    assert_eq!(
        assign_fallibility(&statements[1]),
        &WriteFallibilityFact::UniqueConflict(BTreeSet::from([by_title]))
    );
    assert_eq!(
        assign_fallibility(&statements[2]),
        &WriteFallibilityFact::Infallible
    );
}

#[test]
fn write_fallibility_marks_maintenance_gated_deletes() {
    let root = temp_project("program-write-fallibility-delete", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   meta\n\
             \x20       required digest: string\n\
             store ^books(id: int): Book\n\
             pub fn cleanup(id: Id(^books))\n\
             \x20   delete ^books\n\
             \x20   delete ^books(id)\n\
             \x20   delete ^books(id).title\n\
             \x20   delete ^books(id).meta\n\
             \x20   delete ^books(id).subtitle\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let body = function_body(&program, "cleanup");
    let statements = body.statements();

    assert_eq!(
        delete_fallibility(&statements[0]),
        &WriteFallibilityFact::MaintenanceGated
    );
    assert_eq!(
        delete_fallibility(&statements[1]),
        &WriteFallibilityFact::Infallible
    );
    assert_eq!(
        delete_fallibility(&statements[2]),
        &WriteFallibilityFact::MaintenanceGated
    );
    assert_eq!(
        delete_fallibility(&statements[3]),
        &WriteFallibilityFact::MaintenanceGated
    );
    assert_eq!(
        delete_fallibility(&statements[4]),
        &WriteFallibilityFact::Infallible
    );
}

#[test]
fn write_fallibility_marks_append_calls_in_checked_ir() {
    let root = temp_project("program-write-fallibility-append", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
             pub fn add(id: Id(^books), tag: string): int\n\
             \x20   return append(^books(id).tags, tag)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let body = function_body(&program, "add");
    let CheckedStmt::Return {
        value: Some(CheckedExpr::Call {
            write_fallibility, ..
        }),
        ..
    } = &body.statements()[0]
    else {
        panic!("{:#?}", body.statements());
    };
    assert_eq!(write_fallibility, &Some(WriteFallibilityFact::Infallible));
}

#[test]
fn a_file_with_a_parse_error_contributes_no_module() {
    let root = temp_project("program-parse-error", |root| {
        // A leading tab is a lexical error, so the file parses with errors and
        // is excluded from the artifact.
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\tconst X = 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(report.has_errors(), "{:#?}", report.diagnostics);
    assert!(program.modules.is_empty(), "{program:#?}");
}

fn function_body<'a>(program: &'a CheckedProgram, name: &str) -> &'a CheckedBody {
    program
        .modules
        .iter()
        .flat_map(|module| module.functions.iter())
        .find(|function| function.name == name)
        .and_then(|function| function.runtime_body())
        .unwrap_or_else(|| panic!("checked function body {name}"))
}

fn store_index_id(program: &CheckedProgram, name: &str) -> StoreIndexId {
    program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.name == name)
        .map(|index| index.id)
        .unwrap_or_else(|| panic!("store index {name}"))
}

fn assign_fallibility(statement: &CheckedStmt) -> &WriteFallibilityFact {
    let CheckedStmt::Assign { fallibility, .. } = statement else {
        panic!("{statement:#?}");
    };
    fallibility
}

fn delete_fallibility(statement: &CheckedStmt) -> &WriteFallibilityFact {
    let CheckedStmt::Delete { fallibility, .. } = statement else {
        panic!("{statement:#?}");
    };
    fallibility
}
