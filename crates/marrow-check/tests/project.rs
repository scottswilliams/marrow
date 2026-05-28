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
