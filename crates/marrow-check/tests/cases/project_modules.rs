use crate::support;
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
    let config =
        parse_config(r#"{ "sourceRoots": ["src", "lib"], "store": { "backend": "memory" } }"#)
            .expect("config");

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

fn builtin_collisions(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.builtin_collision")
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
fn single_builtin_name_declaration_is_a_builtin_collision_not_a_duplicate() {
    let source = "module m\npub fn count()\n    return\n";
    let root = temp_project("builtin-name-single", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = builtin_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(collisions[0].span.line, 2, "{:#?}", collisions[0]);
    assert_eq!(collisions[0].payload, DiagnosticPayload::None);
    assert_eq!(
        collisions[0].message,
        "`count` is a builtin name and cannot be used as a module-level declaration"
    );
    // A single declaration is never a redeclaration.
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.surface_collision").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn non_surface_builtin_name_collisions_keep_builtin_diagnostics_only() {
    let source = "module m\nfn exists()\n    return\nconst exists = 1\n";
    let root = temp_project("builtin-name-collision", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = builtin_collisions(&report);
    assert_eq!(collisions.len(), 2, "{:#?}", report.diagnostics);
    assert_eq!(
        collisions
            .iter()
            .map(|diagnostic| diagnostic.span.line)
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
    for diagnostic in collisions {
        assert_eq!(diagnostic.payload, DiagnosticPayload::None);
    }
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.surface_collision").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn import_after_builtin_declaration_does_not_add_duplicate_declaration() {
    let source = "module m\nfn exists()\n    return\nuse shelf::exists\n";
    let root = temp_project("builtin-name-late-import", |root| {
        write(root, "src/m.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = builtin_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(collisions[0].span.line, 2, "{:#?}", collisions[0]);
    assert_eq!(collisions[0].payload, DiagnosticPayload::None);
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.surface_collision").is_empty(),
        "{:#?}",
        report.diagnostics
    );
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
            "use missing::mod\nfn main()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let unresolved = unresolved_imports(&report);
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        unresolved[0].payload,
        DiagnosticPayload::UnresolvedImport("missing::mod".into()),
        "{:#?}",
        unresolved[0]
    );
    assert_eq!(unresolved[0].span.line, 1, "{:#?}", unresolved[0]);
}

#[test]
fn rejects_reserved_path_segment_in_test_module_name() {
    let root = temp_project("check-tests-reserved-segment", |root| {
        write(root, "src/app.mw", "module app\n");
        write(root, "tests/journal.mw", "pub fn smoke()\n    return\n");
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let (src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, test_modules) = check_tests(&root, &cfg, src_program).expect("check tests");

    assert!(!src_report.has_errors(), "{:#?}", src_report.diagnostics);
    let diagnostic = test_report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.module_path")
        .expect("module-path diagnostic");
    assert!(
        diagnostic.file.ends_with("journal.mw"),
        "{:?}",
        diagnostic.file
    );
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::ReservedTestModulePathSegment {
            module_name: "tests::journal".into(),
            reserved_segment: "journal".into(),
        }
    );
    assert!(test_modules.is_empty(), "{test_modules:#?}");
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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
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
fn a_test_file_module_must_match_its_path_derived_name() {
    let root = temp_project("test-module-mismatch", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn add(): int\n    return 1\n",
        );
        // A test file is named from its path (`tests::app_test`). A declared module
        // that does not match that name is rejected, mirroring the source rule, so
        // it cannot masquerade under another module's name.
        write(
            root,
            "tests/app_test.mw",
            "module app\n\npub fn calls_app()\n    std::assert::isTrue(app::add() == 1)\n",
        );
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let (_src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, _modules) = check_tests(&root, &cfg, src_program).expect("check tests");

    let diagnostic = test_report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.module_path")
        .expect("module-path diagnostic");
    assert!(
        diagnostic.file.ends_with("app_test.mw"),
        "{:?}",
        diagnostic.file
    );
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::ModulePath {
            declared: "app".into(),
            expected: Some("tests::app_test".into()),
        }
    );
    // The declaration span, not the start of file.
    assert_eq!(diagnostic.span.line, 1, "{diagnostic:#?}");
}

#[test]
fn a_test_file_with_no_declared_module_is_clean() {
    let root = temp_project("test-module-absent", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn add(): int\n    return 1\n",
        );
        write(
            root,
            "tests/app_test.mw",
            "pub fn calls_app()\n    std::assert::isTrue(app::add() == 1)\n",
        );
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let (_src_report, src_program) = check_project(&root, &cfg).expect("check src");
    let (test_report, test_modules) = check_tests(&root, &cfg, src_program).expect("check tests");

    assert!(!test_report.has_errors(), "{:#?}", test_report.diagnostics);
    assert_eq!(test_modules.len(), 1, "{test_modules:#?}");
    assert_eq!(test_modules[0].name, "tests::app_test");
}

#[test]
fn warns_when_a_public_function_exposes_a_private_enum() {
    // A non-`pub` enum named in a `pub fn` signature escapes its module through a
    // public API even though callers cannot name the type. The author gets a
    // warning, not a hard error, to add `pub` to the enum.
    let source = "module a\n\nenum Color\n    red\n    green\n\npub fn pick(): Color\n    return Color::red\n";
    let root = temp_project("exposed-private-enum-return", |root| {
        write(root, "src/a.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let exposed = with_code(&report, "check.exposed_private_enum");
    assert_eq!(exposed.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(exposed[0].severity, marrow_syntax::Severity::Warning);
    assert_eq!(
        exposed[0].payload,
        DiagnosticPayload::ExposedPrivateEnum {
            enum_name: "a::Color".into(),
            function: "pick".into(),
        }
    );
    // The cross-module `check.private_enum` error must not also fire: this is a
    // same-module signature, not a foreign reference.
    assert!(
        with_code(&report, "check.private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn warns_when_a_public_function_takes_a_private_enum_parameter() {
    let source = "module a\n\nenum Color\n    red\n    green\n\npub fn isRed(c: Color): bool\n    return c is Color::red\n";
    let root = temp_project("exposed-private-enum-param", |root| {
        write(root, "src/a.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let exposed = with_code(&report, "check.exposed_private_enum");
    assert_eq!(exposed.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(exposed[0].severity, marrow_syntax::Severity::Warning);
    assert_eq!(
        exposed[0].payload,
        DiagnosticPayload::ExposedPrivateEnum {
            enum_name: "a::Color".into(),
            function: "isRed".into(),
        }
    );
}

#[test]
fn a_private_enum_in_both_a_parameter_and_the_return_warns_per_occurrence() {
    let source = "module a\n\nenum Color\n    red\n    green\n\npub fn echo(c: Color): Color\n    return c\n";
    let root = temp_project("exposed-private-enum-both", |root| {
        write(root, "src/a.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let exposed = with_code(&report, "check.exposed_private_enum");
    assert_eq!(exposed.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        exposed.iter().all(|d| d.payload
            == DiagnosticPayload::ExposedPrivateEnum {
                enum_name: "a::Color".into(),
                function: "echo".into(),
            }),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_public_enum_in_a_public_signature_does_not_warn() {
    let source = "module a\n\npub enum Color\n    red\n    green\n\npub fn pick(): Color\n    return Color::red\n";
    let root = temp_project("public-enum-public-fn", |root| {
        write(root, "src/a.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        with_code(&report, "check.exposed_private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_private_function_with_a_private_enum_does_not_warn() {
    let source =
        "module a\n\nenum Color\n    red\n    green\n\nfn pick(): Color\n    return Color::red\n";
    let root = temp_project("private-enum-private-fn", |root| {
        write(root, "src/a.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        with_code(&report, "check.exposed_private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_cross_module_private_enum_is_a_hard_error_not_an_exposure_warning() {
    // Module `b` names module `a`'s non-`pub` enum: a foreign reference to a
    // private type is the hard `check.private_enum` error, not the same-module
    // exposure warning. The two diagnostics must not both fire.
    let root = temp_project("cross-module-private-enum", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\nenum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\nuse a\n\npub fn pick(): a::Color\n    return a::Color::red\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        !with_code(&report, "check.private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.exposed_private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_public_function_exposing_a_foreign_public_enum_does_not_warn() {
    let root = temp_project("foreign-public-enum", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\nuse a\n\npub fn pick(): a::Color\n    return a::Color::red\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        with_code(&report, "check.exposed_private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

fn config_with_default_entry(entry: &str) -> marrow_project::ProjectConfig {
    parse_config(&format!(
        r#"{{ "sourceRoots": ["src"], "store": {{ "backend": "memory" }}, "run": {{ "defaultEntry": "{entry}" }} }}"#
    ))
    .expect("config")
}

#[test]
fn rejects_a_missing_default_entry() {
    let root = temp_project("default-entry-missing", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let (report, _program) =
        check_project(&root, &config_with_default_entry("app::nope")).expect("check");

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.default_entry")
        .expect("default-entry diagnostic");
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::DefaultEntry {
            entry: "app::nope".into(),
            problem: marrow_check::DefaultEntryProblem::Missing,
        }
    );
    assert!(
        diagnostic.file.ends_with("marrow.json"),
        "{:?}",
        diagnostic.file
    );
}

#[test]
fn rejects_a_private_default_entry() {
    let root = temp_project("default-entry-private", |root| {
        write(root, "src/app.mw", "module app\n\nfn main()\n    return\n");
    });
    let (report, _program) =
        check_project(&root, &config_with_default_entry("app::main")).expect("check");

    let diagnostic = with_code(&report, "check.default_entry");
    assert_eq!(diagnostic.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostic[0].payload,
        DiagnosticPayload::DefaultEntry {
            entry: "app::main".into(),
            problem: marrow_check::DefaultEntryProblem::Private,
        }
    );
}

#[test]
fn rejects_an_empty_string_default_entry() {
    let root = temp_project("default-entry-empty", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config_with_default_entry("")).expect("check");

    let diagnostic = with_code(&report, "check.default_entry");
    assert_eq!(diagnostic.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostic[0].payload,
        DiagnosticPayload::DefaultEntry {
            entry: "".into(),
            problem: marrow_check::DefaultEntryProblem::Missing,
        }
    );
}

#[test]
fn rejects_an_ambiguous_default_entry() {
    let root = temp_project("default-entry-ambiguous", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
        write(
            root,
            "src/admin.mw",
            "module admin\n\npub fn main()\n    return\n",
        );
    });
    // A bare `main` names a `pub fn main` in two modules.
    let (report, _program) =
        check_project(&root, &config_with_default_entry("main")).expect("check");

    let diagnostic = with_code(&report, "check.default_entry");
    assert_eq!(diagnostic.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostic[0].payload,
        DiagnosticPayload::DefaultEntry {
            entry: "main".into(),
            problem: marrow_check::DefaultEntryProblem::Ambiguous,
        }
    );
}

#[test]
fn rejects_a_default_entry_with_parameters() {
    let root = temp_project("default-entry-params", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(name: string)\n    print(name)\n",
        );
    });
    // A default entry runs with no arguments; a parameter makes it unrunnable.
    let (report, _program) =
        check_project(&root, &config_with_default_entry("app::main")).expect("check");

    let diagnostic = with_code(&report, "check.default_entry");
    assert_eq!(diagnostic.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostic[0].payload,
        DiagnosticPayload::DefaultEntry {
            entry: "app::main".into(),
            problem: marrow_check::DefaultEntryProblem::HasParameters,
        }
    );
}

#[test]
fn a_clean_zero_argument_default_entry_is_accepted() {
    let root = temp_project("default-entry-ok", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let (report, _program) =
        check_project(&root, &config_with_default_entry("app::main")).expect("check");

    assert!(
        with_code(&report, "check.default_entry").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_bare_default_entry_into_a_module_less_script_is_accepted() {
    // A single module-less script joins the program under the empty module name; a
    // bare `main` resolves to its `pub fn main`, so the default entry is runnable.
    let root = temp_project("default-entry-script", |root| {
        write(root, "src/app.mw", "pub fn main()\n    return\n");
    });
    let (report, _program) =
        check_project(&root, &config_with_default_entry("main")).expect("check");

    assert!(
        with_code(&report, "check.default_entry").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_qualified_default_entry_resolves_unambiguously_across_modules() {
    // Two modules each declare `pub fn main`; the bare name is ambiguous, but a
    // qualified `admin::main` names exactly one, so it is accepted.
    let root = temp_project("default-entry-qualified", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
        write(
            root,
            "src/admin.mw",
            "module admin\n\npub fn main()\n    return\n",
        );
    });
    let (report, _program) =
        check_project(&root, &config_with_default_entry("admin::main")).expect("check");

    assert!(
        with_code(&report, "check.default_entry").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn no_default_entry_configured_is_clean() {
    let root = temp_project("default-entry-none", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        with_code(&report, "check.default_entry").is_empty(),
        "{:#?}",
        report.diagnostics
    );
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
