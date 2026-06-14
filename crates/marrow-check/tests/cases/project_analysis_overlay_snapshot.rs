use crate::support;
use std::fs;
use std::path::Path;

use marrow_check::{AnalysisSnapshot, ProjectSources, analyze_project, check_project};
use marrow_project::parse_config;

use support::{config, temp_project, write};

fn content_identity(snapshot: &AnalysisSnapshot) -> String {
    snapshot.content_identity().as_str().to_string()
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
fn source_digest_binds_overlay_source_instead_of_disk() {
    let disk_source = "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n";
    let overlay_source = "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         store ^books(id: int): Book\n";
    let root = temp_project("overlay-digest", |root| {
        write(root, "src/m.mw", disk_source);
    });
    let path = root.join("src/m.mw");

    let disk = check_project(&root, &config())
        .expect("disk check")
        .1
        .source_digest();
    let overlaid = analyze_project(
        &root,
        &config(),
        &ProjectSources::new().with(&path, overlay_source),
        None,
    )
    .expect("overlay analyze")
    .program
    .source_digest();
    let overlay_expected = check_project(
        &temp_project("overlay-digest-expected", |root| {
            write(root, "src/m.mw", overlay_source);
        }),
        &config(),
    )
    .expect("overlay expected check")
    .1
    .source_digest();

    assert_ne!(disk, overlay_expected);
    assert_eq!(overlaid, overlay_expected);
}

#[test]
fn analyze_project_includes_unsaved_source_root_files() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-unsaved-source-file", |root| {
        fs::create_dir_all(root.join("src")).expect("create src");
    });
    let path = root.join("src/new_file.mw");
    let sources =
        ProjectSources::new().with(&path, "module new_file\nfn f()\n    var x: int = \"str\"\n");

    let snapshot = analyze_project(&root, &config(), &sources, None).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.assignment_type" && d.file == path),
        "unsaved source-root files should be checked: {:#?}",
        snapshot.report.diagnostics
    );
    let analyzed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("snapshot retains unsaved source-root file");
    assert_eq!(analyzed.module_name.as_deref(), Some("new_file"));
}

#[test]
fn analyze_project_reports_configured_test_file_parse_errors() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-test-parse", |root| {
        write(root, "src/app.mw", "module app\n");
        // A tab is a lexical error.
        write(root, "tests/bad_test.mw", "pub fn t()\n\tapp::noop()\n");
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");

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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
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
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
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
fn analyze_project_keeps_configured_test_modules_out_of_program_facts() {
    let root = temp_project("analyze-test-facts-boundary", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             pub fn takes_int(n: int)\n\
             \x20   if exists(^books(n).subtitle)\n\
             \x20       print(^books(n).subtitle)\n",
        );
        write(
            root,
            "tests/smoke_test.mw",
            "fn smoke()\n\
             \x20   app::takes_int(\"bad\")\n",
        );
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
    let test_path = root.join("tests/smoke_test.mw");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"
                && diagnostic.file == test_path),
        "configured test diagnostics should still be reported: {:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot.files.iter().any(|file| file.path == test_path
            && file.module_name.as_deref() == Some("tests::smoke_test")),
        "configured test files should still be retained: {:#?}",
        snapshot.files
    );
    assert!(
        !snapshot
            .program
            .modules
            .iter()
            .any(|module| module.name == "tests::smoke_test"),
        "configured test modules must not remain in the returned source program"
    );
    assert!(
        snapshot
            .program
            .facts
            .module_id("tests::smoke_test")
            .is_none(),
        "configured test facts must not remain in the returned source program"
    );
    assert!(
        !snapshot.program.facts.presence_proofs().is_empty(),
        "{:#?}",
        snapshot.program.facts.presence_proofs()
    );
}

#[test]
fn analyze_project_retains_unsaved_configured_test_files_in_snapshot() {
    use marrow_check::{ProjectSources, analyze_project};

    let root = temp_project("analyze-unsaved-test-snapshot", |root| {
        write(root, "src/app.mw", "module app\n");
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");
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

#[test]
fn analysis_content_identity_tracks_analyzed_sources_and_config() {
    let root = temp_project("analysis-content-identity", |root| {
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    print(1)\n",
        );
        write(root, "tests/smoke.mw", "fn smoke()\n    app::main()\n");
        fs::create_dir_all(root.join("empty")).expect("create empty source root");
    });
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("config");

    let baseline =
        analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("baseline analyze");
    let repeated =
        analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("repeat analyze");
    let baseline_identity = content_identity(&baseline);
    assert_eq!(baseline_identity, content_identity(&repeated));
    assert!(baseline_identity.starts_with("sha256:"));

    let accepted = marrow_catalog::CatalogMetadata::new(99, Vec::new());
    let with_accepted = analyze_project(&root, &cfg, &ProjectSources::new(), Some(&accepted))
        .expect("accepted analyze");
    assert_eq!(baseline_identity, content_identity(&with_accepted));

    let source_path = root.join("src/app.mw");
    let edited_source =
        ProjectSources::new().with(&source_path, "module app\npub fn main()\n    print(2)\n");
    let source_edit =
        analyze_project(&root, &cfg, &edited_source, None).expect("source edit analyze");
    assert_ne!(baseline_identity, content_identity(&source_edit));

    let test_path = root.join("tests/smoke.mw");
    let edited_test =
        ProjectSources::new().with(&test_path, "fn smoke()\n    const n = 2\n    app::main()\n");
    let test_edit = analyze_project(&root, &cfg, &edited_test, None).expect("test edit analyze");
    assert_ne!(baseline_identity, content_identity(&test_edit));

    let config_edit = parse_config(
        r#"{ "sourceRoots": ["src", "empty"], "tests": ["tests"], "store": { "backend": "memory" } }"#,
    )
    .expect("config edit");
    let config_edit =
        analyze_project(&root, &config_edit, &ProjectSources::new(), None).expect("config analyze");
    assert_ne!(baseline_identity, content_identity(&config_edit));

    let moved_root = temp_project("analysis-content-identity-moved", |root| {
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    print(1)\n",
        );
        write(root, "tests/smoke.mw", "fn smoke()\n    app::main()\n");
    });
    let moved =
        analyze_project(&moved_root, &cfg, &ProjectSources::new(), None).expect("moved analyze");
    assert_eq!(baseline_identity, content_identity(&moved));
}
