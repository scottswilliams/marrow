use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{check_project, check_tests};
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

/// The `.mw` code block from the canonical reference sample.
fn sample_source() -> String {
    let doc = include_str!("../../../docs/language/sample.md");
    doc.split("```mw")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the sample document has an mw code block")
        .to_string()
}

#[test]
fn the_reference_sample_checks_clean() {
    // The canonical sample (`module shelf::sample`) must check with no diagnostics
    // — in particular no false `check.unresolved_call` on its builtins
    // (keys/append/exists/nextId/...), which would mean `is_builtin_call` no
    // longer recognizes the full set of builtins.
    let root = temp_project("sample-check", |root| {
        write(root, "src/shelf/sample.mw", &sample_source());
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn surfaces_resource_schema_errors() {
    let root = temp_project("schema-error", |root| {
        // An index is only valid as a direct member of a saved resource, not
        // inside a child layer.
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   notes(noteId: string)\n\
             \x20       index bad(noteId)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "schema.index_in_group"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn reports_two_resources_owning_one_saved_root() {
    let root = temp_project("dup-root", |root| {
        // A saved root has one managed owner; two resources on `^books` collide.
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             resource Tome at ^books(id: int)\n\
             \x20   required title: string\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let owners = with_code(&report, "schema.duplicate_root_owner");
    assert_eq!(owners.len(), 1, "{:#?}", report.diagnostics);
    assert!(owners[0].message.contains("books"), "{}", owners[0].message);
}

#[test]
fn reports_stable_id_reused_across_resources() {
    let root = temp_project("dup-stable-id", |root| {
        // A stable id must be unique across the whole project, not only within
        // one resource.
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book at ^books(id: int)\n\
             \x20   @id(\"shared\")\n\
             \x20   required title: string\n\
             resource Shelf at ^shelves(id: int)\n\
             \x20   @id(\"shared\")\n\
             \x20   required name: string\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let dups = with_code(&report, "schema.duplicate_stable_id");
    assert_eq!(dups.len(), 1, "{:#?}", report.diagnostics);
    assert!(dups[0].message.contains("shared"), "{}", dups[0].message);
}

#[test]
fn clean_project_has_no_diagnostics() {
    let root = temp_project("clean", |root| {
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        // A module-less file is a script and is not bound to its path.
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn reports_module_path_mismatch() {
    let root = temp_project("mismatch", |root| {
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
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
    let (report, _program) = check_project(&root, &config()).expect("check");
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
    let (report, _program) = check_project(&root, &config()).expect("check");
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

    let (report, _program) = check_project(&root, &config).expect("check");
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
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_script_file_is_not_bound_to_its_path() {
    let root = temp_project("script", |root| {
        // No module declaration: a script, even at a nested path.
        write(root, "src/tools/migrate.mw", "fn run()\n    return\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

fn duplicate_declarations(
    report: &marrow_check::CheckReport,
) -> Vec<&marrow_check::CheckDiagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code == "check.duplicate_declaration")
        .collect()
}

#[test]
fn reports_duplicate_function_declaration() {
    let root = temp_project("dup-fn", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nfn run()\n    return\nfn run()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        duplicates[0].message.contains("run"),
        "{}",
        duplicates[0].message
    );
    // The later occurrence is reported.
    assert_eq!(duplicates[0].line, 4, "{:#?}", duplicates[0]);
}

#[test]
fn reports_duplicate_const_declaration() {
    let root = temp_project("dup-const", |root| {
        write(root, "src/m.mw", "module m\nconst A = 1\nconst A = 2\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        duplicates[0].message.contains('A'),
        "{}",
        duplicates[0].message
    );
}

#[test]
fn reports_duplicate_resource_declaration() {
    let root = temp_project("dup-resource", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nresource Book\n    title: string\nresource Book\n    title: string\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        duplicates[0].message.contains("Book"),
        "{}",
        duplicates[0].message
    );
}

#[test]
fn reports_const_resource_name_collision() {
    let root = temp_project("const-resource", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nconst Book = 1\nresource Book\n    title: string\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        duplicates[0].message.contains("Book"),
        "{}",
        duplicates[0].message
    );
}

#[test]
fn reports_import_short_name_collision_with_declaration() {
    let root = temp_project("use-collision", |root| {
        // `use shelf::books` contributes the short name `books`, which collides
        // with the declared function of the same name.
        write(
            root,
            "src/m.mw",
            "module m\nuse shelf::books\nfn books()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        duplicates[0].message.contains("books"),
        "{}",
        duplicates[0].message
    );
    // The function declaration is the later occurrence.
    assert_eq!(duplicates[0].line, 3, "{:#?}", duplicates[0]);
}

#[test]
fn distinct_declarations_are_not_flagged() {
    let root = temp_project("distinct-decls", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nuse shelf::books\nconst A = 1\nresource Book\n    title: string\nfn run()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

fn unresolved_imports(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code == "check.unresolved_import")
        .collect()
}

#[test]
fn standard_library_and_project_imports_resolve() {
    let root = temp_project("resolved-imports", |root| {
        // A project library module.
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        // A script that imports a std module and the project module.
        write(
            root,
            "src/app.mw",
            "use std::clock\nuse shelf::books\nfn main()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        unresolved_imports(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn reports_unresolved_import() {
    let root = temp_project("unresolved-import", |root| {
        write(
            root,
            "src/app.mw",
            "use unknown::mod\nfn main()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let unresolved = unresolved_imports(&report);
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        unresolved[0].message.contains("unknown::mod"),
        "{}",
        unresolved[0].message
    );
    assert_eq!(unresolved[0].line, 1, "{:#?}", unresolved[0]);
}

#[test]
fn checks_test_files_into_named_modules() {
    let root = temp_project("check-tests-ok", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn add(): int\n    return 1\n",
        );
        write(
            root,
            "tests/app_test.mw",
            "pub fn add_returns_one()\n    std::assert::isTrue(app::add() = 1)\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, test_modules) = check_tests(&root, &cfg, &src_program).expect("check tests");
    fs::remove_dir_all(&root).ok();

    assert!(!src_report.has_errors(), "{:#?}", src_report.diagnostics);
    assert!(!test_report.has_errors(), "{:#?}", test_report.diagnostics);
    assert_eq!(test_modules.len(), 1, "{test_modules:#?}");
    // A module-less test file is named from its project-relative path.
    assert_eq!(test_modules[0].name, "tests::app_test");
    assert!(
        test_modules[0]
            .functions
            .iter()
            .any(|f| f.name == "add_returns_one" && f.public && f.params.is_empty()),
        "{:#?}",
        test_modules[0].functions
    );
}

#[test]
fn reports_a_parse_error_in_a_test_file() {
    let root = temp_project("check-tests-bad", |root| {
        write(root, "src/app.mw", "module app\n");
        // A tab is a lexical error.
        write(
            root,
            "tests/bad_test.mw",
            "pub fn t()\n\tstd::assert::fail(\"x\")\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (_src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, _modules) = check_tests(&root, &cfg, &src_program).expect("check tests");
    fs::remove_dir_all(&root).ok();

    assert!(
        test_report
            .diagnostics
            .iter()
            .any(|d| d.code == "parse.syntax"),
        "{:#?}",
        test_report.diagnostics
    );
}

#[test]
fn a_test_file_is_named_from_its_path_not_a_declared_module() {
    let root = temp_project("test-name-path", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn add(): int\n    return 1\n",
        );
        // Even though this test file declares `module app`, it must be named from
        // its path so it cannot shadow the project's `app` module.
        write(
            root,
            "tests/app_test.mw",
            "module app\n\npub fn calls_app()\n    std::assert::isTrue(app::add() = 1)\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (_src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, test_modules) = check_tests(&root, &cfg, &src_program).expect("check tests");
    fs::remove_dir_all(&root).ok();

    assert!(!test_report.has_errors(), "{:#?}", test_report.diagnostics);
    assert_eq!(test_modules.len(), 1, "{test_modules:#?}");
    assert_eq!(test_modules[0].name, "tests::app_test");
}

#[test]
fn reports_unknown_types_in_signatures_and_consts() {
    let root = temp_project("unknown-type", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nconst X: Nope = 1\nfn f(a: Booook): Alsobad\n    return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let found = with_code(&report, "check.unknown_type");
    assert_eq!(found.len(), 3, "{:#?}", report.diagnostics);
    assert!(
        found.iter().any(|d| d.message.contains("Booook")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("Alsobad")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("Nope")),
        "{found:#?}"
    );
}

#[test]
fn known_types_are_not_flagged_as_unknown() {
    let root = temp_project("known-types", |root| {
        // Primitive, sequence, identity, the module's own resource, `unknown`, and
        // a qualified cross-module reference are all accepted.
        write(
            root,
            "src/m.mw",
            "module m\nresource Book at ^books(id: int)\n    required title: string\n\nfn f(a: int, b: sequence[string], c: Book::Id, d: Book, e: unknown, g: shelf::Thing): bool\n    return true\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        with_code(&report, "check.unknown_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn reports_a_bare_return_in_a_value_returning_function() {
    let root = temp_project("bare-return", |root| {
        // The bare `return` (inside the `if`) leaves a value-returning function
        // without a value on that path.
        write(
            root,
            "src/m.mw",
            "module m\nfn f(c: bool): int\n    if c\n        return\n    return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert_eq!(
        with_code(&report, "check.return_value").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn reports_a_value_return_in_a_void_function() {
    let root = temp_project("void-return", |root| {
        write(root, "src/m.mw", "module m\nfn g()\n    return 1\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert_eq!(
        with_code(&report, "check.return_value").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn matching_returns_are_not_flagged() {
    let root = temp_project("ok-return", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nfn ok(c: bool): int\n    if c\n        return 1\n    return 2\n\nfn void_fn(c: bool)\n    if c\n        return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        with_code(&report, "check.return_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn reports_a_value_function_that_may_not_return() {
    let root = temp_project("missing-return", |root| {
        // `f` falls through the `if` (no else) without returning; `g` ends in an
        // assignment.
        write(
            root,
            "src/m.mw",
            "module m\nfn f(c: bool): int\n    if c\n        return 1\n\nfn g(): int\n    var x = 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert_eq!(
        with_code(&report, "check.missing_return").len(),
        2,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn functions_that_return_on_all_paths_are_not_flagged() {
    let root = temp_project("returns-all-paths", |root| {
        // Exhaustive if/else; ends in return; void; ends in a call; ends in a loop.
        write(
            root,
            "src/m.mw",
            "module m\n\
             fn a(c: bool): int\n    if c\n        return 1\n    else\n        return 2\n\n\
             fn b(): int\n    return 7\n\n\
             fn c()\n    var x = 1\n\n\
             fn d(): int\n    helper()\n\n\
             fn e(c: bool): int\n    while c\n        return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        with_code(&report, "check.missing_return").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn rejects_arithmetic_on_mismatched_operand_types() {
    // `+` needs matching numeric operands; `1 + true` adds an int and a bool.
    let found = check_script(
        "op-arith",
        "fn f()\n    var x = 1 + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_concatenation_of_non_strings() {
    // `_` concatenates strings; `1 _ 2` joins two ints.
    let found = check_script(
        "op-concat",
        "fn f()\n    var x = 1 _ 2\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_logical_operator_on_a_non_bool() {
    // `and` needs bool operands; `true and 1` mixes in an int.
    let found = check_script(
        "op-logical",
        "fn f()\n    var x = true and 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_comparison_of_different_types() {
    // Ordering compares same-typed values; `1 < "a"` mixes int and string.
    let found = check_script(
        "op-compare",
        "fn f()\n    var x = 1 < \"a\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_unary_operator_on_the_wrong_type() {
    // `not` needs a bool operand; `not 1` negates an int.
    let found = check_script(
        "op-unary",
        "fn f()\n    var x = not 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn infers_parameter_types_for_operator_checks() {
    // `b` is declared `bool`, so `b + 1` adds a bool to an int.
    let found = check_script(
        "op-param",
        "fn f(b: bool): int\n    return b + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn well_typed_operators_are_not_flagged() {
    // Every operator here has correctly typed operands.
    let found = check_script(
        "op-ok",
        "fn ok(a: int, b: int, s: string, t: string, p: bool, q: bool): bool\n\
         \x20   const sum = a + b\n\
         \x20   const quot = a / b\n\
         \x20   const cat = s _ t\n\
         \x20   const cmp = a < b\n\
         \x20   const ne = a != b\n\
         \x20   const both = p and q\n\
         \x20   const neg = -a\n\
         \x20   const inv = not p\n\
         \x20   return both\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn operators_on_unknown_operands_are_not_flagged() {
    // `mystery()` calls an unresolved function, so its result type is unknown; the
    // checker only flags an operator when both operand types are known to be
    // incompatible. (A bare name would itself be a `check.unresolved_name` error,
    // so a call is used here to isolate the operator behavior.)
    let found = check_script(
        "op-unknown",
        "fn f()\n    var x = mystery() + 1\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_bare_undefined_name_is_flagged() {
    // Strict typing: `mystery` is not a parameter, local, loop binding, catch
    // binding, or module constant, so it is genuinely undefined.
    let found = check_script(
        "name-undefined",
        "fn f()\n    var x = mystery\n",
        "check.unresolved_name",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_defined_name_is_not_flagged() {
    // A parameter is in scope, so referencing it is not an unresolved name.
    let found = check_script(
        "name-defined",
        "fn f(a: int)\n    var x = a\n",
        "check.unresolved_name",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unresolved_call_is_not_flagged_as_a_name() {
    // A bare name in callee position names a function, not a value. An unresolved
    // function call is a separate concern, so it is not a `check.unresolved_name`.
    let found = check_script(
        "name-callee",
        "fn f()\n    var x = mystery()\n",
        "check.unresolved_name",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_assignment_to_an_undeclared_name_is_flagged() {
    // Assigning to a name that was never declared targets an unresolved name. The
    // runtime faults the same way (`run.unbound_name`), so the checker catches it
    // earlier rather than weaker than its own runtime.
    let found = check_script(
        "name-assign-undeclared",
        "fn f()\n    x = 1\n",
        "check.unresolved_name",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_mixing_int_and_decimal_arithmetic() {
    // Numeric operands must match exactly; there is no implicit int-to-decimal
    // promotion, so `1.0 + 1` is an error.
    let found = check_script(
        "op-promote",
        "fn f()\n    var x = 1.0 + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_nested_operator_error_is_reported_once() {
    // `1 + true` is the error; the outer `+ 2` sees an unknown left operand (the
    // flagged subexpression) and does not fire a second diagnostic.
    let found = check_script(
        "op-nested",
        "fn f()\n    var x = 1 + true + 2\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_non_bool_if_condition() {
    // `if 1` tests an int where a bool is required.
    let found = check_script(
        "cond-if",
        "fn f()\n    if 1\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_non_bool_while_condition() {
    // `while "go"` tests a string where a bool is required.
    let found = check_script(
        "cond-while",
        "fn f()\n    while \"go\"\n        break\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_non_bool_else_if_condition() {
    // The `else if 2` clause tests an int condition.
    let found = check_script(
        "cond-elseif",
        "fn f(c: bool)\n    if c\n        return\n    else if 2\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn bool_conditions_are_not_flagged() {
    // A bool binding and a comparison both yield bool conditions.
    let found = check_script(
        "cond-ok",
        "fn f(a: int, b: int, c: bool)\n    if a < b\n        return\n    while c\n        break\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unresolved_condition_is_flagged() {
    // Strict typing: `mystery` is unbound (unknown type), so the condition cannot
    // be shown to be `bool` — a `check.untyped_value` error (not a
    // `check.condition_type` non-bool mismatch).
    let found = check_script(
        "cond-unknown",
        "fn f()\n    if mystery\n        return\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    let non_bool = check_script(
        "cond-unknown",
        "fn f()\n    if mystery\n        return\n",
        "check.condition_type",
    );
    assert!(non_bool.is_empty(), "{non_bool:#?}");
}

#[test]
fn an_exists_condition_is_not_flagged() {
    // `exists(...)` resolves to `bool`, so a presence-check condition is clean.
    let found = check_module(
        "cond-exists",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    if exists(^books(1))\n        return\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_a_call_with_the_wrong_argument_count() {
    // `add` takes two parameters; `add(1)` and `add(1, 2, 3)` are both arity errors.
    let found = check_module(
        "call-arity",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(1)\n    var y = add(1, 2, 3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn rejects_a_named_argument_that_is_not_a_parameter() {
    // `add` has no parameter `c`.
    let found = check_module(
        "call-named",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(a: 1, c: 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn correct_calls_are_not_flagged() {
    // Positional and named calls that match the signature are accepted.
    let found = check_module(
        "call-ok",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(1, 2)\n    var y = add(a: 5, b: 6)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_builtin_call_is_not_arity_checked_and_an_unknown_call_is_not_a_mismatch() {
    // `print` is a builtin (dispatched before user functions) and `mystery` does
    // not resolve to a declared function; neither is an arity/argument mismatch.
    let found = check_module(
        "call-skip",
        "module m\n\
         fn caller()\n    print(1, 2, 3)\n    var x = mystery(1, 2)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_call_to_an_undefined_function_is_flagged() {
    // Strict typing, runtime parity (run.unknown_function): a call to a name that
    // is neither a builtin nor a declared function is an unresolved call.
    let found = check_module(
        "call-unknown",
        "module m\n\
         fn caller()\n    mystery(1, 2)\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_call_to_an_unknown_std_submodule_is_flagged() {
    // `std::bogus::foo()` names no real std module (STD_MODULES), so it is not a
    // builtin — it is reported consistently with `use std::bogus` rejection,
    // rather than silently type-checking.
    let found = check_module(
        "call-std-bogus",
        "module m\n\
         fn caller()\n    std::bogus::foo()\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_call_to_a_known_std_submodule_is_not_flagged() {
    // A real std submodule call stays a builtin and is not unresolved.
    let found = check_module(
        "call-std-known",
        "module m\n\
         fn caller()\n    var n = std::text::length(\"hi\")\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_builtin_call_is_not_an_unresolved_call() {
    // Builtins dispatch before user functions, so they never resolve to a program
    // function — but they are defined, not unresolved.
    let found = check_module(
        "call-builtin",
        "module m\n\
         fn caller()\n    print(1, 2, 3)\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_call_to_a_defined_function_is_not_an_unresolved_call() {
    let found = check_module(
        "call-defined",
        "module m\n\
         fn helper(): int\n    return 1\n\n\
         fn caller()\n    var x = helper()\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_resource_constructor_is_not_an_unresolved_call() {
    // `Book(...)` constructs a resource value; it is a known
    // declared resource, not an undefined function.
    let found = check_module(
        "ctor-resource",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn caller()\n    var b = Book(title: \"a\")\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_constructor_is_not_an_unresolved_call() {
    // `Book::Id(1)` constructs a resource identity; it is a
    // known declared resource's identity, not an undefined function.
    let found = check_module(
        "ctor-identity",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn caller()\n    const id = Book::Id(1)\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_call_in_a_module_less_script_is_not_flagged() {
    // A module-less script's functions are not in the program (not runnable as a
    // call target), so its calls are not resolution-checked — only library-module
    // calls are.
    let found = check_script(
        "call-script",
        "fn f()\n    mystery()\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn unresolved_calls_are_suppressed_when_a_module_fails_to_parse() {
    // Module `a` has a lexical error (a leading tab), so it is excluded from the
    // program; a call to `a::helper` in clean module `b` must not be reported as
    // unresolved — the definition exists, the project just did not fully parse.
    let root = temp_project("call-incomplete", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\tpub fn helper()\n    return\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\nfn caller()\n    a::helper()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn rejects_a_wrong_argument_count_in_a_qualified_cross_module_call() {
    // `a::helper` takes one parameter; the qualified call in module `b` passes two.
    let root = temp_project("call-qualified", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub fn helper(x: int)\n    return\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\nfn caller()\n    a::helper(1, 2)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert_eq!(
        with_code(&report, "check.call_argument").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn rejects_a_positional_argument_of_the_wrong_type() {
    // `add` expects two ints; `true` is a bool.
    let found = check_module(
        "call-argtype",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(true, 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_named_argument_of_the_wrong_type() {
    // The named `a: true` passes a bool where `a` is an int.
    let found = check_module(
        "call-named-argtype",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(a: true, b: 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_nested_group_field_read_resolves_its_type() {
    // A read through nested group layers resolves to the innermost field's type,
    // so a typed return of it is not flagged as an untyped value.
    let found = check_module(
        "nested-read",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    \
         versions(version: int)\n        required title: string\n        \
         comments(pos: int)\n            required text: string\n\n\
         fn f(): string\n    return ^books(1).versions(2).comments(3).text\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_nested_group_field_read_of_the_wrong_type_is_flagged() {
    // The nested read resolves to `string`, so storing it into an `int` is a
    // genuine type mismatch — proving the type is resolved, not left unknown.
    let found = check_module(
        "nested-read-mismatch",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    \
         versions(version: int)\n        required title: string\n        \
         comments(pos: int)\n            required text: string\n\n\
         fn f()\n    const n: int = ^books(1).versions(2).comments(3).text\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unresolved_argument_into_a_typed_parameter_is_flagged() {
    // Strict typing: `mystery` is unbound (unknown type), but `add`'s parameter is
    // `int`, so the argument is a `check.untyped_value` error — convert it first.
    // It is not a `check.call_argument` mismatch.
    let found = check_module(
        "call-argtype-unknown",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(mystery, 2)\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    let mismatch = check_module(
        "call-argtype-unknown",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(mystery, 2)\n",
        "check.call_argument",
    );
    assert!(mismatch.is_empty(), "{mismatch:#?}");
}

#[test]
fn a_call_return_type_feeds_further_type_checks() {
    // `makeInt()` is typed `int`, so `makeInt() + true` is an int-plus-bool error.
    let found = check_module(
        "call-return-type",
        "module m\n\
         fn makeInt(): int\n    return 1\n\n\
         fn caller()\n    var x = makeInt() + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_field_read_feeds_the_return_type_check() {
    // `^books(1).title` is `string` from the schema, but `f` returns `int`.
    let found = check_module(
        "saved-field-return",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f(): int\n    return ^books(1).title\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_field_read_feeds_operator_checks() {
    // `currentVersion` is `int` from the schema, so `+ true` is int-plus-bool.
    let found = check_module(
        "saved-field-op",
        "module m\n\
         resource Book at ^books(id: int)\n    currentVersion: int\n\n\
         fn f()\n    var x = ^books(1).currentVersion + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_saved_field_read_is_not_flagged() {
    // `^books(1).title` is `string`, matching `f`'s declared `string` return.
    let found = check_module(
        "saved-field-ok",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f(): string\n    return ^books(1).title\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_local_resource_field_read_feeds_operator_checks() {
    // `book.title` is `string` from Book's schema, so `+ 1` is string-plus-int.
    let found = check_module(
        "local-field-op",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    var book: Book\n    var x = book.title + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_local_resource_field_is_not_flagged() {
    // `book.title` is `string`, matching `f`'s declared `string` return.
    let found = check_module(
        "local-field-ok",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f(): string\n    var book: Book\n    return book.title\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn passing_a_resource_to_a_mismatched_resource_parameter_is_not_flagged() {
    // Resources are not primitives, so a resource-typed argument is never flagged
    // against a different resource-typed parameter — resource-name resolution must
    // not turn this sound omission into a false positive.
    let found = check_module(
        "resource-arg",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         resource Shelf at ^shelves(id: int)\n    name: string\n\n\
         fn useShelf(s: Shelf): bool\n    return true\n\n\
         fn f()\n    var book: Book\n    var ok = useShelf(book)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_whole_resource_read_into_a_local_types_its_fields() {
    // `^books(1)` reads the whole record as a `Book`; `b.title` then resolves to
    // `string` from the schema, so `+ 1` is string-plus-int.
    let found = check_module(
        "whole-read-field",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    var b = ^books(1)\n    var x = b.title + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_constant_is_in_scope_and_typed() {
    // A top-level `const` is in scope (bare) for the module's functions and carries
    // its annotated type, so `M` is `int` and storing it into a `string` mismatches.
    let found = check_module(
        "module-const",
        "module m\nconst M: int = 5\n\nfn f()\n    var x: string = M\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_constant_reference_is_not_unresolved() {
    // The bare constant reference resolves (it is in scope), so it is not flagged
    // as an untyped value when stored into a matching place.
    let found = check_module(
        "module-const-ok",
        "module m\nconst M: int = 5\n\nfn f()\n    var x: int = M\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_for_binding_over_a_sequence_types_the_element() {
    // `std::text::split` yields `sequence[string]`, so `part` is `string` and
    // `part + 1` is string-plus-int.
    let found = check_module(
        "for-elem",
        "module m\nfn f(s: string)\n    for part in std::text::split(s, \",\")\n        var x = part + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn exists_and_append_builtin_return_types_feed_checks() {
    // `exists` returns `bool` and `append` returns `int`; using them in mismatched
    // operators is caught.
    let found = check_module(
        "builtin-returns",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n    tags(pos: int): string\n\n\
         fn f()\n    var a = exists(^books(1)) + 1\n    var b = append(^books(1).tags, \"t\") and true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn get_builtin_returns_the_default_type() {
    // `get(path, default)` returns the leaf-or-default type; with a string default
    // it is `string`, so `+ 1` is string-plus-int.
    let found = check_module(
        "get-return",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    var x = get(^books(1).title, \"none\") + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_std_call_return_type_feeds_operator_checks() {
    // `std::text::length` returns `int`, so `+ true` is int-plus-bool.
    let found = check_module(
        "std-return-op",
        "module m\nfn f()\n    var x = std::text::length(\"hi\") + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_std_call_return_type_feeds_the_return_type_check() {
    // `std::clock::now()` is `instant`, but `f` returns `int`.
    let found = check_module(
        "std-return-mismatch",
        "module m\nfn f(): int\n    return std::clock::now()\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_std_call_return_is_not_flagged() {
    // `std::text::length` returns `int`, matching `f`'s declared `int` return.
    let found = check_module(
        "std-return-ok",
        "module m\nfn f(): int\n    return std::text::length(\"hi\")\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_sequence_returning_std_call_is_not_flagged() {
    // `std::text::split` returns `sequence[string]` (non-primitive), so the checks
    // — which gate on primitives — never flag it, even against an `int` return.
    let found = check_module(
        "std-return-seq",
        "module m\nfn f(): int\n    return std::text::split(\"a,b\", \",\")\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_conversion_call_return_type_feeds_operator_checks() {
    // `int(raw)` returns `int`, so `+ true` is int-plus-bool.
    let found = check_module(
        "conv-return-op",
        "module m\nfn f(raw: unknown)\n    var x = int(raw) + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_conversion_into_a_mismatched_annotated_place_is_flagged() {
    // `int(raw)` is `int`, but the place is `string`.
    let found = check_module(
        "conv-assign-bad",
        "module m\nfn f(raw: unknown)\n    const s: string = int(raw)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_conversion_into_a_matching_annotated_place_is_not_flagged() {
    // `int(raw)` is `int`, matching the declared `int` place — the documented
    // `const n: int = int(raw)` pattern checks clean.
    let found = check_module(
        "conv-assign-ok",
        "module m\nfn f(raw: unknown)\n    const n: int = int(raw)\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_error_code_conversion_into_an_error_code_place_is_not_flagged() {
    // `ErrorCode(raw)` is `ErrorCode`, matching the declared `ErrorCode` place —
    // the documented `const code: ErrorCode = ErrorCode(raw)` conversion checks
    // clean (no false `check.untyped_value`).
    let found = check_module(
        "conv-error-code",
        "module m\nfn f(raw: unknown)\n    const code: ErrorCode = ErrorCode(raw)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_group_field_read_feeds_type_checks() {
    // `^books(1).versions(2).title` is `string` from the group schema, but `f`
    // returns `int`.
    let found = check_module(
        "saved-group-field",
        "module m\n\
         resource Book at ^books(id: int)\n    versions(v: int)\n        title: string\n\n\
         fn f(): int\n    return ^books(1).versions(2).title\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_singleton_field_read_feeds_type_checks() {
    // `^settings.theme` on a keyless singleton resource (`Settings at ^settings`)
    // is `string` from the schema, not Unknown — so a typed use never
    // false-positives check.untyped_value, and a real mismatch (returning it
    // from an `int` function) is caught.
    let found = check_module(
        "singleton-field",
        "module m\n\
         resource Settings at ^settings\n    theme: string\n\n\
         fn f(): int\n    return ^settings.theme\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_singleton_field_read_in_a_typed_place_is_not_an_untyped_value() {
    // The documented `const t: string = ^settings.theme` reads a singleton field
    // into a matching place — no false check.untyped_value.
    let found = check_module(
        "singleton-field-ok",
        "module m\n\
         resource Settings at ^settings\n    theme: string\n\n\
         fn f()\n    const t: string = ^settings.theme\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_singleton_whole_read_and_write_are_not_flagged() {
    // `var s: Settings = ^settings` and `^settings = s` address the keyless
    // root directly; neither should raise a false unresolved or type diagnostic.
    let report = check_module_report(
        "singleton-whole",
        "module m\n\
         resource Settings at ^settings\n    theme: string\n    required maxLoans: int\n\n\
         fn snapshot(): Settings\n    return ^settings\n\n\
         fn restore(s: Settings)\n    ^settings = s\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn an_unkeyed_group_field_read_feeds_type_checks() {
    // `^patients(1).name.first` reaches a scalar field through an unkeyed group
    // (`name { first; last }`). It is `string` from the schema, not Unknown, so a
    // typed mismatch (returning it from an `int` function) is caught.
    let found = check_module(
        "unkeyed-group-field",
        "module m\n\
         resource Patient at ^patients(id: int)\n\
         \x20   name\n        first: string\n        last: string\n\n\
         fn f(): int\n    return ^patients(1).name.first\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_unkeyed_group_field_read_is_not_flagged() {
    let found = check_module(
        "unkeyed-group-field-ok",
        "module m\n\
         resource Patient at ^patients(id: int)\n\
         \x20   name\n        first: string\n        last: string\n\n\
         fn f(): string\n    return ^patients(1).name.first\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_keyed_leaf_read_feeds_type_checks() {
    // `^books(1).tags(2)` is `string` (the layer's leaf type), but `f` returns `int`.
    let found = check_module(
        "saved-leaf",
        "module m\n\
         resource Book at ^books(id: int)\n    tags(pos: int): string\n\n\
         fn f(): int\n    return ^books(1).tags(2)\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn correctly_typed_group_and_leaf_reads_are_not_flagged() {
    // The group field and the keyed leaf both match their declared `string` use.
    let found = check_module(
        "saved-layer-ok",
        "module m\n\
         resource Book at ^books(id: int)\n\
         \x20   tags(pos: int): string\n\
         \x20   versions(v: int)\n        title: string\n\n\
         fn title(): string\n    return ^books(1).versions(2).title\n\n\
         fn tag(): string\n    return ^books(1).tags(2)\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unannotated_module_const_is_inferred_and_a_matching_use_is_not_flagged() {
    // `const M = 5` has an inferable `int` type; using it in `var x: int = M`
    // must not false-positive check.untyped_value.
    let found = check_module(
        "module-const-ok",
        "module m\nconst M = 5\nfn f()\n    var x: int = M\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unannotated_module_const_mismatch_is_caught() {
    // `const M = 5` is `int`; storing it into a `string` place is a real mismatch
    // that was previously missed because the const typed to Unknown.
    let found = check_module(
        "module-const-mismatch",
        "module m\nconst M = 5\nfn f()\n    var x: string = M\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_over_range_int_literal_is_flagged_at_check_time() {
    // `99999999999999999999999999` exceeds i64; the runtime would reject it as
    // run.overflow, so the checker flags it too.
    let found = check_script(
        "int-literal-overflow",
        "fn f()\n    const x: int = 99999999999999999999999999\n",
        "check.literal_range",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_in_range_int_literal_is_not_flagged() {
    // i64::MAX checks clean.
    let found = check_script(
        "int-literal-max",
        "fn f()\n    const x: int = 9223372036854775807\n",
        "check.literal_range",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_over_envelope_decimal_literal_is_flagged_at_check_time() {
    // 35 significant digits exceeds the 34-digit decimal envelope.
    let found = check_script(
        "decimal-literal-overflow",
        "fn f()\n    const d: decimal = 1.2345678901234567890123456789012345\n",
        "check.literal_range",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_in_range_decimal_literal_is_not_flagged() {
    // 34 significant digits is exactly at the envelope, and a long trailing-zero
    // fraction normalizes back into range — neither is flagged.
    let found = check_script(
        "decimal-literal-ok",
        "fn f()\n\
         \x20   const d: decimal = 1.234567890123456789012345678901234\n\
         \x20   const z: decimal = 0.000000000000000000000000000000000000\n",
        "check.literal_range",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_over_range_module_const_literal_is_flagged_at_check_time() {
    // A module-level `const` initializer is range-checked like a local one. The
    // diagnostic previously fired only during scope-building type inference, whose
    // diagnostics are discarded, so an out-of-range module constant slipped past
    // `marrow check` and was caught only at runtime.
    let found = check_module(
        "module-const-literal-overflow",
        "module m\nconst BIG: int = 99999999999999999999999999\n",
        "check.literal_range",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_var_initializer_of_the_wrong_type() {
    // `x` is declared `int` but initialized with a string.
    let found = check_script(
        "init-var",
        "fn f()\n    var x: int = \"hi\"\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_const_initializer_of_the_wrong_type() {
    // `x` is declared `bool` but initialized with an int.
    let found = check_script(
        "init-const",
        "fn f()\n    const x: bool = 1\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_an_assignment_to_a_local_of_the_wrong_type() {
    // `x` is an int local; assigning a string is a mismatch.
    let found = check_script(
        "assign-local",
        "fn f()\n    var x: int = 1\n    x = \"hi\"\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_saved_field_write_of_the_wrong_type() {
    // `currentVersion` is `int`, so writing a string is a mismatch.
    let found = check_module(
        "assign-saved",
        "module m\n\
         resource Book at ^books(id: int)\n    currentVersion: int\n\n\
         fn f()\n    ^books(1).currentVersion = \"hi\"\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn well_typed_assignments_and_initializers_are_not_flagged() {
    // Each binding and assignment matches the declared/known type.
    let found = check_script(
        "assign-ok",
        "fn f()\n    var x: int = 1\n    x = 2\n    const s: string = \"a\"\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_value_into_a_typed_place_is_flagged() {
    // Strict typing: `mystery()` does not resolve, so storing it into the concrete
    // `int` place is a `check.untyped_value` error — convert or define it. (It is
    // not a `check.assignment_type` mismatch.)
    let found = check_script(
        "assign-unknown",
        "fn f()\n    var x: int = 1\n    x = mystery()\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    // The same assignment is not reported as a primitive mismatch.
    let mismatch = check_script(
        "assign-unknown",
        "fn f()\n    var x: int = 1\n    x = mystery()\n",
        "check.assignment_type",
    );
    assert!(mismatch.is_empty(), "{mismatch:#?}");
}

#[test]
fn a_typed_initializer_with_an_unresolved_value_is_flagged() {
    // A typed `const` initializer whose value has no known type is flagged.
    let found = check_script(
        "init-unknown",
        "fn f()\n    const n: int = mystery()\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_an_identity_place_is_not_flagged() {
    // The place is an identity, not a primitive, so strict untyped-value checking
    // does not apply — this guards the `const id: Book::Id = nextId(^books)` shape.
    let found = check_module(
        "untyped-identity",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    const id: Book::Id = nextId(^books)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_value_into_an_unknown_place_is_not_flagged() {
    // `unknown` is the explicit dynamic opt-out: storing an unresolved value into
    // an `unknown`-typed place is allowed.
    let found = check_script(
        "untyped-into-unknown",
        "fn f()\n    var raw: unknown = mystery()\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_a_return_of_the_wrong_type() {
    // The function is declared to return `int`, but `true` is a bool.
    let found = check_script(
        "ret-type",
        "fn f(): int\n    return true\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_returned_local_of_the_wrong_type() {
    // `s` is inferred `string` from its initializer, but `f` returns `int`.
    let found = check_script(
        "ret-local",
        "fn f(): int\n    const s = \"hi\"\n    return s\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn correct_returns_are_not_flagged() {
    // Each returned value matches the function's declared return type.
    let found = check_script(
        "ret-ok",
        "fn f(): int\n    return 1\n\nfn g(b: bool): bool\n    return b\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_return_of_an_unresolved_value_into_a_typed_return_is_flagged() {
    // Strict typing: `mystery()` has no known type, but `f` returns `int`, so the
    // return is a `check.untyped_value` error (not a `check.return_type` mismatch).
    let found = check_script(
        "ret-unknown",
        "fn f(): int\n    return mystery()\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    let mismatch = check_script(
        "ret-unknown",
        "fn f(): int\n    return mystery()\n",
        "check.return_type",
    );
    assert!(mismatch.is_empty(), "{mismatch:#?}");
}

#[test]
fn a_return_of_an_unresolved_value_into_an_identity_return_is_not_flagged() {
    // A non-primitive return type (an identity) is excluded from strict
    // untyped-value checking — guards the sample's `return nextId(...)`-style code.
    let found = check_module(
        "ret-identity",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f(): Book::Id\n    return nextId(^books)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_unique_index_lookup_types_as_the_resource_identity() {
    // `^books.byIsbn(isbn)` reads back the owning identity, so it types as
    // `Book::Id` — not `Unknown`. Returned where `int` is expected, that is a
    // typed value (a non-primitive identity), so strict untyped-value checking
    // does not fire.
    let found = check_module(
        "unique-index-identity",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\n\
         fn f(isbn: string): int\n    return ^books.byIsbn(isbn)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn std_log_error_of_an_error_constructor_checks_clean() {
    // std::log::error takes an Error; the Error(...) constructor must type AS Error
    // (not Unknown), so the canonical log::error(Error(...)) is not a false
    // check.untyped_value / check.call_argument.
    let src =
        "module m\nuse std::log\nfn f()\n    log::error(Error(code: \"x.y\", message: \"m\"))\n";
    assert!(
        check_module("std-log-error-untyped", src, "check.untyped_value").is_empty(),
        "Error(...) must type as Error, not Unknown"
    );
    assert!(
        check_module("std-log-error-arg", src, "check.call_argument").is_empty(),
        "log::error(Error(...)) is the spec-canonical call"
    );
}

fn with_code<'a>(
    report: &'a marrow_check::CheckReport,
    code: &str,
) -> Vec<&'a marrow_check::CheckDiagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .collect()
}

/// Check a single script `src` and return its diagnostics with `code`.
fn check_script(name: &str, src: &str, code: &str) -> Vec<marrow_check::CheckDiagnostic> {
    let root = temp_project(name, |root| write(root, "src/app.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    with_code(&report, code).into_iter().cloned().collect()
}

/// Check a single library module `src` (declaring `module m`, placed at the
/// matching path `src/m.mw`) and return its diagnostics with `code`. Unlike
/// [`check_script`], the file declares a module, so its functions are part of the
/// checked program and resolve as call targets.
fn check_module(name: &str, src: &str, code: &str) -> Vec<marrow_check::CheckDiagnostic> {
    let root = temp_project(name, |root| write(root, "src/m.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    with_code(&report, code).into_iter().cloned().collect()
}

/// Check a single library module and return its whole report, for tests that
/// assert a program is clean rather than filtering for one code.
fn check_module_report(name: &str, src: &str) -> marrow_check::CheckReport {
    let root = temp_project(name, |root| write(root, "src/m.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    report
}

/// Check a project whose `src/app.mw` library declares `app_src` and whose
/// `tests/app_test.mw` test script holds `test_src`, returning the test report.
/// Used by tests that assert what `marrow test`/check catches in test files.
fn check_tests_report(name: &str, app_src: &str, test_src: &str) -> marrow_check::CheckReport {
    let root = temp_project(name, |root| {
        write(root, "src/app.mw", app_src);
        write(root, "tests/app_test.mw", test_src);
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (src_report, src_program) = check_project(&root, &cfg).expect("check src");
    assert!(!src_report.has_errors(), "{:#?}", src_report.diagnostics);
    let (test_report, _modules) = check_tests(&root, &cfg, &src_program).expect("check tests");
    fs::remove_dir_all(&root).ok();
    test_report
}

#[test]
fn check_tests_catches_a_std_call_with_the_wrong_argument_type() {
    // `std::text::length` takes a `string`; passing `42` is the same
    // `check.call_argument` mismatch a library file would report — test files run
    // the full type-inference pass, so this is caught at check time, not only at
    // run time.
    let report = check_tests_report(
        "check-tests-std-arg",
        "module app\n",
        "pub fn t()\n    var n = std::text::length(42)\n",
    );
    assert_eq!(
        with_code(&report, "check.call_argument").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn check_tests_catches_a_nextid_misuse_on_a_composite_root() {
    // `^orders` has a composite identity, so it has no default `nextId` policy; a
    // test file calling `nextId(^orders)` gets the `check.next_id_requires_single_int`
    // gate the library files already enforce.
    let report = check_tests_report(
        "check-tests-nextid",
        "module app\n\
         resource Order at ^orders(region: string, id: int)\n    required total: int\n",
        "pub fn t()\n    var id = nextId(^orders)\n",
    );
    assert_eq!(
        with_code(&report, "check.next_id_requires_single_int").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn check_tests_catches_a_type_mismatched_assignment() {
    // A test file's ordinary type errors are reported too: storing an `int` const
    // into a `string` place is a `check.assignment_type` mismatch.
    let report = check_tests_report(
        "check-tests-assign",
        "module app\n",
        "pub fn t()\n    const s: string = 1\n",
    );
    assert_eq!(
        with_code(&report, "check.assignment_type").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn check_tests_leaves_a_clean_test_file_clean() {
    // A well-typed test file that calls a project function and a std helper checks
    // with no diagnostics — the new type pass must not false-positive.
    let report = check_tests_report(
        "check-tests-clean",
        "module app\n\npub fn add(): int\n    return 1\n",
        "pub fn t()\n    std::assert::isTrue(app::add() = 1)\n    var n = std::text::length(\"hi\")\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn finally_return_is_rejected() {
    let found = check_script(
        "fin-return",
        "fn f()\n    try\n        x = 1\n    finally\n        return\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].line, 5, "{:#?}", found[0]);
}

#[test]
fn finally_break_inside_nested_loop_is_allowed() {
    let found = check_script(
        "fin-break-loop",
        "fn f()\n    try\n        x = 1\n    finally\n        while c\n            break\n",
        "check.finally_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn finally_unlabeled_break_that_escapes_is_rejected() {
    let found = check_script(
        "fin-break-escape",
        "fn f()\n    try\n        x = 1\n    finally\n        break\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn finally_labeled_break_to_outer_loop_is_rejected() {
    // The label names a loop outside the finally block, so the break escapes it.
    let found = check_script(
        "fin-break-label",
        "fn f()\n    outer: while a\n        try\n            x = 1\n        finally\n            break outer\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn finally_labeled_break_to_inner_loop_is_allowed() {
    // The label names a loop nested within the finally block.
    let found = check_script(
        "fin-break-inner-label",
        "fn f()\n    try\n        x = 1\n    finally\n        inner: while c\n            break inner\n",
        "check.finally_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn break_outside_any_loop_is_rejected() {
    // A `break` with no enclosing loop only fails late at runtime
    // (RUN_NO_ENCLOSING_LOOP); the checker must reject it statically.
    let found = check_script(
        "break-no-loop",
        "fn f()\n    break\n",
        "check.loop_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].line, 2, "{:#?}", found[0]);
}

#[test]
fn continue_outside_any_loop_is_rejected() {
    let found = check_script(
        "continue-no-loop",
        "fn f()\n    continue\n",
        "check.loop_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn labeled_break_naming_no_enclosing_loop_is_rejected() {
    // The label names no enclosing loop, so the break can never resolve.
    let found = check_script(
        "break-bad-label",
        "fn f()\n    while c\n        break outer\n",
        "check.loop_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn break_and_continue_inside_a_loop_are_allowed() {
    let found = check_script(
        "break-in-loop",
        "fn f()\n    while c\n        break\n        continue\n",
        "check.loop_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn labeled_break_to_an_enclosing_loop_is_allowed() {
    let found = check_script(
        "break-good-label",
        "fn f()\n    outer: while a\n        while b\n            break outer\n",
        "check.loop_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn catch_with_non_error_type_is_rejected() {
    let found = check_script(
        "catch-bad-type",
        "fn f()\n    try\n        x = 1\n    catch e: string\n        return\n",
        "check.catch_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn catch_with_error_type_and_bare_catch_are_allowed() {
    let typed = check_script(
        "catch-error-type",
        "fn f()\n    try\n        x = 1\n    catch e: Error\n        return\n",
        "check.catch_type",
    );
    assert!(typed.is_empty(), "{typed:#?}");

    let bare = check_script(
        "catch-bare",
        "fn f()\n    try\n        x = 1\n    catch e\n        return\n",
        "check.catch_type",
    );
    assert!(bare.is_empty(), "{bare:#?}");
}

#[test]
fn call_shaped_assignment_target_is_rejected() {
    // `f(x) = y`: a call on a bare name is not a writable place.
    let found = check_script(
        "assign-call",
        "fn f()\n    f(x) = y\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn literal_assignment_target_is_rejected() {
    let found = check_script(
        "assign-literal",
        "fn f()\n    1 = y\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn saved_path_assignment_targets_are_allowed() {
    let found = check_script(
        "assign-saved",
        "fn f()\n    ^books(id).title = x\n",
        "check.invalid_assign_target",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn local_field_and_name_assignment_targets_are_allowed() {
    let found = check_script(
        "assign-local",
        "fn f()\n    x = 1\n    book.title = x\n",
        "check.invalid_assign_target",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn invalid_merge_target_is_rejected() {
    let found = check_script(
        "merge-bad",
        "fn f()\n    merge f(x) = y\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn constant_const_values_are_allowed() {
    // Literals, arithmetic over literals, a reference to another constant, a
    // unary operator, and a standard-library constant are all compile-time
    // constant expressions.
    let found = check_script(
        "const-ok",
        "const A = 1\nconst B = 2 + 3 * 4\nconst C = A\nconst N = -1\nconst P = std::math::PI\n",
        "check.non_constant_const",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn const_value_calling_a_function_is_rejected() {
    // A const cannot call a function or host module.
    let found = check_script(
        "const-call",
        "const X = compute()\n",
        "check.non_constant_const",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn const_value_reading_saved_data_is_rejected() {
    // A const cannot read saved data.
    let found = check_script(
        "const-saved",
        "const X = ^counter\n",
        "check.non_constant_const",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn const_value_with_a_nested_saved_read_is_rejected() {
    // The rule looks through operators: a saved-data read anywhere in the
    // expression makes the whole value non-constant.
    let found = check_script(
        "const-nested-saved",
        "const X = 1 + ^counter\n",
        "check.non_constant_const",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn deleting_the_root_a_loop_traverses_is_rejected() {
    // `for id in ^books` traverses the `^books` identity layer; `delete ^books(id)`
    // removes a key from that same layer, which the checker rejects.
    let found = check_module(
        "loop-delete-root",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in ^books\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].line, 7, "{:#?}", found[0]);
}

#[test]
fn appending_to_the_sequence_a_loop_traverses_is_rejected() {
    // `for pos in ^books(1).tags` traverses the `tags` layer; `append(...tags...)`
    // adds a key to that same layer.
    let found = check_module(
        "loop-append-seq",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for pos in ^books(1).tags\n        append(^books(1).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn writing_a_keyed_leaf_the_loop_traverses_is_rejected() {
    let found = check_module(
        "loop-write-leaf",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for pos in ^books(1).tags\n        ^books(1).tags(pos) = \"x\"\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn collecting_keys_first_then_mutating_is_allowed() {
    // The documented safe pattern: snapshot the keys into a local, iterate the
    // local, and mutate the layer. The loop traverses a local value, not the layer.
    let found = check_module(
        "loop-collect-first",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    const ids = keys(^books)\n    for id in ids\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn mutating_a_different_record_in_a_layer_loop_is_allowed() {
    // The loop traverses `^books(1).tags`; appending to `^books(2).tags` is a
    // different record's layer, so it is safe.
    let found = check_module(
        "loop-other-record",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for pos in ^books(1).tags\n        append(^books(2).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn writing_a_field_in_a_record_loop_is_allowed() {
    // `for id in ^books` traverses the identity layer; writing a scalar field of a
    // record does not change which keys the layer holds, so it is allowed.
    let found = check_module(
        "loop-field-write",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in ^books\n        ^books(id).title = \"x\"\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}
