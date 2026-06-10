mod support;

use std::path::Path;

use marrow_check::check_project;
use marrow_project::parse_config;

use support::{config, temp_project, write};

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

    let overlaid = analyze_project(&root, &config(), &sources, None).expect("analyze");
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

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");

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

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

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

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");

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

    let snapshot = analyze_project(&root, &cfg, &sources, None).expect("analyze");

    let analyzed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("snapshot retains configured overlay test file");
    assert_eq!(analyzed.module_name.as_deref(), Some("tests::new_test"));
    assert_eq!(analyzed.source, source);
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

    let snapshot =
        analyze_project(&root, &config(), &ProjectSources::new(), None).expect("analyze");

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
