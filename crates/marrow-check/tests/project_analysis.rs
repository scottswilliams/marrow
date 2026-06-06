mod support;

use std::path::Path;

use marrow_check::{DiagnosticPayload, check_project};
use marrow_project::parse_config;

use support::{assert_clean, config, temp_project, write};

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
    assert_clean(&report);
}

#[test]
fn analyze_project_uses_overlay_source_instead_of_disk() {
    use marrow_check::{ProjectSources, analyze_project};

    // The on-disk file is clean; an editor overlay supplies buffer text for the
    // same path that introduces a checker error (`1 + true`). The overlay must
    // win — proving analysis reads buffer text, not the disk file, for that path.
    let root = temp_project("overlay-wins", |root| {
        write(root, "src/m.mw", "module m\nfn f()\n    var x = 1\n");
    });
    let path = root.join("src/m.mw");
    let sources = ProjectSources::new().with(&path, "module m\nfn f()\n    var x = 1 + true\n");

    let overlaid = analyze_project(&root, &config(), &sources).expect("analyze");
    let (clean, _program) = check_project(&root, &config()).expect("check");

    assert!(
        overlaid
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.operator_type"),
        "overlay text should be analyzed: {:#?}",
        overlaid.report.diagnostics
    );
    assert!(
        !clean.has_errors(),
        "disk source is clean: {:#?}",
        clean.diagnostics
    );
}

#[test]
fn analyze_project_reports_configured_test_file_parse_errors() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-parse", |root| {
        write(root, "src/app.mw", "module app\n");
        // A tab is a lexical error.
        write(root, "tests/bad_test.mw", "pub fn t()\n\tapp::noop()\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new()).expect("analyze");

    assert!(
        snapshot.report.diagnostics.iter().any(|d| {
            d.code == "parse.syntax" && d.file.ends_with(Path::new("tests/bad_test.mw"))
        }),
        "configured test diagnostics should be included: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_reports_unsaved_configured_test_file_parse_errors() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-unsaved-test-parse", |root| {
        write(root, "src/app.mw", "module app\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/new_test.mw");
    let sources = ProjectSources::new().with(&path, "pub fn t()\n\tapp::noop()\n");

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "parse.syntax" && d.file == path),
        "configured overlay test diagnostics should be included: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_retains_configured_test_files_in_snapshot() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-snapshot", |root| {
        write(root, "src/app.mw", "module app\n");
        write(root, "tests/smoke_test.mw", "fn smoke()\n    var x = 1\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new()).expect("analyze");

    let analyzed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("snapshot retains configured test file");
    assert_eq!(analyzed.module_name.as_deref(), Some("tests::smoke_test"));
    assert_eq!(analyzed.source, "fn smoke()\n    var x = 1\n");
}

#[test]
fn analyze_project_retains_unsaved_configured_test_files_in_snapshot() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-unsaved-test-snapshot", |root| {
        write(root, "src/app.mw", "module app\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/new_test.mw");
    let source = "fn smoke()\n    var x = 1\n";
    let sources = ProjectSources::new().with(&path, source);

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    let analyzed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("snapshot retains configured overlay test file");
    assert_eq!(analyzed.module_name.as_deref(), Some("tests::new_test"));
    assert_eq!(analyzed.source, source);
}

#[test]
fn analyze_project_includes_configured_tests_when_sources_have_errors() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-with-source-error", |root| {
        write(
            root,
            "src/app.mw",
            "module app\nfn f()\n    var x: int = \"str\"\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/new_test.mw");
    let sources = ProjectSources::new().with(&path, "fn smoke()\n    var y: int = \"str\"\n");

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot.files.iter().any(|file| file.path == path),
        "snapshot should retain configured test files even when source files have errors"
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "configured test checker diagnostics should be included: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_suppresses_test_resolution_noise_when_source_modules_are_incomplete() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-incomplete-source", |root| {
        // The tab is a lexical error, so this file contributes no `app` module,
        // even though the parser saw its resource and function declarations.
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             fn f()\n\
             \x20   return\n\
             \tconst BAD = 1\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use app\nfn smoke()\n    app::f()\n    var b: Book\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        !snapshot.report.diagnostics.iter().any(|d| {
            d.file == path
                && (d.code == "check.unresolved_import"
                    || d.code == "check.unresolved_call"
                    || d.code == "check.unknown_type")
        }),
        "resolution against incomplete source modules should be suppressed: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_keeps_test_local_resolution_diagnostics_when_source_modules_are_incomplete() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-local-errors-with-incomplete-source", |root| {
        write(root, "src/app.mw", "module app\n\tfn f()\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use std::definitely_missing\nfn smoke()\n    tests::helper::missing()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    for code in [
        "check.unresolved_import",
        "check.unresolved_call",
        "check.unknown_type",
        "check.assignment_type",
    ] {
        assert!(
            snapshot
                .report
                .diagnostics
                .iter()
                .any(|d| d.code == code && d.file == path),
            "{code} should remain for test-local errors: {:#?}",
            snapshot.report.diagnostics
        );
    }
}

#[test]
fn analyze_project_keeps_test_local_bare_call_matching_hidden_source_module() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project(
        "analyze-test-local-bare-call-with-incomplete-source",
        |root| {
            write(root, "src/app.mw", "module app\n\tfn f()\n");
        },
    );
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources =
        ProjectSources::new().with(&path, "fn smoke()\n    app()\n    var y: int = \"str\"\n");

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_call" && d.file == path),
        "bare test-local calls should remain: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_keeps_test_local_submodule_import_matching_hidden_source_prefix() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project(
        "analyze-test-local-submodule-import-with-incomplete-source",
        |root| {
            write(root, "src/app.mw", "module app\n\tfn f()\n");
        },
    );
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use app::missing\nfn smoke()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_import" && d.file == path),
        "submodule imports should remain exact-module errors: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_keeps_test_local_unresolved_call_when_another_test_has_parse_error() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-local-call-with-broken-sibling-test", |root| {
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    return\n",
        );
        write(root, "tests/a_bad_test.mw", "fn broken()\n\treturn\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    for code in [
        "check.unresolved_call",
        "check.unknown_type",
        "check.assignment_type",
    ] {
        assert!(
            snapshot
                .report
                .diagnostics
                .iter()
                .any(|d| d.code == code && d.file == path),
            "{code} should remain for the clean configured test: {:#?}",
            snapshot.report.diagnostics
        );
    }
}

#[test]
fn analyze_project_suppresses_unresolved_import_when_broken_configured_test_is_imported() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-import-broken-sibling-test", |root| {
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    return\n",
        );
        write(root, "tests/helper.mw", "fn helper()\n\treturn\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use tests::helper\nfn smoke()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        !snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_import" && d.file == path),
        "imports of incomplete configured tests should not become resolution noise: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_ignores_declared_modules_in_broken_configured_tests_for_call_suppression() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project(
        "analyze-test-local-call-with-broken-declared-module-test",
        |root| {
            write(
                root,
                "src/app.mw",
                "module app\npub fn main()\n    return\n",
            );
            write(
                root,
                "tests/a_bad_test.mw",
                "module app\nfn broken()\n\treturn\n",
            );
        },
    );
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    app::missing()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_call" && d.file == path),
        "declared modules in configured tests must not suppress source-module calls: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_keeps_source_module_calls_when_broken_test_path_collides() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-path-collides-with-source-module", |root| {
        write(
            root,
            "src/tests/app.mw",
            "module tests::app\npub fn main()\n    return\n",
        );
        write(root, "tests/app.mw", "fn broken()\n\treturn\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_call" && d.file == path),
        "broken test paths must not suppress calls into complete source modules: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_keeps_test_module_calls_when_broken_source_path_collides() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-source-path-collides-with-test-module", |root| {
        write(root, "src/tests/app.mw", "module tests::app\n\tfn f()\n");
        write(root, "tests/app.mw", "fn existing()\n    return\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_call" && d.file == path),
        "broken source paths must not suppress calls into complete test modules: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_reports_duplicate_when_test_module_collides_with_source_module() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-module-duplicates-source-module", |root| {
        write(
            root,
            "src/tests/app.mw",
            "module tests::app\npub fn sourceOnly()\n    return\n",
        );
        write(root, "tests/app.mw", "pub fn testOnly()\n    return\n");
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    tests::app::testOnly()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    let duplicate = snapshot
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == "check.duplicate_module" && d.file.ends_with(Path::new("tests/app.mw")))
        .unwrap_or_else(|| {
            panic!(
                "a configured test module must not silently duplicate a source module: {:#?}",
                snapshot.report.diagnostics
            )
        });
    assert_eq!(
        duplicate.payload,
        DiagnosticPayload::DuplicateModule {
            name: "tests::app".into(),
            first_file: root.join("src/tests/app.mw"),
        }
    );
    assert!(
        !snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_call" && d.file == path),
        "the duplicate-module error should not surface as a misleading unresolved call: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_suppresses_unknown_types_from_broken_configured_test_declarations() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-type-from-broken-sibling-test", |root| {
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    return\n",
        );
        write(
            root,
            "tests/a_bad_test.mw",
            "resource Fixture\n    title: string\n\tconst BAD = 1\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    var f: Fixture\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        !snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unknown_type" && d.file == path),
        "types declared in broken configured tests should not become sibling unknown-type noise: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analyze_project_keeps_test_local_type_syntax_diagnostics_when_hidden_type_names_match() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project(
        "analyze-test-local-type-syntax-with-incomplete-source",
        |root| {
            write(
                root,
                "src/app.mw",
                "module app\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \tconst BAD = 1\n",
            );
        },
    );
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    var n: map[Book,int]\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unknown_type" && d.file == path),
        "test-local type syntax diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "other test-local checker diagnostics should remain: {:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn analysis_snapshot_retains_files_with_parse_errors() {
    use marrow_check::{ProjectSources, analyze_project};

    // A tab is a lexical error, so the file carries a parse diagnostic and
    // contributes no module to the program. The snapshot must still retain the
    // parsed file (with its parse diagnostic) so editor tooling can work on it.
    let root = temp_project("snapshot-parse-error", |root| {
        write(root, "src/bad.mw", "module bad\n\tconst X: int = 1\n");
    });
    let path = root.join("src/bad.mw");

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new()).expect("analyze");

    let analyzed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("snapshot retains the error file");
    assert!(
        analyzed.parsed.has_errors(),
        "retained file carries its parse diagnostic: {:#?}",
        analyzed.parsed.diagnostics
    );
    assert!(
        analyzed
            .parsed
            .diagnostics
            .iter()
            .any(|d| d.code == "parse.syntax"),
        "{:#?}",
        analyzed.parsed.diagnostics
    );
    assert!(
        !snapshot
            .program
            .modules
            .iter()
            .any(|module| module.source_file == path),
        "the error file contributes no module to the program"
    );
}

#[test]
fn surfaces_resource_body_index_errors() {
    let root = temp_project("schema-error", |root| {
        // Resource bodies no longer own index declarations; indexes belong to the
        // store body, so a nested resource-body index is rejected by the parser.
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
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "parse.syntax" && diagnostic.span.line == 5),
        "{:#?}",
        report.diagnostics
    );
}
