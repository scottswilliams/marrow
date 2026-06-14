mod support;

use std::path::Path;

use marrow_check::DiagnosticPayload;
use marrow_project::parse_config;

use support::{temp_project, write};

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/new_test.mw");
    let sources = ProjectSources::new().with(&path, "fn smoke()\n    var y: int = \"str\"\n");

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   return\n\
             \tconst BAD = 1\n",
        );
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use app\nfn smoke()\n    app::f()\n    var b: Book\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use std::definitely_missing\nfn smoke()\n    tests::helper::missing()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources =
        ProjectSources::new().with(&path, "fn smoke()\n    app()\n    var y: int = \"str\"\n");

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use app::missing\nfn smoke()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "use tests::helper\nfn smoke()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    app::missing()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    tests::app::testOnly()\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/b_smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    var f: Fixture\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
fn analyze_project_keeps_test_local_unknown_type_diagnostics_when_hidden_type_names_match() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project(
        "analyze-test-local-type-syntax-with-incomplete-source",
        |root| {
            write(
                root,
                "src/app.mw",
                "module app\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             \tconst BAD = 1\n",
            );
        },
    );
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let path = root.join("tests/smoke_test.mw");
    let sources = ProjectSources::new().with(
        &path,
        "fn smoke()\n    var n: Nope\n    var y: int = \"str\"\n",
    );

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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
