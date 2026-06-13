mod support;

use marrow_check::{MarrowType, check_project};
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
