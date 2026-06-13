mod support;

use marrow_check::{DiagnosticPayload, check_project, check_tests};
use marrow_project::parse_config;
use marrow_syntax::SourceSpan;

use support::{assert_clean, config, temp_project, with_code, write};

#[test]
fn clean_project_has_no_diagnostics() {
    let root = temp_project("clean", |root| {
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        // A module-less file is a script and is not bound to its path.
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn reports_module_path_mismatch() {
    let root = temp_project("mismatch", |root| {
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.module_path")
        .expect("module-path diagnostic");
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::ModulePath {
            declared: "shelf::other".into(),
            expected: Some("shelf::books".into()),
        }
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

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.module_path")
        .expect("module-path diagnostic");
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::ModulePath {
            declared: "config".into(),
            expected: Some("config.v2".into()),
        }
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

    let duplicates = with_code(&report, "check.duplicate_module");
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        duplicates[0].payload,
        DiagnosticPayload::DuplicateModule {
            name: "shared".into(),
            first_file: root.join("lib/shared.mw"),
        }
    );
}

#[test]
fn distinct_modules_are_not_flagged_as_duplicates() {
    let root = temp_project("distinct", |root| {
        write(root, "src/a.mw", "module a\n");
        write(root, "src/b.mw", "module b\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn a_script_file_is_not_bound_to_its_path() {
    let root = temp_project("script", |root| {
        // No module declaration: a script, even at a nested path.
        write(root, "src/tools/script.mw", "fn run()\n    return\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

fn duplicate_declarations(
    report: &marrow_check::CheckReport,
) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.duplicate_declaration")
}

fn source_line_span(source: &str, line: u32) -> SourceSpan {
    let start_byte = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum();
    let end_byte = source[start_byte..]
        .find('\n')
        .map_or(source.len(), |offset| start_byte + offset);
    SourceSpan {
        start_byte,
        end_byte,
        line,
        column: 1,
    }
}

fn assert_duplicate_declaration_payload(
    diagnostic: &marrow_check::CheckDiagnostic,
    name: &str,
    source: &str,
    first_line: u32,
) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::DuplicateDeclaration {
            name: name.into(),
            first_span: source_line_span(source, first_line),
        }
    );
}

#[test]
fn reports_duplicate_function_declaration() {
    let source = "module m\nfn run()\n    return\nfn run()\n    return\n";
    let root = temp_project("dup-fn", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_duplicate_declaration_payload(duplicates[0], "run", source, 2);
    // The later occurrence is reported.
    assert_eq!(duplicates[0].span.line, 4, "{:#?}", duplicates[0]);
}

#[test]
fn reports_duplicate_const_declaration() {
    let source = "module m\nconst A = 1\nconst A = 2\n";
    let root = temp_project("dup-const", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_duplicate_declaration_payload(duplicates[0], "A", source, 2);
}

#[test]
fn reports_duplicate_resource_declaration() {
    let source = "module m\nresource Book\n    title: string\nresource Book\n    title: string\n";
    let root = temp_project("dup-resource", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_duplicate_declaration_payload(duplicates[0], "Book", source, 2);
}

#[test]
fn reports_const_resource_name_collision() {
    let source = "module m\nconst Book = 1\nresource Book\n    title: string\n";
    let root = temp_project("const-resource", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_duplicate_declaration_payload(duplicates[0], "Book", source, 2);
}

#[test]
fn reports_import_short_name_collision_with_declaration() {
    let source = "module m\nuse shelf::books\nfn books()\n    return\n";
    let root = temp_project("use-collision", |root| {
        // `use shelf::books` contributes the short name `books`, which collides
        // with the declared function of the same name.
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_duplicate_declaration_payload(duplicates[0], "books", source, 2);
    // The function declaration is the later occurrence.
    assert_eq!(duplicates[0].span.line, 3, "{:#?}", duplicates[0]);
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
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

fn unresolved_imports(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.unresolved_import")
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

    let unresolved = unresolved_imports(&report);
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        unresolved[0].payload,
        DiagnosticPayload::UnresolvedImport("unknown::mod".into()),
        "{:#?}",
        unresolved[0]
    );
    assert_eq!(unresolved[0].span.line, 1, "{:#?}", unresolved[0]);
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
            "pub fn add_returns_one()\n    std::assert::isTrue(app::add() == 1)\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, test_modules) = check_tests(&root, &cfg, src_program).expect("check tests");

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
    let (test_report, _modules) = check_tests(&root, &cfg, src_program).expect("check tests");

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
            "module app\n\npub fn calls_app()\n    std::assert::isTrue(app::add() == 1)\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (_src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, test_modules) = check_tests(&root, &cfg, src_program).expect("check tests");

    assert!(!test_report.has_errors(), "{:#?}", test_report.diagnostics);
    assert_eq!(test_modules.len(), 1, "{test_modules:#?}");
    assert_eq!(test_modules[0].name, "tests::app_test");
}

#[test]
fn reports_an_enum_resource_name_collision() {
    // An enum and a resource share the module-level declaration namespace.
    let source = "module m\nenum Book\n    a\nresource Book\n    title: string\n";
    let root = temp_project("enum-resource-collision", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let duplicates = duplicate_declarations(&report);
    assert_eq!(duplicates.len(), 1, "{:#?}", report.diagnostics);
    assert_duplicate_declaration_payload(duplicates[0], "Book", source, 2);
}
