use std::fs;

use crate::support;
use serde_json::Value;
use support::{temp_project_uncommitted as temp_project, write};

fn run_check(args: &[&str]) -> std::process::Output {
    support::marrow_sub("check", args)
}

fn assert_has_code(records: &[Value], code: &str) {
    assert!(support::codes(records).contains(&code), "{records:#?}");
}

fn assert_lacks_code(records: &[Value], code: &str) {
    assert!(!support::codes(records).contains(&code), "{records:#?}");
}

fn assert_has_file(records: &[Value], suffix: &str) {
    assert!(
        records.iter().any(|record| record["source_span"]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with(suffix))),
        "{records:#?}"
    );
}

#[test]
fn checks_a_clean_project_directory() {
    let root = temp_project("proj-clean", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_eq!(records.last().unwrap()["kind"], "summary");
    assert_eq!(records.last().unwrap()["status"], "ok");
}

#[test]
fn format_json_reports_canonical_absolute_project_for_relative_path() {
    let root = temp_project("proj-json-canonical-project", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let cwd = root.parent().expect("temp project parent");
    let relative = root
        .file_name()
        .expect("temp project name")
        .to_str()
        .expect("utf8 project name");
    let output = support::marrow_sub_in(cwd, "check", &["--format", "json", relative]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = support::json(output.stdout);
    let expected = fs::canonicalize(&root)
        .expect("canonical project path")
        .display()
        .to_string();
    assert_eq!(value["project"], serde_json::json!(expected));
}

#[test]
fn reports_project_check_as_jsonl() {
    let root = temp_project("proj-jsonl", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let records = support::jsonl(output.stdout);
    assert_eq!(records[0]["code"], "check.module_path");
    assert_eq!(records.last().unwrap()["kind"], "summary");
    assert_eq!(records.last().unwrap()["status"], "failed");
}

#[test]
fn pending_catalog_intent_reports_declaration_spans() {
    let root = support::temp_project("proj-catalog-intent-spans", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/books.mw",
            "module books\n\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n",
        );
    });
    write(
        &root,
        "src/books.mw",
        "module books\n\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
    );
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = support::jsonl(output.stdout);
    let lines: Vec<i64> = records
        .iter()
        .filter(|record| record["code"] == "check.catalog_intent")
        .map(|record| record["source_span"]["line"].as_i64().expect("line"))
        .collect();
    assert_eq!(lines, vec![5], "{records:#?}");
}

#[test]
fn warning_only_project_check_prints_stable_summary() {
    let root = support::temp_project("proj-warning-summary", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/books.mw",
            "module books\n\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n",
        );
    });
    write(
        &root,
        "src/books.mw",
        "module books\n\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
    );
    let output = run_check(&[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ok: checked (1 warning)\n");
}

#[test]
fn rejects_duplicate_format_flag() {
    let output = run_check(&["--format", "json", "--format", "text", "missing.mw"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}

#[test]
fn surfaces_a_parse_error_in_a_project_file_with_its_path() {
    let root = temp_project("proj-parse", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        // A tab is a lexical error.
        write(root, "src/bad.mw", "module bad\n\tconst X: int = 1\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_file(&records, "bad.mw");
}

#[test]
fn project_check_reports_parse_errors_in_configured_tests() {
    let root = temp_project("proj-test-parse", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn ok(): int\n    return 1\n",
        );
        write(
            root,
            "tests/broken_test.mw",
            "pub fn broken()\n    var for: int = 1\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_file(&records, "broken_test.mw");
}

#[test]
fn project_check_reports_type_errors_in_configured_tests() {
    let root = temp_project("proj-test-type", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn ok(): int\n    return 1\n",
        );
        write(
            root,
            "tests/t_test.mw",
            "pub fn bad(): int\n    return \"nope\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "check.return_type");
    assert_has_file(&records, "t_test.mw");
}

#[test]
fn surfaces_a_parse_error_in_configured_test_files() {
    let root = temp_project("proj-test-parse-tab", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(root, "src/app.mw", "module app\n");
        // A tab is a lexical error.
        write(root, "tests/bad_test.mw", "pub fn t()\n\tapp::noop()\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_file(&records, "bad_test.mw");
}

#[test]
fn reports_configured_test_files_when_source_files_have_errors() {
    let root = temp_project("proj-test-source-error", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\nfn f()\n    var x: int = \"str\"\n",
        );
        // A tab is a lexical error.
        write(root, "tests/bad_test.mw", "pub fn t()\n\treturn\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "check.assignment_type");
    assert_has_code(&records, "parse.syntax");
    assert_has_file(&records, "bad_test.mw");
}

#[test]
fn suppresses_configured_test_resolution_noise_when_source_parse_fails() {
    let root = temp_project("proj-test-incomplete-source", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
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
        write(
            root,
            "tests/smoke_test.mw",
            "use app\nfn smoke()\n    app::f()\n    var b: Book\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.assignment_type");
    assert_lacks_code(&records, "check.unresolved_import");
    assert_lacks_code(&records, "check.unresolved_call");
    assert_lacks_code(&records, "check.unknown_type");
}

#[test]
fn keeps_configured_test_local_resolution_diagnostics_when_source_parse_fails() {
    let root = temp_project("proj-test-local-errors-incomplete-source", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(root, "src/app.mw", "module app\n\tfn f()\n");
        write(
            root,
            "tests/smoke_test.mw",
            "use std::definitely_missing\n\
             fn smoke()\n\
             \x20   tests::helper::missing()\n\
             \x20   missing_local()\n\
             \x20   var n: NotAType\n\
             \x20   var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "check.unresolved_import");
    assert_has_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.unknown_type");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn keeps_configured_test_local_bare_call_matching_hidden_source_module() {
    let root = temp_project("proj-test-local-bare-call-incomplete-source", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(root, "src/app.mw", "module app\n\tfn f()\n");
        write(
            root,
            "tests/smoke_test.mw",
            "fn smoke()\n    app()\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn keeps_configured_test_local_submodule_import_matching_hidden_source_prefix() {
    let root = temp_project(
        "proj-test-local-submodule-import-incomplete-source",
        |root| {
            write(
                root,
                "marrow.json",
                r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
            );
            write(root, "src/app.mw", "module app\n\tfn f()\n");
            write(
                root,
                "tests/smoke_test.mw",
                "use app::missing\nfn smoke()\n    var y: int = \"str\"\n",
            );
        },
    );
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unresolved_import");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn keeps_configured_test_local_unresolved_call_when_another_test_has_parse_error() {
    let root = temp_project("proj-test-local-call-with-broken-sibling-test", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    return\n",
        );
        write(root, "tests/a_bad_test.mw", "fn broken()\n\treturn\n");
        write(
            root,
            "tests/b_smoke_test.mw",
            "fn smoke()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.unknown_type");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn suppresses_unresolved_import_when_broken_configured_test_is_imported() {
    let root = temp_project("proj-test-import-broken-sibling-test", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\npub fn main()\n    return\n",
        );
        write(root, "tests/helper.mw", "fn helper()\n\treturn\n");
        write(
            root,
            "tests/smoke_test.mw",
            "use tests::helper\nfn smoke()\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_lacks_code(&records, "check.unresolved_import");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn ignores_declared_modules_in_broken_configured_tests_for_call_suppression() {
    let root = temp_project(
        "proj-test-local-call-with-broken-declared-module-test",
        |root| {
            write(
                root,
                "marrow.json",
                r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
            );
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
            write(
                root,
                "tests/b_smoke_test.mw",
                "fn smoke()\n    app::missing()\n    var y: int = \"str\"\n",
            );
        },
    );
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn keeps_source_module_calls_when_broken_test_path_collides() {
    let root = temp_project("proj-test-path-collides-with-source-module", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/tests/app.mw",
            "module tests::app\npub fn main()\n    return\n",
        );
        write(root, "tests/app.mw", "fn broken()\n\treturn\n");
        write(
            root,
            "tests/b_smoke_test.mw",
            "fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn keeps_test_module_calls_when_broken_source_path_collides() {
    let root = temp_project("proj-source-path-collides-with-test-module", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(root, "src/tests/app.mw", "module tests::app\n\tfn f()\n");
        write(root, "tests/app.mw", "fn existing()\n    return\n");
        write(
            root,
            "tests/b_smoke_test.mw",
            "fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn reports_duplicate_when_test_module_collides_with_source_module() {
    let root = temp_project("proj-test-module-duplicates-source-module", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/tests/app.mw",
            "module tests::app\npub fn sourceOnly()\n    return\n",
        );
        write(root, "tests/app.mw", "pub fn testOnly()\n    return\n");
        write(
            root,
            "tests/b_smoke_test.mw",
            "fn smoke()\n    tests::app::testOnly()\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "check.duplicate_module");
    assert_lacks_code(&records, "check.unresolved_call");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn suppresses_unknown_types_from_broken_configured_test_declarations() {
    let root = temp_project("proj-test-type-from-broken-sibling-test", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
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
        write(
            root,
            "tests/b_smoke_test.mw",
            "fn smoke()\n    var f: Fixture\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_lacks_code(&records, "check.unknown_type");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn keeps_configured_test_local_unknown_type_diagnostics_when_hidden_type_names_match() {
    let root = temp_project("proj-test-local-unknown-type-incomplete-source", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             \tconst BAD = 1\n",
        );
        write(
            root,
            "tests/smoke_test.mw",
            "fn smoke()\n    var n: Nope\n    var y: int = \"str\"\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::jsonl(output.stdout);
    assert_has_code(&records, "parse.syntax");
    assert_has_code(&records, "check.unknown_type");
    assert_has_code(&records, "check.assignment_type");
}

#[test]
fn reports_missing_marrow_json() {
    let root = temp_project("proj-noconfig", |root| {
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let output = run_check(&["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let record = support::json(output.stdout);
    assert_eq!(record["code"], "io.read");
}

#[test]
fn project_diagnostics_carry_the_documented_kind_envelope_field() {
    // The error envelope's `kind` is a common field, so the project diagnostic
    // path must emit it.
    let root = temp_project("proj-kind", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_check(&["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let report: Value = serde_json::from_str(&stdout).expect("json report");
    let diagnostic = &report["diagnostics"][0];
    assert_eq!(diagnostic["code"], "check.module_path");
    assert_eq!(diagnostic["kind"], "check", "{diagnostic}");
}

#[test]
fn a_simple_config_error_carries_the_documented_kind_envelope_field() {
    // `report_simple_error` (config/project/runtime/store failures) must also
    // emit `kind`; an invalid marrow.json yields a `config.*` code -> tooling.
    let root = temp_project("proj-badconfig", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": [] }"#);
    });
    let output = run_check(&["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let record: Value = serde_json::from_str(stdout.trim()).expect("json record");
    assert_eq!(record["code"], "config.invalid");
    assert_eq!(record["kind"], "tooling", "{record}");
}

#[test]
fn rejects_a_private_default_entry() {
    let root = temp_project("proj-default-entry-private", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(root, "src/app.mw", "module app\n\nfn main()\n    return\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::diagnostic_records(output.stdout);
    assert_has_code(&records, "check.default_entry");
    assert_has_file(&records, "marrow.json");
}

#[test]
fn rejects_a_nonexistent_default_entry() {
    let root = temp_project("proj-default-entry-missing", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::nope" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::diagnostic_records(output.stdout);
    assert_has_code(&records, "check.default_entry");
}

#[test]
fn rejects_an_empty_string_default_entry() {
    let root = temp_project("proj-default-entry-empty", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::diagnostic_records(output.stdout);
    assert_has_code(&records, "check.default_entry");
}

#[test]
fn rejects_a_default_entry_that_takes_arguments() {
    let root = temp_project("proj-default-entry-args", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(name: string)\n    print(name)\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::diagnostic_records(output.stdout);
    assert_has_code(&records, "check.default_entry");
}

#[test]
fn accepts_a_clean_zero_argument_default_entry() {
    let root = temp_project("proj-default-entry-ok", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    return\n",
        );
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = support::diagnostic_records(output.stdout);
    assert_lacks_code(&records, "check.default_entry");
}
