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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

    assert!(
        snapshot.report.diagnostics.iter().any(|d| {
            d.code == "check.duplicate_module" && d.file.ends_with(Path::new("tests/app.mw"))
        }),
        "a configured test module must not silently duplicate a source module: {:#?}",
        snapshot.report.diagnostics
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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
fn rejects_an_enum_typed_identity_key() {
    // A key must be an orderable scalar. An enum names no scalar, so accepting it
    // as an identity key lets a raw string or int settle silently into the
    // keyspace. The rule is structural, so it fires without resolving the name.
    let errors = check_module(
        "enum-identity-key",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order at ^orders(state: Status)\n\
         \x20   required note: string\n",
        "schema.nonscalar_key",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("Status"));
}

#[test]
fn rejects_an_enum_typed_layer_key_param() {
    let errors = check_module(
        "enum-layer-key",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order at ^orders(id: int)\n\
         \x20   byState(state: Status): string\n",
        "schema.nonscalar_key",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("Status"));
}

#[test]
fn rejects_a_typo_named_identity_key() {
    // A name that resolves to nothing is rejected exactly like a declared one: the
    // allowlist asks only "is this an orderable scalar?". A typo'd key would
    // otherwise accept any value, letting an int and a string coexist in one
    // identity keyspace.
    let errors = check_module(
        "typo-identity-key",
        "module m\n\
         resource Order at ^orders(state: Stutus)\n\
         \x20   required note: string\n",
        "schema.nonscalar_key",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("Stutus"));
}

#[test]
fn rejects_a_typo_named_layer_key_param() {
    let errors = check_module(
        "typo-layer-key",
        "module m\n\
         resource Order at ^orders(id: int)\n\
         \x20   byState(state: Stutus): string\n",
        "schema.nonscalar_key",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("Stutus"));
}

#[test]
fn rejects_a_resource_typed_identity_key() {
    // A bare name that names a declared resource is still not an orderable scalar.
    // `Person` here is a declared resource, yet it cannot be a key.
    let errors = check_module(
        "resource-identity-key",
        "module m\n\
         resource Person\n\
         \x20   required name: string\n\
         resource Order at ^orders(owner: Person)\n\
         \x20   required note: string\n",
        "schema.nonscalar_key",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("Person"));
}

#[test]
fn rejects_a_cross_module_qualified_enum_identity_key() {
    // A qualified `a::Status` key is structurally a non-scalar name, so it is
    // rejected without resolving which module owns it. This is the case a
    // file-local enum list could never reach.
    let root = temp_project("cross-module-enum-key", |root| {
        write(
            root,
            "src/a.mw",
            "module a\nenum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             resource Order at ^orders(state: a::Status)\n    required note: string\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "schema.nonscalar_key");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(found[0].message.contains("a::Status"));
}

#[test]
fn rejects_a_sequence_index_argument() {
    // An index argument keys on its field's stored scalar. A `sequence` has none,
    // so the index is rejected at the third key position.
    let errors = check_module(
        "sequence-index-arg",
        "module m\n\
         resource Order at ^orders(id: int)\n\
         \x20   tags: sequence[string]\n\
         \x20   index byTags(tags, id)\n",
        "schema.nonscalar_key",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("byTags"));
}

#[test]
fn an_enum_field_index_argument_checks_clean() {
    // An enum field stores its ordinal as an orderable `int`, so an index over it
    // keys on that ordinal — the staged enum-field-index behavior, unchanged.
    let report = check_module_report(
        "enum-index-ok",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order at ^orders(id: int)\n\
         \x20   state: Status\n\
         \x20   index byState(state, id)\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn an_orderable_scalar_key_checks_clean() {
    // The allowlist does not over-reject an orderable scalar key alongside a
    // declared enum field on the same resource.
    let report = check_module_report(
        "scalar-key-ok",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order at ^orders(id: int)\n\
         \x20   required state: Status\n\
         \x20   byTag(tag: string): string\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A nested enum used as a value, a `match` over its leaves, and `is` tests all
/// over one declaration, used by the hierarchy checker tests.
fn cat_enum() -> &'static str {
    "module m\n\
     enum Cat\n\
     \x20   category tiger\n\
     \x20       bengal\n\
     \x20       siberian\n\
     \x20   housecat\n"
}

#[test]
fn value_is_category_types_bool() {
    let report = check_module_report(
        "is-types-bool",
        &format!(
            "{}\
             fn f(pet: Cat): bool\n    \
             return pet is Cat::tiger\n",
            cat_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn is_against_a_concrete_leaf_is_clean() {
    // `is` against a concrete-leaf right operand is the exact case; it types `bool`
    // with no category error.
    let report = check_module_report(
        "is-leaf",
        &format!(
            "{}\
             fn f(pet: Cat): bool\n    \
             return pet is Cat::bengal\n",
            cat_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn is_with_a_non_enum_left_is_rejected() {
    let errors = check_module(
        "is-non-enum",
        &format!(
            "{}\
             fn f(): bool\n    \
             return 1 is Cat::tiger\n",
            cat_enum()
        ),
        "check.is_requires_enum",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn is_against_a_different_enum_is_rejected() {
    let errors = check_module(
        "is-cross-enum",
        &format!(
            "{}\
             enum Dog\n    \
             poodle\n    \
             beagle\n\n\
             fn f(pet: Cat): bool\n    \
             return pet is Dog::poodle\n",
            cat_enum()
        ),
        "check.is_type",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn a_category_is_not_selectable_in_value_position() {
    let errors = check_module(
        "category-not-selectable",
        &format!(
            "{}\
             fn f(): Cat\n    \
             return Cat::tiger\n",
            cat_enum()
        ),
        "check.category_not_selectable",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn a_category_member_error_does_not_emit_an_untyped_return_hint() {
    let report = check_module_report(
        "category-no-untyped-cascade",
        &format!(
            "{}\
             fn f(): Cat\n    \
             return Cat::tiger\n",
            cat_enum()
        ),
    );
    assert_eq!(
        with_code(&report, "check.category_not_selectable").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_match_with_a_category_arm_covers_its_subtree() {
    // A `tiger` arm covers both `bengal` and `siberian`; with `housecat` covered,
    // the match is exhaustive over the selectable leaves.
    let report = check_module_report(
        "match-category-arm",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             housecat\n            return 2\n",
            cat_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_match_missing_a_leaf_is_nonexhaustive() {
    // Listing only `bengal` and `housecat` leaves `siberian` uncovered.
    let errors = check_module(
        "match-missing-leaf",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             bengal\n            return 1\n        \
             housecat\n            return 2\n",
            cat_enum()
        ),
        "check.nonexhaustive_match",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].message.contains("tiger::siberian"),
        "{:#?}",
        errors[0].message
    );
}

#[test]
fn a_non_category_parent_with_children_is_rejected() {
    // `tiger` has children but is not marked `category`, so it is a grouping node a
    // value can never hold and a `match` can never cover. The schema rule rejects it
    // at check time; without it the program would check clean here yet fault at run,
    // since `Cat::tiger` types as a value the match's leaf coverage cannot reach.
    // The repro uses `tiger` BOTH as a value (`var t: Cat = Cat::tiger`) and as a
    // match scrutinee, so the rejection lands regardless of how the parent is used.
    let errors = check_module(
        "parent-not-category",
        "module m\n\
         enum Cat\n    \
         tiger\n        \
         bengal\n        \
         siberian\n    \
         housecat\n\
         fn classify(pet: Cat): int\n    \
         match pet\n        \
         bengal\n            return 1\n        \
         siberian\n            return 2\n        \
         housecat\n            return 3\n\
         fn use_value(): int\n    \
         var t: Cat = Cat::tiger\n    \
         return classify(t)\n",
        "schema.parent_not_category",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("tiger"), "{:#?}", errors[0]);
}

#[test]
fn a_correctly_marked_category_parent_checks_clean() {
    // The same program with `category tiger` is well-formed: `tiger` is no longer a
    // value, and the match's leaf coverage (`bengal`, `siberian`, `housecat`) is
    // exhaustive. The fix must not over-reject the correct shape.
    let report = check_module_report(
        "parent-category-clean",
        "module m\n\
         enum Cat\n    \
         category tiger\n        \
         bengal\n        \
         siberian\n    \
         housecat\n\
         fn classify(pet: Cat): int\n    \
         match pet\n        \
         bengal\n            return 1\n        \
         siberian\n            return 2\n        \
         housecat\n            return 3\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_leaf_and_its_ancestor_category_overlap_is_a_duplicate_arm() {
    // Covering both `tiger` (the category) and `bengal` (a leaf under it) double-
    // covers `bengal`.
    let errors = check_module(
        "match-overlap",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             bengal\n            return 2\n        \
             housecat\n            return 3\n",
            cat_enum()
        ),
        "check.duplicate_match_arm",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn an_overlapping_arm_yields_only_a_duplicate_not_a_secondary_nonexhaustive() {
    // `bengal` covers itself, then the `tiger` category overlaps it and is rejected
    // as a duplicate. Rejecting `tiger` must not drop its other leaf (`siberian`)
    // from coverage and falsely report the match non-exhaustive: the overlap is one
    // clear diagnostic, never two.
    let report = check_module_report(
        "overlap-no-secondary-nonexhaustive",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             bengal\n            return 1\n        \
             tiger\n            return 2\n        \
             housecat\n            return 3\n",
            cat_enum()
        ),
    );
    assert_eq!(
        with_code(&report, "check.duplicate_match_arm").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.nonexhaustive_match").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_flat_enum_match_and_equality_still_check() {
    // A flat enum is unchanged: its `match` is exhaustive over its members and `==`
    // is exact nominal equality, both clean.
    let report = check_module_report(
        "flat-enum-regression",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         fn label(s: Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n\n\
         fn same(): bool\n    return Status::active == Status::active\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// An enum where `paw` appears under two categories — the blessed duplicate-name
/// case. Pre-order: tiger(0), bengal(1), paw(2), lion(3), paw(4), mane(5).
fn duplicate_paw_enum() -> &'static str {
    "module m\n\
     enum Cat\n\
     \x20   category tiger\n        bengal\n        paw\n\
     \x20   category lion\n        paw\n        mane\n"
}

#[test]
fn a_full_member_path_to_a_duplicated_leaf_resolves_in_value_position() {
    // `Cat::tiger::paw` and `Cat::lion::paw` are distinct members, both selectable.
    let report = check_module_report(
        "dup-value-full-path",
        &format!(
            "{}\
             fn a(): Cat\n    return Cat::tiger::paw\n\
             fn b(): Cat\n    return Cat::lion::paw\n",
            duplicate_paw_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_bare_duplicated_member_in_value_position_is_ambiguous() {
    // Bare `Cat::paw` names `paw` under both `tiger` and `lion`; the value cannot
    // pick one. The message must name the qualifying paths.
    let errors = check_module(
        "dup-value-bare",
        &format!(
            "{}\
             fn a(): Cat\n    return Cat::paw\n",
            duplicate_paw_enum()
        ),
        "check.ambiguous_member",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].message.contains("tiger::paw") && errors[0].message.contains("lion::paw"),
        "{:#?}",
        errors[0].message
    );
}

#[test]
fn an_ambiguous_enum_member_does_not_emit_an_untyped_return_hint() {
    let report = check_module_report(
        "dup-value-bare-no-untyped-cascade",
        &format!(
            "{}\
             fn a(): Cat\n    return Cat::paw\n",
            duplicate_paw_enum()
        ),
    );
    assert_eq!(
        with_code(&report, "check.ambiguous_member").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_match_with_qualified_arms_over_duplicated_leaves_is_exhaustive() {
    // Arms `tiger::paw`, `lion::paw`, `tiger::bengal`, `lion::mane` cover every
    // selectable leaf exactly once.
    let report = check_module_report(
        "dup-match-qualified",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger::bengal\n            return 1\n        \
             tiger::paw\n            return 2\n        \
             lion::paw\n            return 3\n        \
             lion::mane\n            return 4\n",
            duplicate_paw_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_bare_duplicated_match_arm_is_actionably_ambiguous() {
    // A bare `paw` arm cannot pick a subtree; the diagnostic names the qualifying
    // paths so the dev can disambiguate.
    let errors = check_module(
        "dup-match-bare-arm",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger::bengal\n            return 1\n        \
             paw\n            return 2\n        \
             lion::mane\n            return 3\n",
            duplicate_paw_enum()
        ),
        "check.ambiguous_match_arm",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].message.contains("tiger::paw") && errors[0].message.contains("lion::paw"),
        "{:#?}",
        errors[0].message
    );
}

#[test]
fn a_match_with_category_arms_over_a_duplicated_enum_is_exhaustive() {
    // Two category arms `tiger` and `lion` each cover their whole subtree, so every
    // leaf is covered exactly once.
    let report = check_module_report(
        "dup-match-category",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             lion\n            return 2\n",
            duplicate_paw_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_category_arm_overlapping_a_qualified_leaf_arm_is_a_duplicate() {
    // `tiger` covers `tiger::paw`, so a separate `tiger::paw` arm double-covers it.
    let errors = check_module(
        "dup-match-overlap",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             tiger::paw\n            return 2\n        \
             lion\n            return 3\n",
            duplicate_paw_enum()
        ),
        "check.duplicate_match_arm",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn a_match_missing_a_duplicated_leaf_reports_its_full_path() {
    // Dropping `lion::mane` leaves it uncovered; the non-exhaustive message names it
    // by its full path so a bare `mane` is unambiguous to the reader.
    let errors = check_module(
        "dup-match-nonexhaustive",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger::bengal\n            return 1\n        \
             tiger::paw\n            return 2\n        \
             lion::paw\n            return 3\n",
            duplicate_paw_enum()
        ),
        "check.nonexhaustive_match",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].message.contains("lion::mane"),
        "{:#?}",
        errors[0].message
    );
}

#[test]
fn is_with_a_full_member_path_is_exact_and_a_category_is_a_subtree_test() {
    // `pet is Cat::tiger::paw` is the exact-leaf test; `pet is Cat::tiger` is the
    // subtree test. Both type clean — `is` admits a category right operand.
    let report = check_module_report(
        "dup-is-full-path",
        &format!(
            "{}\
             fn exact(pet: Cat): bool\n    return pet is Cat::tiger::paw\n\
             fn subtree(pet: Cat): bool\n    return pet is Cat::tiger\n",
            duplicate_paw_enum()
        ),
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn is_with_a_bare_duplicated_member_is_ambiguous() {
    // A bare `Cat::paw` as an `is` operand is the symmetric footgun; reject it with
    // the qualifying paths, like value position.
    let errors = check_module(
        "dup-is-bare",
        &format!(
            "{}\
             fn f(pet: Cat): bool\n    return pet is Cat::paw\n",
            duplicate_paw_enum()
        ),
        "check.ambiguous_member",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].message.contains("tiger::paw") && errors[0].message.contains("lion::paw"),
        "{:#?}",
        errors[0].message
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
fn prototype_only_constructs_are_rejected_after_parsing() {
    let report = check_module_report(
        "prototype-only",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn normalize(inout book: Book)\n    return\n\
         fn save(out book: Book)\n    book = Book(title: \"saved\")\n\n\
         fn f(id: int)\n    var local = Book(title: \"local\")\n    normalize(inout local)\n    save(out ^books(id))\n    lock ^books(id)\n        print(\"locked\")\n    merge ^books(id) = ^books(id)\n    normalize(inout ^books(id))\n",
    );

    let found = with_code(&report, "check.prototype_only");
    assert_eq!(found.len(), 3, "{:#?}", report.diagnostics);
    assert!(
        found.iter().any(|d| d.message.contains("lock")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("merge")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("saved `inout`")),
        "{found:#?}"
    );
}

#[test]
fn saved_inout_through_index_entry_is_prototype_only() {
    let report = check_module_report(
        "prototype-index-inout",
        "module m\n\
         resource Book at ^books(id: int)\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn touch(inout id: Book::Id)\n    return\n\
         fn f(id: int)\n    touch(inout ^books.byShelf(\"fiction\")(id))\n",
    );

    let found = with_code(&report, "check.prototype_only");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(found[0].message.contains("saved `inout`"), "{found:#?}");
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
    assert_eq!(duplicates[0].span.line, 4, "{:#?}", duplicates[0]);
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
            "module app\n\npub fn calls_app()\n    std::assert::isTrue(app::add() == 1)\n",
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
fn map_annotations_outside_resource_members_are_not_supported_types() {
    let root = temp_project("map-type-annotation", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nresource Draft\n    scores: map[string, int]\nconst X: map[string, int] = 1\nfn f(a: map[string, int]): map[string, int]\n    return 1\nfn g()\n    const c: map[string, int] = 1\n    var v: map[string, int]\n    var counts(k: map[string, int]): int\n    try\n        return\n    catch e: map[string, int]\n        return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let found = with_code(&report, "check.unknown_type");
    assert_eq!(found.len(), 7, "{:#?}", report.diagnostics);
    assert!(
        found
            .iter()
            .all(|diagnostic| diagnostic.message.contains("map[string,int]")),
        "{found:#?}"
    );
    let schema = with_code(&report, "schema.unsupported_type");
    assert_eq!(schema.len(), 1, "{:#?}", report.diagnostics);
    assert!(schema[0].message.contains("scores"), "{schema:#?}");
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
fn bytes_interpolation_is_a_check_error() {
    let found = check_module(
        "interp-bytes",
        "module m\nfn f(): string\n    const b: bytes = b\"hi\"\n    return $\"<{b}>\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("bytes"), "{}", found[0].message);
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
fn rejects_duplicate_named_arguments() {
    // The second `a:` cannot stand in for the missing `c:` parameter.
    let found = check_module(
        "call-duplicate-named",
        "module m\n\
         fn add(a: int, b: int, c: int): int\n    return a + b + c\n\n\
         fn caller()\n    var x = add(a: 1, a: 2, b: 3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("a"), "{found:#?}");
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
fn out_and_inout_calls_keep_their_declared_return_types() {
    let report = check_module_report(
        "out-inout-return-types",
        "module m\n\
         fn parse(out value: int): bool\n    value = 7\n    return true\n\
         fn take(inout remaining: int, unit: int): string\n    remaining = remaining - unit\n    return \"ok\"\n\n\
         fn caller(): string\n    var n: int = 0\n    if parse(out n)\n        const piece: string = take(inout n, 1)\n        return piece\n    return \"no\"\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn out_parameters_must_be_assigned_before_returning() {
    let found = check_module(
        "out-assignment",
        "module m\n\
         fn never_set(out value: int)\n    const ignore: int = 1\n",
        "check.out_parameter_assignment",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn read_only_parameters_are_not_assignment_targets() {
    let found = check_module(
        "readonly-param",
        "module m\n\
         fn bump(value: int): int\n    value = value + 1\n    return value\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn read_only_parameter_checks_respect_local_shadowing() {
    let report = check_module_report(
        "readonly-param-shadow",
        "module m\n\
         fn set_to(out value: int)\n    value = 1\n\
         fn caller(value: int): int\n    if true\n        var value: int = 0\n        value = value + 1\n        set_to(out value)\n        return value\n    return value\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn read_only_parameters_are_not_out_or_inout_arguments() {
    let found = check_module(
        "readonly-param-out-arg",
        "module m\n\
         fn set_to(out value: int)\n    value = 1\n\
         fn caller(value: int): int\n    set_to(out value)\n    return value\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn out_parameters_can_be_assigned_by_out_calls() {
    let report = check_module_report(
        "out-call-assigns-out-param",
        "module m\n\
         fn set_to(out value: int)\n    value = 1\n\
         fn relay(out value: int)\n    set_to(out value)\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn short_circuit_rhs_out_calls_do_not_assign_out_parameters() {
    let found = check_module(
        "out-call-short-circuit-rhs",
        "module m\n\
         fn set_to(out value: int): bool\n    value = 1\n    return true\n\
         fn relay_and(out value: int)\n    if false and set_to(out value)\n        return\n\
         fn relay_or(out value: int)\n    if true or set_to(out value)\n        return\n",
        "check.out_parameter_assignment",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn finally_assignment_counts_before_try_return_completes() {
    let report = check_module_report(
        "out-finally-assigns-before-return",
        "module m\n\
         fn relay(out value: int)\n    try\n        return\n    finally\n        value = 1\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn out_and_inout_call_markers_must_match_parameters() {
    let missing = check_module(
        "out-marker-missing",
        "module m\n\
         fn set_to(out value: int, src: int)\n    value = src\n\
         fn caller(src: int): int\n    var n: int = 0\n    set_to(n, src)\n    return n\n",
        "check.call_argument",
    );
    assert_eq!(missing.len(), 1, "{missing:#?}");

    let wrong = check_module(
        "inout-marker-wrong",
        "module m\n\
         fn add(inout value: int, src: int)\n    value = value + src\n\
         fn caller(src: int): int\n    var n: int = 0\n    add(out n, src)\n    return n\n",
        "check.call_argument",
    );
    assert_eq!(wrong.len(), 1, "{wrong:#?}");
}

#[test]
fn out_and_inout_arguments_must_be_writable_places() {
    let found = check_module(
        "out-literal",
        "module m\n\
         fn set_to(out value: int, src: int)\n    value = src\n\
         fn caller(src: int): int\n    set_to(out 5, src)\n    return src\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn out_and_inout_markers_are_rejected_on_plain_call_targets() {
    let found = check_module(
        "out-marker-plain-calls",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\
         fn caller()\n    var s: string = \"abc\"\n    var id: int = 1\n    print(out 5)\n    const len: int = std::text::length(out s)\n    const book_id: Book::Id = Book::Id(out id)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 3, "{found:#?}");
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
    // `std::bogus::foo()` names no real std module (the std-module set derived
    // from the shared stdlib table), so it is not a builtin — it is reported
    // consistently with `use std::bogus` rejection, rather than silently
    // type-checking.
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
fn a_resource_constructor_checks_field_arguments() {
    let found = check_module(
        "ctor-field-type",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n\
         fn caller()\n    var b = Book(title: 1, shelf: \"fiction\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_resource_constructor_rejects_unknown_fields() {
    let found = check_module(
        "ctor-unknown-field",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn caller()\n    var b = Book(title: \"a\", pages: 3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_resource_constructor_requires_required_fields() {
    let found = check_module(
        "ctor-required-field",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n\
         fn caller()\n    var b = Book(shelf: \"fiction\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_qualified_resource_constructor_is_not_an_unresolved_call() {
    let root = temp_project("qualified-resource-constructor", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book\n    title: string\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse library\nfn caller()\n    var b = library::Book(title: \"Mort\")\n",
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
fn an_identity_constructor_keeps_precedence_over_a_qualified_resource_name() {
    let root = temp_project("identity-constructor-precedence", |root| {
        write(
            root,
            "src/Book.mw",
            "module Book\nresource Id\n    title: string\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nresource Book at ^books(id: int)\n    title: string\nfn caller()\n    var id = Book::Id(1)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_call_in_a_module_less_script_is_flagged() {
    // A module-less script joins the program under the empty module name, so its
    // own calls resolve against it: a call naming a function the script does not
    // declare is `check.unresolved_call`, not a silently-accepted reference.
    let found = check_script(
        "call-script",
        "fn f()\n    mystery()\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_primary_root_loop_binds_resource_elements() {
    let report = check_module_report(
        "root-loop-elements",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn titles()\n    for book in ^books\n        var title: string = book.title\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_two_name_primary_root_loop_binds_identity_and_resource() {
    let report = check_module_report(
        "root-loop-entries",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn titles()\n    for id, book in ^books\n        var typed: Book::Id = id\n        var title: string = book.title\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_sequence_layer_loop_binds_element_values() {
    let report = check_module_report(
        "layer-loop-elements",
        "module m\n\
         resource Book at ^books(id: int)\n    tags: sequence[string]\n\n\
         fn tags(id: Book::Id)\n    for tag in ^books(id).tags\n        var text: string = tag\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_keyed_group_layer_loop_binds_group_entry_values() {
    let report = check_module_report(
        "group-layer-loop-elements",
        "module m\n\
         resource Book at ^books(id: int)\n    versions(version: int)\n        required title: string\n\n\
         fn titles(id: Book::Id)\n    for version in ^books(id).versions\n        var title: string = version.title\n    for n, version in ^books(id).versions\n        var typed: int = n\n        var title: string = version.title\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_single_name_entries_loop_does_not_bind_the_key_type() {
    let found = check_module(
        "single-name-entries",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for entry in entries(^books)\n        var n = entry + 1\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn two_name_keys_and_values_loops_do_not_bind_pair_types() {
    for wrapper in ["keys", "values"] {
        let found = check_module(
            &format!("two-name-{wrapper}"),
            &format!(
                "module m\n\
                 resource Book at ^books(id: int)\n    required title: string\n\n\
                 fn f()\n    for first, second in {wrapper}(^books)\n        var n = first + 1\n",
            ),
            "check.operator_type",
        );
        assert!(found.is_empty(), "{wrapper}: {found:#?}");
    }
}

#[test]
fn a_unique_index_lookup_loop_is_not_typed_as_an_index_branch() {
    let found = check_module(
        "unique-index-loop",
        "module m\n\
         resource Book at ^books(id: int)\n    isbn: string\n\n    index byIsbn(isbn) unique\n\n\
         fn f(isbn: string)\n    for id in ^books.byIsbn(isbn)\n        var n = id + 1\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn key_only_index_branches_do_not_bind_pair_types() {
    for iterable in [
        "^books.byShelf(\"fiction\")",
        "entries(^books.byShelf(\"fiction\"))",
    ] {
        let found = check_module(
            "index-pair-loop",
            &format!(
                "module m\n\
                 resource Book at ^books(id: int)\n    shelf: string\n\n    index byShelf(shelf, id)\n\n\
                 fn f()\n    for id, marker in {iterable}\n        var n = id + 1\n",
            ),
            "check.operator_type",
        );
        assert!(found.is_empty(), "{iterable}: {found:#?}");
    }
}

#[test]
fn singleton_root_keys_do_not_bind_generated_identities() {
    let found = check_module(
        "singleton-root-keys",
        "module m\n\
         resource Settings at ^settings\n    value: int\n\n\
         fn f()\n    for id in keys(^settings)\n        var n = id + 1\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn supported_collection_wrappers_bind_their_documented_shapes() {
    let report = check_module_report(
        "collection-wrapper-shapes",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in keys(^books)\n        var typed: Book::Id = id\n    for book in values(^books)\n        var title: string = book.title\n    for id, book in entries(^books)\n        var typed: Book::Id = id\n        var title: string = book.title\n    for book in reversed(values(^books))\n        var title: string = book.title\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn layer_key_traversal_binds_declared_key_types() {
    let report = check_module_report(
        "layer-key-traversal-types",
        "module m\n\
         resource Run at ^runs(id: int)\n    terms: sequence[string]\n    amounts(pos: int): decimal\n\n\
         fn f(id: Run::Id)\n    for pos in keys(^runs(id).terms)\n        const first: bool = pos == 1\n    for pos, amount in entries(^runs(id).amounts)\n        const numbered: bool = pos == 1\n        const total: decimal = amount + 1.0\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn composite_root_traversal_binds_addressable_identities() {
    let report = check_module_report(
        "composite-root-traversal-id",
        "module m\n\
         resource Cell at ^cells(x: int, y: int)\n    required v: int\n\n\
         fn f()\n    for id, cell in ^cells\n        const same: int = ^cells(id).v\n        const copy: int = cell.v\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn index_branches_reject_value_materialization_wrappers() {
    for wrapper in ["values", "entries"] {
        let found = check_module(
            &format!("index-{wrapper}-unsupported"),
            &format!(
                "module m\n\
                 resource Book at ^books(id: int)\n    shelf: string\n\n    index byShelf(shelf, id)\n\n\
                 fn f()\n    for item in {wrapper}(^books.byShelf(\"fiction\"))\n        write($\"{{item}}\")\n",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{wrapper}: {found:#?}");
    }
}

#[test]
fn reversed_saved_collection_expressions_type_element_sequences() {
    let found = check_module(
        "reversed-saved-expressions",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags: sequence[string]\n\n\
         fn f(id: Book::Id)\n    const books = reversed(^books)\n    for book in books\n        var bad = book.title + 1\n    const tags = reversed(^books(id).tags)\n    for tag in tags\n        var also_bad = tag + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
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
fn passing_a_resource_to_a_mismatched_resource_parameter_is_flagged() {
    // Resources are nominally typed: a `Book` argument to a `Shelf` parameter names
    // a different resource and is a real argument mismatch.
    let found = check_module(
        "resource-arg",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         resource Shelf at ^shelves(id: int)\n    name: string\n\n\
         fn useShelf(s: Shelf): bool\n    return true\n\n\
         fn f()\n    var book: Book\n    var ok = useShelf(book)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_resource_to_a_matching_resource_parameter_is_not_flagged() {
    // A `Book` argument to a `Book` parameter is the same resource, so it checks
    // clean — nominal typing accepts the matching resource.
    let found = check_module(
        "resource-arg-ok",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn useBook(b: Book): bool\n    return true\n\n\
         fn f()\n    var book: Book\n    var ok = useBook(book)\n",
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
fn append_to_a_group_layer_is_a_check_error() {
    let found = check_module(
        "append-group-layer",
        "module m\n\
         resource Log at ^log(name: string)\n    items(pos: int)\n        required n: int\n\n\
         fn add(name: string): int\n    return append(^log(name).items, 1)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains("leaf layer"),
        "{}",
        found[0].message
    );
}

#[test]
fn append_to_a_keyed_leaf_layer_still_checks_clean() {
    let report = check_module_report(
        "append-leaf-layer",
        "module m\n\
         resource Log at ^log(name: string)\n    items(pos: int): int\n\n\
         fn add(name: string): int\n    return append(^log(name).items, 1)\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn coalesce_yields_the_default_type() {
    // `path ?? default` types to the path's leaf-or-default type; with a string
    // default it is `string`, so `+ 1` is string-plus-int.
    let found = check_module(
        "coalesce-return",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    var x = (^books(1).title ?? \"none\") + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn coalesce_rejects_a_present_non_path_left_operand() {
    // `??` only defaults an absent read; a literal (or any always-present value)
    // on the left has nothing to default, so it is an operator misuse.
    let found = check_module(
        "coalesce-non-path",
        "module m\nfn f()\n    var x = 1 ?? 2\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn coalesce_rejects_a_mismatched_default_type() {
    // The default must match the path's leaf type: an `int` field defaulted with a
    // string is an operator misuse.
    let found = check_module(
        "coalesce-mismatch",
        "module m\n\
         resource Book at ^books(id: int)\n    pages: int\n\n\
         fn f()\n    var x = ^books(1).pages ?? \"none\"\n",
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
fn a_sequence_returning_std_call_against_a_scalar_return_is_flagged() {
    // `std::text::split` returns `sequence[string]`; returning it from an `int`
    // function is a real type mismatch — a sequence is not a scalar.
    let found = check_module(
        "std-return-seq",
        "module m\nfn f(): int\n    return std::text::split(\"a,b\", \",\")\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_sequence_returning_std_call_against_a_matching_return_is_not_flagged() {
    // Returning `sequence[string]` from a `sequence[string]` function recurses into
    // the element type and checks clean.
    let found = check_module(
        "std-return-seq-ok",
        "module m\nfn f(): sequence[string]\n    return std::text::split(\"a,b\", \",\")\n",
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
fn bytes_conversion_rejects_a_known_non_string_source() {
    let found = check_module(
        "bytes-conv-int",
        "module m\nfn f(): bytes\n    return bytes(int(9))\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("bytes"), "{}", found[0].message);
    assert!(found[0].message.contains("string"), "{}", found[0].message);
}

#[test]
fn bytes_conversion_accepts_string_bytes_and_unknown_sources() {
    let report = check_module_report(
        "bytes-conv-ok",
        "module m\n\
         fn fromString(s: string): bytes\n    return bytes(s)\n\n\
         fn fromBytes(b: bytes): bytes\n    return bytes(b)\n\n\
         fn fromUnknown(raw: unknown): bytes\n    return bytes(raw)\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn conversion_calls_reject_known_unsupported_sources() {
    let found = check_module(
        "conv-known-bad-sources",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn dateFromInt(): date\n    return date(1)\n\n\
         fn durationFromInt(): duration\n    return duration(1)\n\n\
         fn boolFromString(): bool\n    return bool(\"true\")\n\n\
         fn decimalFromBool(): decimal\n    return decimal(true)\n\n\
         fn enumToInt(): int\n    return int(Color::green)\n\n\
         fn enumToString(): string\n    return string(Color::green)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 6, "{found:#?}");
    assert!(
        found.iter().any(|d| d.message.contains("date")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("duration")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("bool")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("decimal")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.message.contains("Color")),
        "{found:#?}"
    );
}

#[test]
fn interpolation_rejects_enum_values() {
    let found = check_module(
        "interp-enum",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn f(c: Color): string\n    return $\"c={c}\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains("interpolation cannot render"),
        "{}",
        found[0].message
    );
    assert!(found[0].message.contains("Color"), "{}", found[0].message);
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
fn type_surface_count_builtin_result_is_an_int() {
    let report = check_module_report(
        "count-result-int",
        "module m\n\
         resource Book at ^books(id: int)\n    tags(pos: int): string\n\n\
         fn countBooks(): int\n    return count(^books)\n\n\
         fn countTags(id: Book::Id): int\n    return count(^books(id).tags)\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn type_surface_count_of_a_non_path_is_not_an_int() {
    let found = check_module(
        "count-non-path",
        "module m\nfn f(): int\n    return count(1)\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn type_surface_caught_error_fields_have_declared_types() {
    let report = check_module_report(
        "caught-error-fields",
        "module m\n\
         fn f()\n\
         \x20   try\n        throw Error(code: \"x.y\", message: \"boom\")\n\
         \x20   catch err: Error\n\
         \x20       const code: ErrorCode = err.code\n\
         \x20       const message: string = err.message\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn type_surface_ledger_reads_and_traversals_have_concrete_types() {
    let report = check_module_report(
        "ledger-type-surfaces",
        "module m\n\
         resource Account at ^accounts(code: string)\n    required name: string\n    amounts(pos: int): decimal\n\n\
         fn sumAmounts(code: Account::Id): decimal\n    var sum: decimal = 0.0\n    for amount in values(^accounts(code).amounts)\n        sum = sum + amount\n    return sum\n\n\
         fn countAccounts(): int\n    return count(^accounts)\n\n\
         fn ids()\n    for code in keys(^accounts)\n        const typed: Account::Id = code\n\n\
         fn accounts()\n    for account in ^accounts\n        const name: string = account.name\n\n\
         fn handle(): bool\n    try\n        throw Error(code: \"x.y\", message: \"m\")\n    catch err: Error\n        return err.code == ErrorCode(\"x.y\")\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
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
fn type_surface_singleton_keyed_leaf_read_feeds_type_checks() {
    let found = check_module(
        "singleton-keyed-leaf",
        "module m\n\
         resource Settings at ^settings\n    counts(name: string): int\n\n\
         fn f(name: string): int\n    return ^settings.counts(name)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_singleton_keyed_group_field_read_feeds_type_checks() {
    let found = check_module(
        "singleton-keyed-group-field",
        "module m\n\
         resource Settings at ^settings\n    tokens(pos: int)\n        kind: string\n\n\
         fn f(pos: int): string\n    return ^settings.tokens(pos).kind\n",
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
fn type_surface_optional_group_field_read_preserves_the_leaf_type() {
    let found = check_module(
        "optional-group-field",
        "module m\n\
         resource Book at ^books(id: int)\n\
         \x20   binding\n        cover: string\n\n\
         fn cover(id: Book::Id): string\n    return ^books(id)?.binding?.cover\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_optional_keyed_root_chain_is_not_a_typed_leaf() {
    let found = check_module(
        "optional-keyed-root-chain",
        "module m\n\
         resource Book at ^books(id: int)\n\
         \x20   binding\n        cover: string\n\n\
         fn cover(): string\n    return ^books?.binding?.cover\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
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
    // `nextId(^books)` is typed `Book::Id`, not `unknown`, so the initializer is the
    // nominal match — this guards the `const id: Book::Id = nextId(^books)` shape
    // against a false untyped-value error.
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
fn an_identity_typed_field_accepts_an_identity_of_that_resource() {
    // A saved field typed `Author::Id` is a reference: assigning a real
    // `Author::Id` is the nominal match, so nothing is flagged.
    let found = check_module(
        "ref-field-ok",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    authorId: Author::Id\n\n\
         fn f()\n    ^books(1).authorId = Author::Id(7)\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_typed_field_rejects_a_wrong_resource_identity() {
    // Assigning a `Book::Id` into an `Author::Id` field is the nominal mismatch a
    // typed reference forbids.
    let found = check_module(
        "ref-field-wrong-resource",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    authorId: Author::Id\n\n\
         fn f()\n    ^books(1).authorId = Book::Id(7)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_identity_typed_field_rejects_a_raw_scalar() {
    // A bare `int` is not an identity; it must be constructed as `Author::Id(...)`.
    let found = check_module(
        "ref-field-raw-scalar",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    authorId: Author::Id\n\n\
         fn f()\n    ^books(1).authorId = 7\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_an_identity_field_is_an_untyped_value() {
    // A dynamic `unknown` parameter stored into an `Author::Id` field is the
    // foreign-value hazard: a single raw key is a structurally valid identity
    // encoding, so `data integrity` cannot catch it later. The conversion boundary
    // for an identity is its `Author::Id(...)` constructor, so strict typing rejects
    // the unconverted value the same way a scalar place does — convert it first.
    let found = check_module(
        "ref-field-untyped",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    authorId: Author::Id\n\n\
         fn put(x: unknown)\n    ^books(1).authorId = x\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn nextid_into_an_identity_field_is_not_an_untyped_value() {
    // `nextId(^authors)` is typed `Author::Id`, not `unknown`, so assigning it into
    // an `Author::Id` field is the nominal match — never the untyped-value path.
    let found = check_module(
        "ref-field-nextid-ok",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    authorId: Author::Id\n\n\
         fn put()\n    ^books(1).authorId = nextId(^authors)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_converted_value_into_an_identity_field_is_not_an_untyped_value() {
    // Converting a dynamic value through the `Author::Id(...)` constructor produces
    // an `Author::Id`, so the assignment is the nominal match — the dynamic value has
    // been made typed and is no longer flagged.
    let found = check_module(
        "ref-field-converted-ok",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    authorId: Author::Id\n\n\
         fn put(x: unknown)\n    ^books(1).authorId = Author::Id(int(x))\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_a_wrong_typed_key() {
    // `Author::Id("x")` builds an identity for an `int`-keyed resource from a
    // string. The identity keyspace is typed, so a wrong-scalar key would settle a
    // string into an `int` keyslot; it is the same `check.key_type` a record lookup
    // `^authors("x")` reports.
    let found = check_module(
        "ctor-key-wrong-type",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         fn f()\n    const id = Author::Id(\"x\")\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_an_identity_as_a_key() {
    // `Author::Id(Book::Id(7))` passes another resource's identity where a scalar
    // `int` key is declared; an identity is not a key scalar, so it is the same
    // `check.key_type` mismatch.
    let found = check_module(
        "ctor-key-identity",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    const id = Author::Id(Book::Id(7))\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_a_wrong_typed_composite_key() {
    // A composite identity `Pair::Id(int, string)` built with a swapped second key
    // (`int` where `string` is declared) settles the wrong scalar into a keyslot;
    // each key is checked against its declared type, so this is `check.key_type`.
    let found = check_module(
        "ctor-composite-wrong-type",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(7, 9)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_identity_constructor_accepts_a_correct_key() {
    // `Author::Id(7)` matches the declared `int` key, so nothing is flagged — the
    // type guard must not over-reject a correct identity construction.
    let found = check_module(
        "ctor-key-ok",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         fn f()\n    const id = Author::Id(7)\n",
        "check.key_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_constructor_accepts_a_correct_composite_key() {
    // `Pair::Id(7, "x")` matches the declared `(int, string)` keys, positional and
    // named both — a correct composite must round-trip the type guard clean.
    let found = check_module(
        "ctor-composite-ok",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(7, \"x\")\n    const named = Pair::Id(a: 7, b: \"x\")\n",
        "check.key_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_a_wrong_typed_named_composite_key() {
    // Named composite keys are matched to their declared key by name, so a swapped
    // type under the right name (`b: 9` where `b: string`) is still `check.key_type`.
    let found = check_module(
        "ctor-named-wrong-type",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(a: 7, b: 9)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_the_wrong_key_count() {
    let found = check_module(
        "ctor-key-count",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         fn f()\n    const id = Author::Id()\n    const extra = Author::Id(1, 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn an_identity_constructor_requires_missing_named_keys() {
    let found = check_module(
        "ctor-named-missing-key",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(a: 7)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("b"), "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_unknown_named_keys() {
    let found = check_module(
        "ctor-named-unknown-key",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(a: 7, c: \"x\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("c"), "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_duplicate_named_keys() {
    let found = check_module(
        "ctor-named-duplicate-key",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(a: 7, a: 8)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("a"), "{found:#?}");
}

#[test]
fn an_identity_constructor_rejects_mixed_positional_and_named_keys() {
    let found = check_module(
        "ctor-mixed-keys",
        "module m\n\
         resource Pair at ^pairs(a: int, b: string)\n    note: string\n\n\
         fn f()\n    const id = Pair::Id(7, b: \"x\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_a_whole_resource_is_an_untyped_value() {
    // `^books(1) = x` writes a whole `Book`. A dynamic `unknown` value carries no
    // type, so its fields could spill a raw scalar or a foreign identity into a
    // typed (identity) field — a structurally valid encoding the runtime cannot
    // later distinguish. A whole resource is a concrete typed place, so the value
    // must be converted into a `Book` first.
    let found = check_module(
        "whole-resource-untyped",
        "module m\n\
         resource Book at ^books(id: int)\n    authorId: int\n\n\
         fn put(x: unknown)\n    ^books(1) = x\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_a_whole_group_entry_is_an_untyped_value() {
    // `^books(1).chapters(0) = x` writes a whole group entry. Like a whole
    // resource, the entry is a concrete typed record place, so a dynamic `unknown`
    // value (which could land a raw scalar or foreign identity in a typed field)
    // must be converted first.
    let found = check_module(
        "whole-group-entry-untyped",
        "module m\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20chapters(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20title: string\n\n\
         fn put(x: unknown)\n    ^books(1).chapters(0) = x\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_typed_whole_resource_write_is_not_an_untyped_value() {
    // A whole-resource write of a value already typed as the resource (a read
    // `^books(2)`, a constructed `Book(...)`, or a `Book`-typed local) is the
    // nominal match — never the untyped-value path.
    let found = check_module(
        "whole-resource-typed-ok",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn copy()\n    ^books(1) = ^books(2)\n\n\
         fn construct()\n    ^books(1) = Book(title: \"hi\")\n\n\
         fn local()\n    var b: Book\n    b.title = \"hi\"\n    ^books(1) = b\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_typed_whole_group_entry_write_is_not_an_untyped_value() {
    // A whole-group-entry write of a value typed as the owning resource (a
    // `Book`-typed local or another read group entry) is the nominal match, not the
    // untyped-value path.
    let report = check_module_report(
        "whole-group-entry-typed-ok",
        "module m\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20chapters(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\n\
         fn local()\n    var b: Book\n    b.title = \"v1\"\n    ^books(1).chapters(0) = b\n\n\
         fn copy()\n    ^books(1).chapters(1) = ^books(1).chapters(0)\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_group_entry_does_not_flow_as_a_whole_resource() {
    let source = "module m\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\n\
         fn takesBook(book: Book)\n    print(book.title)\n\n\
         fn returnsBook(id: Book::Id): Book\n    for version in ^books(id).versions\n        return version\n    return ^books(id)\n\n\
         fn pass(id: Book::Id)\n    for version in ^books(id).versions\n        takesBook(version)\n\n\
         fn assign(id: Book::Id)\n    for version in ^books(id).versions\n        var book: Book = version\n";

    let returns = check_module(
        "group-entry-not-resource-return",
        source,
        "check.return_type",
    );
    assert_eq!(returns.len(), 1, "{returns:#?}");
    let args = check_module(
        "group-entry-not-resource-arg",
        source,
        "check.call_argument",
    );
    assert_eq!(args.len(), 1, "{args:#?}");
    let assignments = check_module(
        "group-entry-not-resource-assignment",
        source,
        "check.assignment_type",
    );
    assert_eq!(assignments.len(), 1, "{assignments:#?}");
}

#[test]
fn a_whole_group_entry_write_rejects_a_different_group_layer() {
    let found = check_module(
        "whole-group-entry-different-layer",
        "module m\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20chapters(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\n\
         fn copy()\n    ^books(1).chapters(1) = ^books(1).versions(1)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn equality_on_two_identities_of_the_same_resource_types_bool() {
    // Two `Author::Id` values compare with `==`; the result is `bool`, so no
    // operator diagnostic is raised.
    let found = check_module(
        "ref-eq-same-resource",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         fn f(): bool\n    return Author::Id(1) == Author::Id(2)\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn equality_across_resource_identities_is_an_operator_error() {
    // `==` between an `Author::Id` and a `Book::Id` is a nominal category error.
    let found = check_module(
        "ref-eq-cross-resource",
        "module m\n\
         resource Author at ^authors(id: int)\n    name: string\n\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f(): bool\n    return Author::Id(1) == Book::Id(1)\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_self_referencing_identity_field_accepts_its_own_identity() {
    // A field typed `Self::Id`-style — here `Person::Id` on `Person` — is a valid
    // same-resource reference.
    let found = check_module(
        "ref-self",
        "module m\n\
         resource Person at ^people(id: int)\n    managerId: Person::Id\n\n\
         fn f()\n    ^people(1).managerId = Person::Id(2)\n",
        "check.assignment_type",
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
        "pub fn t()\n    std::assert::isTrue(app::add() == 1)\n    var n = std::text::length(\"hi\")\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn check_tests_catches_a_wrong_enum_to_a_qualified_project_parameter() {
    // A test file calls a project function whose parameter is the qualified
    // `app::Status`, passing `app::Color::green`. The test type pass reads the
    // project's already-normalized parameter, so the nominal mismatch is caught the
    // same way it is in a library call — not silently dispatched.
    let report = check_tests_report(
        "check-tests-enum-arg",
        "module app\n\
         pub enum Status\n    active\n    archived\n\n\
         pub enum Color\n    red\n    green\n\n\
         pub fn dispatch(s: app::Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n",
        "pub fn t()\n    var n = app::dispatch(app::Color::green)\n",
    );
    assert_eq!(
        with_code(&report, "check.call_argument").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn finally_return_is_rejected() {
    let found = check_script(
        "fin-return",
        "fn f()\n    try\n        x = 1\n    finally\n        return\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].span.line, 5, "{:#?}", found[0]);
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
    assert_eq!(found[0].span.line, 2, "{:#?}", found[0]);
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
fn throw_requires_an_error_value() {
    let found = check_script(
        "throw-non-error",
        "fn f()\n    throw \"oops\"\n",
        "check.throw_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn throwing_an_error_value_is_allowed() {
    let found = check_script(
        "throw-error",
        "fn f()\n    throw Error(code: \"test.error\", message: \"oops\")\n",
        "check.throw_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn try_requires_a_catch_or_finally_clause() {
    let found = check_script(
        "bare-try",
        "fn f()\n    try\n        write(\"x\")\n",
        "check.try_handler",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn try_with_catch_or_finally_is_allowed() {
    let with_catch = check_script(
        "try-catch",
        "fn f()\n    try\n        write(\"x\")\n    catch e\n        return\n",
        "check.try_handler",
    );
    assert!(with_catch.is_empty(), "{with_catch:#?}");

    let with_finally = check_script(
        "try-finally",
        "fn f()\n    try\n        write(\"x\")\n    finally\n        write(\"done\")\n",
        "check.try_handler",
    );
    assert!(with_finally.is_empty(), "{with_finally:#?}");
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
fn merge_reports_only_prototype_rejection() {
    let report = check_module_report("merge-bad", "module m\nfn f()\n    merge f(x) = y\n");
    assert!(
        with_code(&report, "check.invalid_assign_target").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "check.prototype_only").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
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
    // `keys(^books)` traverses the `^books` identity layer; `delete ^books(id)`
    // removes a key from that same layer, which the checker rejects.
    let found = check_module(
        "loop-delete-root",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in keys(^books)\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].span.line, 7, "{:#?}", found[0]);
}

#[test]
fn deleting_a_reversed_key_loop_traverses_is_rejected() {
    let found = check_module(
        "loop-reversed-delete-root",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in reversed(keys(^books))\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn appending_to_the_sequence_a_loop_traverses_is_rejected() {
    // `for tag in ^books(1).tags` traverses the `tags` layer; `append(...tags...)`
    // adds a key to that same layer.
    let found = check_module(
        "loop-append-seq",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for tag in ^books(1).tags\n        append(^books(1).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn appending_to_a_string_keyed_layer_is_rejected() {
    let found = check_module(
        "append-string-keyed",
        "module m\n\
         resource Doc at ^docs(id: int)\n    required title: string\n    scores(who: string): int\n\n\
         fn f()\n    append(^docs(1).scores, 7)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn writing_a_keyed_leaf_the_loop_traverses_is_rejected() {
    let found = check_module(
        "loop-write-leaf",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for pos in keys(^books(1).tags)\n        ^books(1).tags(pos) = \"x\"\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn reversed_loop_mutating_the_traversed_layer_is_rejected() {
    let found = check_module(
        "loop-reversed-append-seq",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for tag in reversed(^books(1).tags)\n        append(^books(1).tags, \"x\")\n",
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
         fn f()\n    for tag in ^books(1).tags\n        append(^books(2).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn writing_a_field_in_a_record_loop_is_allowed() {
    // A two-name root loop traverses records and exposes each identity; writing a
    // scalar field of a record does not change which keys the layer holds.
    let found = check_module(
        "loop-field-write",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id, book in ^books\n        ^books(id).title = \"x\"\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn invalid_lock_targets_report_only_prototype_rejections() {
    let report = check_module_report(
        "lock-targets",
        "module m\n\
         resource Cell at ^cells(id: int)\n    required v: int\n\
         fn lockLocal()\n    var x: int = 1\n    lock x\n        x = 2\n\
         fn lockField(id: int)\n    lock ^cells(id).v\n        ^cells(id).v = 2\n\
         fn lockLiteral()\n    lock 5\n        print(\"nope\")\n",
    );
    assert_eq!(
        with_code(&report, "check.prototype_only").len(),
        3,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn lock_target_out_calls_do_not_affect_production_out_flow() {
    let report = check_module_report(
        "lock-target-out-flow",
        "module m\n\
         fn assign(out value: int)\n    value = 1\n\
         fn f(out value: int)\n    lock assign(out value)\n        return\n",
    );

    assert_eq!(
        with_code(&report, "check.prototype_only").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "check.out_parameter_assignment").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.invalid_assign_target").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn lock_statements_are_prototype_only_regardless_of_target_shape() {
    let report = check_module_report(
        "lock-targets-ok",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    notes(pos: int)\n        body: string\n\
         fn ok(id: int, pos: int)\n    lock ^books\n        print(\"root\")\n    lock ^books(id)\n        print(\"record\")\n    lock ^books(id).notes\n        print(\"layer\")\n    lock ^books(id).notes(pos)\n        ^books(id).notes(pos).body = \"x\"\n",
    );
    assert_eq!(
        with_code(&report, "check.prototype_only").len(),
        4,
        "{:#?}",
        report.diagnostics
    );
}

// --- W1 unified resolver: module-aware, visibility-aware call resolution ---

/// Check a two-module project (`src/aaa.mw` + `src/zzz.mw`), returning the whole
/// report. The two modules let a call in `zzz` be resolved against `zzz`'s own
/// declarations, `aaa`'s declarations, and any imports — exercising the
/// module-aware resolver across a real module boundary.
fn check_two_modules(name: &str, aaa: &str, zzz: &str) -> marrow_check::CheckReport {
    let root = temp_project(name, |root| {
        write(root, "src/aaa.mw", aaa);
        write(root, "src/zzz.mw", zzz);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    report
}

#[test]
fn bare_call_resolves_in_own_module_not_a_foreign_one() {
    // Two modules each declare `fn greet`. `zzz::run` calls a bare `greet()`: a
    // bare name resolves in its own module first, so it must reach `zzz::greet`
    // and check clean — never a foreign `aaa::greet`. (The runtime twin of this
    // pin proves the P1 bug, where the bare call ran `aaa::greet`.)
    let report = check_two_modules(
        "w1-bare-own-module",
        "module aaa\npub fn greet(): int\n    return 1\n",
        "module zzz\nfn greet(): int\n    return 2\nfn run(): int\n    return greet()\n",
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty()
            && with_code(&report, "check.private_function").is_empty(),
        "a bare call to a same-module function must resolve clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_bare_call_is_unresolved_not_first_match() {
    // `aaa` declares `pub fn greet`; `zzz` declares no `greet` and calls a bare
    // `greet()`. Imports bring module names, not bare names, so a cross-module
    // function is only reachable as `aaa::greet`. The bare call must be
    // `check.unresolved_call` — not silently first-matched to `aaa::greet`.
    let report = check_two_modules(
        "w1-cross-bare-unresolved",
        "module aaa\npub fn greet(): int\n    return 1\n",
        "module zzz\nfn run(): int\n    return greet()\n",
    );
    assert_eq!(
        with_code(&report, "check.unresolved_call").len(),
        1,
        "a bare cross-module call must be unresolved, not first-matched: {:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_call_to_a_private_fn_is_a_visibility_error() {
    // `aaa` declares a module-private `fn secret`; `zzz` qualifies it as
    // `aaa::secret()`. The function exists but is not `pub`, so a cross-module
    // call is a distinct visibility error (`check.private_function`), not a plain
    // unresolved call — the name resolves, the visibility does not.
    let report = check_two_modules(
        "w1-cross-private",
        "module aaa\nfn secret(): int\n    return 1\n",
        "module zzz\nfn run(): int\n    return aaa::secret()\n",
    );
    assert_eq!(
        with_code(&report, "check.private_function").len(),
        1,
        "a cross-module call to a non-pub function is a visibility error: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "a private function resolves by name, so it is not also unresolved: {:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_use_of_a_private_enum_is_a_visibility_error() {
    let root = temp_project("cross-private-enum", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             enum Hidden\n    one\n    two\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             fn f(): a::Hidden\n    return a::Hidden::one\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.private_enum");
    assert_eq!(found.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        found
            .iter()
            .all(|diagnostic| diagnostic.message.contains("a::Hidden")),
        "{found:#?}"
    );
    assert!(
        with_code(&report, "check.unknown_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_use_of_a_public_enum_checks_clean() {
    let root = temp_project("cross-public-enum", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             pub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             fn f(): a::Status\n    return a::Status::active\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn same_named_resources_constructor_resolves_by_module() {
    // Both modules declare a resource named `Book`. A constructor is the resource
    // NAME, which is module-scoped: a bare `Book(...)` in `zzz` constructs the
    // `zzz` resource. The call must type as a constructor (no unresolved call),
    // resolving by the calling module rather than first-matching `aaa::Book`.
    let report = check_two_modules(
        "w1-same-named-resource",
        "module aaa\nresource Book\n    title: string\n",
        "module zzz\nresource Book\n    title: string\nfn make(): Book\n    return Book(title: \"x\")\n",
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "a bare same-module constructor must resolve, not report unresolved: {:#?}",
        report.diagnostics
    );
}

#[test]
fn bare_call_to_a_pub_fn_in_two_modules_is_ambiguous() {
    // `aaa` and `bbb` each declare a `pub fn greet`; `zzz` declares no `greet` and
    // calls a bare `greet()`. Each is reachable only as `module::greet`, so the
    // bare name cannot pick one: a distinct `check.ambiguous_call` (qualify it),
    // not a plain unresolved call or a silent first-match to `aaa::greet`.
    let root = temp_project("w1-ambiguous-call", |root| {
        write(
            root,
            "src/aaa.mw",
            "module aaa\npub fn greet(): int\n    return 1\n",
        );
        write(
            root,
            "src/bbb.mw",
            "module bbb\npub fn greet(): int\n    return 2\n",
        );
        write(
            root,
            "src/zzz.mw",
            "module zzz\nfn run(): int\n    return greet()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert_eq!(
        with_code(&report, "check.ambiguous_call").len(),
        1,
        "a bare call to a pub fn in two modules must be ambiguous: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "an ambiguous call has candidates, so it is not also unresolved: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_enum_member_reference_checks_clean() {
    // `Status::archived` is a known member of a declared enum; using it as a
    // value must not raise an unresolved-name or unknown-member diagnostic.
    let report = check_module_report(
        "enum-member-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f()\n    const s: Status = Status::archived\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn an_enum_typed_param_and_var_annotation_is_accepted() {
    // An enum name is a valid type annotation on a parameter, a `var`, and a
    // `const`; none should be flagged `check.unknown_type`.
    let report = check_module_report(
        "enum-annotation-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status): Status\n    var t: Status = Status::active\n    return t\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn an_enum_typed_resource_field_is_accepted() {
    let report = check_module_report(
        "enum-field-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn reports_an_unknown_enum_member() {
    let found = check_module(
        "enum-unknown-member",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f()\n    const s: Status = Status::deleted\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("deleted"), "{}", found[0].message);
}

#[test]
fn reports_an_enum_resource_name_collision() {
    // An enum and a resource share the module-level declaration namespace.
    let root = temp_project("enum-resource-collision", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nenum Book\n    a\nresource Book\n    title: string\n",
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
fn the_checked_program_carries_enum_schemas() {
    let root = temp_project("enum-program", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nenum Status\n    active\n    archived\n    banned\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let status = &program.modules[0].enums[0];
    assert_eq!(status.name, "Status");
    assert_eq!(status.ordinal("banned"), Some(2));
}

#[test]
fn enum_equality_against_the_same_enum_is_accepted() {
    let report = check_module_report(
        "enum-eq-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(): bool\n    return Status::active == Status::archived\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn comparing_an_enum_to_a_string_is_an_operator_error() {
    // Nominal `==`: an enum value is comparable only with the same enum, never a
    // raw string. The mismatch is the existing operator-type diagnostic.
    let found = check_module(
        "enum-eq-string",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(): bool\n    return Status::active == \"active\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn comparing_two_different_enums_is_an_operator_error() {
    let found = check_module(
        "enum-eq-cross",
        "module m\n\
         enum Status\n    active\n\nenum Color\n    red\n\n\
         fn f(): bool\n    return Status::active == Color::red\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn enum_operator_errors_do_not_emit_untyped_return_hints() {
    let report = check_module_report(
        "enum-operator-no-untyped-cascade",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn ordered(): bool\n    return Status::active < Status::archived\n\n\
         fn added(): Status\n    return Status::active + Status::archived\n",
    );
    assert_eq!(
        with_code(&report, "check.operator_type").len(),
        2,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_exhaustive_match_over_an_enum_checks_clean() {
    let report = check_module_report(
        "match-ok",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        \
         archived\n            return\n        banned\n            return\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_nonexhaustive_match_is_a_check_error() {
    let found = check_module(
        "match-nonexhaustive",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        archived\n            return\n",
        "check.nonexhaustive_match",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("banned"), "{}", found[0].message);
}

#[test]
fn a_match_arm_for_an_unknown_member_is_a_check_error() {
    let found = check_module(
        "match-unknown-arm",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        \
         archived\n            return\n        deleted\n            return\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("deleted"), "{}", found[0].message);
}

#[test]
fn a_match_over_a_non_enum_scrutinee_is_rejected() {
    let found = check_module(
        "match-non-enum",
        "module m\n\
         fn f(n: int)\n    \
         match n\n        active\n            return\n",
        "check.match_requires_enum",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_duplicate_match_arm_is_a_check_error() {
    let found = check_module(
        "match-duplicate-arm",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        \
         active\n            return\n        archived\n            return\n",
        "check.duplicate_match_arm",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_match_over_a_modules_own_same_named_enum_checks_clean() {
    // Two modules each declare an enum `Status`, with different members. Module
    // `b`'s function matches its own `Status` (members `open`/`closed`)
    // exhaustively. Enum identity is module-qualified, so the checker validates
    // the match against `b::Status`, not the first project-wide `Status`
    // (`a::Status`, `active`/`archived`). Resolving by bare name would read
    // `a::Status`'s members and falsely reject `b`'s match as nonexhaustive with
    // unknown arms.
    let root = temp_project("enum-same-name-match", |root| {
        write(
            root,
            "src/a.mw",
            "module a\nenum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             enum Status\n    open\n    closed\n\n\
             fn classify(s: Status): int\n    \
             match s\n        open\n            return 1\n        \
             closed\n            return 2\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn passing_one_enum_where_a_different_enum_is_expected_is_a_check_error() {
    // `classify(s: Status)` is called with a `Color` value. Nominal identity:
    // enum `Color` is not enum `Status`, so the argument is a real mismatch, not
    // silently accepted.
    let found = check_module(
        "enum-arg-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn classify(s: Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n\n\
         fn caller(): int\n    return classify(Color::green)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_scalar_where_an_enum_is_expected_is_a_check_error() {
    // A raw scalar into an enum parameter is a mismatch: the parameter is `Status`,
    // the argument is `int`.
    let found = check_module(
        "enum-arg-scalar",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn classify(s: Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n\n\
         fn caller(): int\n    return classify(3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn returning_a_different_enum_than_declared_is_a_check_error() {
    let found = check_module(
        "enum-return-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f(): Status\n    return Color::red\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn assigning_a_different_enum_into_an_enum_local_is_a_check_error() {
    let found = check_module(
        "enum-assign-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f()\n    var s: Status = Status::active\n    s = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn assignment_between_same_named_enums_qualifies_the_message() {
    let root = temp_project("enum-same-name-assign-message", |root| {
        write(root, "src/a.mw", "module a\npub enum Color\n    red\n");
        write(root, "src/b.mw", "module b\npub enum Color\n    blue\n");
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn f()\n    var c: a::Color = a::Color::red\n    c = b::Color::blue\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.assignment_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        found[0].message.contains("a::Color") && found[0].message.contains("b::Color"),
        "{}",
        found[0].message
    );
}

#[test]
fn writing_a_different_enum_into_an_enum_saved_field_is_a_check_error() {
    // The saved field `state: Status` is written a `Color` value: a nominal
    // mismatch at the saved-field write boundary.
    let found = check_module(
        "enum-field-write-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f()\n    ^orders(1).state = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_qualified_enum_saved_field_declaration_checks_clean() {
    let root = temp_project("qualified-enum-saved-field", |root| {
        write(
            root,
            "src/pkg/kinds.mw",
            "module pkg::kinds\n\nenum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\n\nuse pkg::kinds\n\nresource Saved at ^saved(id: int)\n    required k: kinds::Color\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn reading_an_enum_saved_field_types_as_that_enum() {
    // A read of `^orders(1).state` (an enum-typed saved field) must type as
    // `Status`: comparing it against the *same* enum is clean. Before the field
    // read was typed it was `Unknown`, so a nominal `==` against any enum reported
    // an operator error — this same-enum comparison was wrongly rejected.
    let report = check_module_report(
        "enum-field-read-eq-same",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f(): bool\n    return ^orders(1).state == Status::active\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    // And typing as `Status` means a `==` against a *different* enum is rejected.
    let found = check_module(
        "enum-field-read-eq-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f(): bool\n    return ^orders(1).state == Color::red\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_match_over_an_enum_saved_field_enforces_exhaustiveness() {
    // A match over a saved enum field `^orders(1).state` must resolve to `Status`
    // and require every member. Missing `banned` is a check error, not a silently
    // skipped match that faults at runtime.
    let found = check_module(
        "enum-field-read-match",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f()\n    \
         match ^orders(1).state\n        active\n            return\n        archived\n            return\n",
        "check.nonexhaustive_match",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("banned"), "{}", found[0].message);
}

#[test]
fn a_singleton_keyed_enum_leaf_read_types_as_that_enum() {
    let report = check_module_report(
        "enum-singleton-keyed-leaf-read",
        "module m\n\
         enum Kind\n    number\n    plus\n\n\
         resource Session at ^session\n    required cursor: int\n    kinds(pos: int): Kind\n\n\
         fn readBack(): int\n    \
         var k: Kind = ^session.kinds(1)\n    \
         match ^session.kinds(1)\n        number\n            return 0\n        plus\n            return 1\n",
    );
    assert!(
        !report.has_errors(),
        "a keyed enum leaf under a singleton saved root must read as its enum: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_nonexhaustive_match_over_a_qualified_enum_scrutinee_is_a_check_error() {
    // `s: b::Status` is a qualified enum annotation. The match over it must resolve
    // to `b::Status` and enforce exhaustiveness; missing `closed` is a check error,
    // not a runtime crash from an unresolved scrutinee that passed open.
    let root = temp_project("enum-qualified-nonexhaustive", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             fn classify(s: b::Status): int\n    \
             match s\n        open\n            return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.nonexhaustive_match");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(found[0].message.contains("closed"), "{}", found[0].message);
}

#[test]
fn passing_a_third_modules_enum_to_a_qualified_parameter_is_a_check_error() {
    // Module `c` calls `b::classify`, whose parameter is `b::Status`, with
    // `a::Status`. Three modules, three same-or-different enums: only `b::Status`
    // is accepted. Passing `a::Status` is a nominal mismatch.
    let root = temp_project("enum-third-module-arg", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n\n\
             pub fn classify(s: Status): int\n    \
             match s\n        open\n            return 1\n        closed\n            return 2\n",
        );
        write(
            root,
            "src/c.mw",
            "module c\nuse a\nuse b\n\
             fn run(): int\n    return b::classify(a::Status::active)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_bare_foreign_only_enum_annotation_resolves_to_the_real_owner_not_a_phantom() {
    // Module `a` declares `Status`; module `b` does not. A bare `Status` annotation
    // in `b` must resolve to the real owner `a::Status` — the same enum a bare
    // `Status::member` literal resolves to there — not a phantom `b::Status` minted
    // by stamping the referencing module onto a project-wide name (the F3 hole).
    //
    // Proof of correct identity: in `b`, `s == Status::active` (both the
    // annotation and the literal name the real `a::Status`) checks clean, and a
    // `match s` reads `a::Status`'s members exhaustively. A phantom `b::Status`
    // would own no members, so the literal `Status::active` would resolve to
    // `a::Status` while `s` carried `b::Status`, making the `==` a cross-enum
    // operator error — exactly the false rejection a phantom causes.
    let root = temp_project("enum-foreign-real-owner", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             fn same(s: Status): bool\n    return s == Status::active\n\n\
             fn classify(s: Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        !report.has_errors(),
        "a bare foreign-only enum annotation must resolve to the real owner, not a phantom: {:#?}",
        report.diagnostics
    );
}

#[test]
fn passing_a_foreign_enum_to_a_qualified_parameter_is_a_check_error() {
    // `b::dispatch(s: b::Status)` annotates its parameter with the *qualified*
    // `b::Status`. Per-file resolution sees only module `b`'s own enum names, so a
    // qualified `b::Status` slot is left `Unknown` until the whole program is
    // assembled — the argument gate must still fire after the slot is stamped with
    // its true owner. Calling it with `a::Color::green` is a nominal mismatch
    // (`Color` is not `Status`), not a silently dispatched wrong value.
    let root = temp_project("enum-qualified-arg-cross", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn run(): int\n    return b::dispatch(a::Color::green)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn passing_a_raw_scalar_to_a_qualified_enum_parameter_is_a_check_error() {
    // The same qualified `b::dispatch(s: b::Status)` slot, called with a raw `int`.
    // A scalar in an enum slot is a concrete mismatch the argument gate must catch
    // once the cross-module parameter carries its real enum identity.
    let root = temp_project("enum-qualified-arg-scalar", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse b\n\
             fn run(): int\n    return b::dispatch(1)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_enum_through_a_relay_chain_to_a_qualified_parameter_is_a_check_error() {
    // A three-module relay: `app` calls `mid::relay`, whose parameter is the
    // qualified `b::Status`. Passing `a::Color::green` through the relay is a
    // nominal mismatch the argument gate must catch in `mid`, even though `mid`'s
    // file resolved `b::Status` to `Unknown` before the program was whole.
    let root = temp_project("enum-relay-chain-arg", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/leaf.mw",
            "module leaf\nuse b\n\
             pub fn sink(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/mid.mw",
            "module mid\nuse b\nuse leaf\n\
             pub fn relay(s: b::Status): int\n    return leaf::sink(s)\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse mid\n\
             fn run(): int\n    return mid::relay(a::Color::green)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_enum_to_a_qualified_parameter_in_an_equality_body_is_a_check_error() {
    // `b::isActive(s: b::Status): bool` compares its qualified-enum parameter to
    // `b::Status::active`. Called with `a::Color::red`, the argument is a nominal
    // mismatch the gate must catch — the qualified parameter's identity, recovered
    // once the program is whole, drives the comparison.
    let root = temp_project("enum-qualified-arg-eq", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn isActive(s: b::Status): bool\n    return s == b::Status::active\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn run(): bool\n    return b::isActive(a::Color::red)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_enum_to_a_qualified_parameter_inside_a_loop_is_a_check_error() {
    // The same qualified-enum argument mismatch inside a `for` loop body: each
    // iteration's call is checked, so the nominal mismatch is reported once.
    let root = temp_project("enum-qualified-arg-loop", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn run()\n    for i in 1..3\n        b::dispatch(a::Color::green)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn passing_the_matching_enum_to_a_qualified_parameter_checks_clean() {
    // The clean counterpart: `b::dispatch(s: b::Status)` called with the matching
    // `b::Status::active`. The argument gate must accept a like-for-like enum across
    // the module boundary, not over-reject once the slot carries its real owner.
    let root = temp_project("enum-qualified-arg-clean", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse b\n\
             fn run(): int\n    return b::dispatch(b::Status::active)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        !report.has_errors(),
        "a matching cross-module enum argument must check clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_match_over_a_sequence_enum_element_enforces_its_identity() {
    // A `sequence[Status]` element carries `Status`: iterating it binds the loop
    // variable to that enum, so a `match` over it is dispatched against `Status`'s
    // members. Arms naming a *different* enum's members (`Color`'s `red`/`green`)
    // are then unknown `Status` members — a check error. Without recursing the
    // element through enum resolution the element binds `Unknown`, the match is
    // left alone as an unresolved scrutinee, and the foreign arms pass open: a
    // silent loss of identity over a sequence of enums.
    let found = check_module(
        "enum-sequence-element-foreign",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f(items: sequence[Status])\n    \
         for s in items\n        \
         match s\n            red\n                return\n            green\n                return\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn a_const_annotated_with_one_enum_and_a_different_enum_value_is_a_check_error() {
    // The const-annotation place is an enum; the initializer is a different enum.
    let found = check_module(
        "enum-const-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f()\n    const s: Status = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_qualified_enum_var_annotation_accepts_the_same_qualified_member() {
    // A qualified `var t: b::Status` annotation accepts a `b::Status::open` value:
    // the annotation and the qualified member literal name the same enum, so the
    // initializer checks clean. (Proves qualified annotation + qualified member
    // value resolve to the same nominal identity.)
    let root = temp_project("enum-qualified-var-ok", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             fn f()\n    var t: b::Status = b::Status::open\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_match_over_a_qualified_member_typed_local_dispatches_clean() {
    // A `const s: b::Status = b::Status::open` then an exhaustive `match s` over
    // `b::Status` checks clean: the qualified member literal types the local as
    // `b::Status`, so the match resolves and is exhaustive.
    let root = temp_project("enum-qualified-member-match", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             fn f(): int\n    const s: b::Status = b::Status::open\n    \
             match s\n        open\n            return 1\n        closed\n            return 2\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A nested-module enum `module a::b` owns `Status` and `Color`. Its module name
/// has *two* segments (`a::b`), so a qualified annotation `a::b::Status` and a
/// qualified literal `a::b::Color::red` are four-segment paths. The module/enum
/// split must keep all-but-the-last segment as the module (`a::b`), not the first
/// (`a`) — otherwise the slot stays `Unknown` and every boundary fails open.
fn nested_module_sources(root: &Path) {
    write(
        root,
        "src/a/b.mw",
        "module a::b\n\
         pub enum Status\n    active\n    archived\n\n\
         pub enum Color\n    red\n    green\n\n\
         pub fn take(s: a::b::Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n",
    );
}

#[test]
fn passing_a_nested_module_wrong_enum_to_a_qualified_parameter_is_a_check_error() {
    // `a::b::take(s: a::b::Status)` called with `a::b::Color::red`: enum `Color`
    // is not enum `Status`, a nominal mismatch. A first-separator split would make
    // the parameter `Unknown` (module "a", enum "b::Status" matches nothing), so
    // the wrong enum would pass with zero diagnostics.
    let root = temp_project("enum-nested-arg-cross", |root| {
        nested_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    return a::b::take(a::b::Color::red)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn passing_a_raw_scalar_to_a_nested_module_enum_parameter_is_a_check_error() {
    // The same `a::b::take(s: a::b::Status)` slot, called with a raw `int`. The
    // nested-module parameter must carry its real enum identity so the scalar is a
    // concrete mismatch, not silently accepted.
    let root = temp_project("enum-nested-arg-scalar", |root| {
        nested_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    return a::b::take(1)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn returning_a_wrong_enum_from_a_nested_module_function_is_a_check_error() {
    // A function declared `: a::b::Status` returns `a::b::Color::red`. The return
    // slot must resolve to `a::b::Status` (nested module kept whole), so returning
    // a `Color` is a nominal mismatch rather than an unresolved slot accepting any
    // value.
    let root = temp_project("enum-nested-return-cross", |root| {
        write(
            root,
            "src/a/b.mw",
            "module a::b\n\
             pub enum Status\n    active\n    archived\n\n\
             pub enum Color\n    red\n    green\n\n\
             fn f(): a::b::Status\n    return a::b::Color::red\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.return_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn assigning_a_wrong_enum_into_a_nested_module_enum_local_is_a_check_error() {
    // A `var s: a::b::Status` local is assigned `a::b::Color::red`. The annotation
    // must resolve to `a::b::Status` so the cross-enum assignment is caught.
    let root = temp_project("enum-nested-assign-cross", |root| {
        write(
            root,
            "src/a/b.mw",
            "module a::b\n\
             pub enum Status\n    active\n    archived\n\n\
             pub enum Color\n    red\n    green\n\n\
             fn f()\n    var s: a::b::Status = a::b::Status::active\n    s = a::b::Color::red\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.assignment_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_nonexhaustive_match_over_a_nested_module_enum_scrutinee_is_a_check_error() {
    // `s: a::b::Status` is a nested-module qualified annotation. The match over it
    // must resolve to `a::b::Status` and enforce exhaustiveness; missing `archived`
    // is a check error, not a runtime crash from a scrutinee that passed open.
    let root = temp_project("enum-nested-nonexhaustive", |root| {
        write(
            root,
            "src/a/b.mw",
            "module a::b\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn classify(s: a::b::Status): int\n    \
             match s\n        active\n            return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.nonexhaustive_match");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        found[0].message.contains("archived"),
        "{}",
        found[0].message
    );
}

#[test]
fn an_unknown_member_of_a_nested_module_enum_literal_is_a_check_error() {
    // `a::b::Status::bogus` names a real nested-module enum but an unknown member.
    // The four-segment literal must resolve enum=Status in module=a::b, then report
    // the missing member — not type `Unknown` and pass silently.
    let root = temp_project("enum-nested-unknown-member", |root| {
        write(
            root,
            "src/a/b.mw",
            "module a::b\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    const s: a::b::Status = a::b::Status::bogus\n    return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.unknown_enum_member");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(found[0].message.contains("bogus"), "{}", found[0].message);
}

#[test]
fn passing_the_matching_nested_module_enum_checks_clean() {
    // The clean counterpart: `a::b::take(s: a::b::Status)` called with the matching
    // `a::b::Status::active`. A like-for-like nested-module enum argument must check
    // clean — the fix must not over-reject once the slot carries its real owner.
    let root = temp_project("enum-nested-arg-clean", |root| {
        nested_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    return a::b::take(a::b::Status::active)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        !report.has_errors(),
        "a matching nested-module enum argument must check clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_module_less_script_string_into_an_int_field_is_a_check_error() {
    // A file with no `module` line is a single-file script. Its own `^orders`
    // resource must still be nominally checked: storing a `string` into the
    // `int` field `count` is a type mismatch, not a silently-accepted write.
    let found = check_script(
        "script-string-into-int",
        "resource Order at ^orders(id: int)\n    required count: int\n\n\
         pub fn main()\n    var o: Order\n    o.count = \"alsobad\"\n    ^orders(1) = o\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_less_script_string_into_an_enum_field_is_a_check_error() {
    // The enum counterpart: a script's enum-typed field `state: Status` written a
    // raw `string`. The field type resolves to the script's own `Status`, so the
    // mismatch is caught rather than dropping to `Unknown` and passing.
    let found = check_script(
        "script-string-into-enum",
        "enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         pub fn main()\n    var o: Order\n    o.state = \"notamember\"\n    ^orders(1) = o\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_less_script_self_reference_checks_clean() {
    // The over-rejection guard: once a script's own types become visible, a
    // correct script must still check clean. Its resource, its enum-typed field,
    // and a same-enum comparison all resolve to the script's own declarations.
    let root = temp_project("script-self-reference-clean", |root| {
        write(
            root,
            "src/app.mw",
            "enum Status\n    active\n    archived\n\n\
             resource Order at ^orders(id: int)\n    required state: Status\n\n\
             pub fn main()\n    var o: Order\n    o.state = Status::active\n    \
             ^orders(1) = o\n\n\
             pub fn isActive(): bool\n    return ^orders(1).state == Status::active\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        !report.has_errors(),
        "a correct module-less script must check clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn another_module_cannot_use_a_module_less_script() {
    // The import-safety invariant: a script is self-resolvable but un-importable.
    // A sibling `module other` that does `use app` against a module-less `app.mw`
    // must still fail with `check.unresolved_import` — the empty-named script is
    // never bound to a name a `use` can spell.
    let root = temp_project("script-not-importable", |root| {
        write(
            root,
            "src/app.mw",
            "resource Order at ^orders(id: int)\n    required count: int\n\n\
             pub fn main()\n    print(\"hi\")\n",
        );
        write(
            root,
            "src/other.mw",
            "module other\nuse app\n\npub fn run()\n    print(\"ok\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.unresolved_import");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(found[0].message.contains("app"), "{}", found[0].message);
}

#[test]
fn a_module_less_script_joins_the_program_under_the_empty_name() {
    // Pins the construction: a parse-clean script enters `program.modules` under
    // the empty module name, carrying its own resources, so the nominal resolvers
    // (which scan `program.modules`) can see `Order`. This is what turns the
    // script's field types from `Unknown` into its real types.
    let root = temp_project("script-empty-named-module", |root| {
        write(
            root,
            "src/app.mw",
            "resource Order at ^orders(id: int)\n    required count: int\n\n\
             pub fn main()\n    print(\"hi\")\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let script = program
        .modules
        .iter()
        .find(|module| module.name.is_empty())
        .expect("the module-less script joins the program under the empty name");
    assert!(
        script.resources.iter().any(|r| r.name == "Order"),
        "the script's own resource is present for nominal resolution"
    );
}

#[test]
fn two_module_less_scripts_are_a_check_error() {
    // The soundness fix: a project may hold at most one module-less file (its
    // single entrypoint script). Two scripts share the empty module name, so a
    // bare reference in one could resolve against the other's declarations. Rather
    // than alias them, the checker rejects every module-less file past the first —
    // a project's library files must declare a `module`.
    let root = temp_project("two-scripts-rejected", |root| {
        write(
            root,
            "src/one.mw",
            "resource Order at ^orders(id: int)\n    required count: int\n\n\
             pub fn main()\n    print(\"one\")\n",
        );
        write(
            root,
            "src/two.mw",
            "resource Ticket at ^tickets(id: int)\n    required note: string\n\n\
             pub fn other()\n    print(\"two\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    let found = with_code(&report, "check.multiple_scripts");
    // Each offending file is named; neither is privileged over the other.
    assert_eq!(found.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        found.iter().all(|d| d.message.contains("module")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|d| d.file.ends_with("one.mw"))
            && found.iter().any(|d| d.file.ends_with("two.mw")),
        "{found:#?}"
    );
}

#[test]
fn two_scripts_with_clashing_resources_never_silently_bind_to_the_wrong_shape() {
    // The wrong-resource-binding repro: each script declares its own `Order` with a
    // different shape (`one.mw`'s has `count`, `two.mw`'s has `priority`). Under the
    // empty-name alias, `two.mw`'s `var o: Order` could bind to `one.mw`'s `Order`,
    // and assigning a field only `two.mw`'s `Order` has would either silently accept
    // against the wrong shape or corrupt at run time. Rejecting the second script
    // makes that impossible: the binding never happens because the file is an error.
    let root = temp_project("two-scripts-wrong-shape", |root| {
        write(
            root,
            "src/one.mw",
            "resource Order at ^orders_a(id: int)\n    required count: int\n\n\
             pub fn main()\n    var o: Order\n    o.count = 1\n    ^orders_a(1) = o\n",
        );
        write(
            root,
            "src/two.mw",
            "resource Order at ^orders_b(id: int)\n    required priority: int\n\n\
             pub fn other()\n    var o: Order\n    o.priority = 9\n    ^orders_b(1) = o\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        !with_code(&report, "check.multiple_scripts").is_empty(),
        "two scripts with clashing resources must be rejected, never silently bound: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_script_cannot_see_another_scripts_functions() {
    // The cross-script call repro: `b.mw` calls `helper`, declared only in `a.mw`.
    // Under the empty-name alias `b` could resolve `helper` against `a`'s module —
    // false cross-script visibility. With the scripts rejected, the call cannot
    // resolve across the file boundary.
    let root = temp_project("cross-script-call", |root| {
        write(
            root,
            "src/a.mw",
            "pub fn helper(): int\n    return 1\n\npub fn main()\n    print(\"a\")\n",
        );
        write(root, "src/b.mw", "pub fn other()\n    var x = helper()\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        !with_code(&report, "check.multiple_scripts").is_empty(),
        "b cannot see a's functions; the two-script project is rejected: {:#?}",
        report.diagnostics
    );
}
