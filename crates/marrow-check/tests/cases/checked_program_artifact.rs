use crate::support;

use marrow_check::CheckedProgram;
use marrow_check::{MarrowType, check_project};
use marrow_store::value::ScalarType;

use support::{config, temp_project, write};

#[test]
fn runtime_program_exposes_statement_stop_points_by_file_id() {
    let root = temp_project("program-runtime-stop-points", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             const BASE: int = 1\n\
             pub fn run(flag: bool): int\n\
             \x20   const first = BASE\n\
             \x20   if flag\n\
             \x20       print(first)\n\
             \x20   else\n\
             \x20       print(0)\n\
             \x20   return first\n\
             fn helper(): int\n\
             \x20   while false\n\
             \x20       print(2)\n\
             \x20   return 3\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let runtime = program.runtime();
    let stop_points = runtime.stop_points();
    let lines: Vec<u32> = stop_points.iter().map(|point| point.span.line).collect();

    assert_eq!(lines, vec![4, 5, 6, 8, 9, 11, 12, 13]);
    assert!(
        stop_points.iter().all(|point| runtime
            .file_path(point.file_id)
            .is_some_and(|path| path.ends_with("src/app.mw"))),
        "{stop_points:#?}"
    );
}

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

/// The shape digest is a pure function of the durable schema structure — resources, members,
/// stores, indexes, enums, and consts — so reconstructing a program from its modules alone
/// reproduces exactly the digest the full pipeline stamped. It needs no captured source text or
/// binding side table: the schema the modules already carry is the whole input.
#[test]
fn manually_assembled_program_reproduces_the_structural_source_digest() {
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
    assert_eq!(
        program.source_digest(),
        manual.source_digest(),
        "the shape digest must depend only on the durable schema the modules carry"
    );
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
fn function_descriptors_carry_optional_return_types() {
    let root = temp_project("program-return-presence", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             pub fn maybe_title(): string?\n\
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

    assert!(maybe.returns_maybe_present(), "{maybe:#?}");
    assert!(!title.returns_maybe_present(), "{title:#?}");
    assert!(!log.returns_maybe_present(), "{log:#?}");
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

    assert!(runtime_maybe.returns_maybe_present(), "{runtime_maybe:#?}");
    assert!(!runtime_title.returns_maybe_present(), "{runtime_title:#?}");
    assert!(!runtime_log.returns_maybe_present(), "{runtime_log:#?}");
    assert!(runtime_log.return_type.is_none(), "{runtime_log:#?}");
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
