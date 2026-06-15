use std::fs;

use crate::support;
use serde_json::Value;
fn check_json(path: impl AsRef<std::ffi::OsStr>) -> std::process::Output {
    let path = path.as_ref().to_str().expect("utf8 path");
    support::marrow_sub("check", &["--format", "json", path])
}

fn check_jsonl(path: impl AsRef<std::ffi::OsStr>) -> std::process::Output {
    let path = path.as_ref().to_str().expect("utf8 path");
    support::marrow_sub("check", &["--format", "jsonl", path])
}

fn diagnostic_codes(report: &Value) -> Vec<&str> {
    report["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect()
}

fn assert_has_code(report: &Value, code: &str) {
    assert!(diagnostic_codes(report).contains(&code), "{report:#?}");
}

/// The diagnostic records of a `--format jsonl` run: every record except the
/// trailing summary. Asserting against parsed records, not a stderr blob, keeps
/// the oracle on typed codes and payload fields rather than rendered prose.
fn diagnostic_records(output: std::process::Output) -> Vec<Value> {
    support::jsonl(output.stdout)
        .into_iter()
        .filter(|record| record["kind"] != "summary")
        .collect()
}

/// Whether any diagnostic record carries `code`.
fn has_code(records: &[Value], code: &str) -> bool {
    support::codes(records).contains(&code)
}

#[test]
fn check_rejects_file_targets_as_usage_failures() {
    let path = support::temp_source(
        "file-target",
        r#"module app
pub fn main()
    print("ok")
"#,
    );

    let output = support::marrow_sub("check", &[path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("project directory"), "{stderr}");
    assert!(stderr.contains("marrow.json"), "{stderr}");
}

#[test]
fn check_rejects_file_targets_before_json_diagnostics() {
    let path = support::temp_source("json-file-target", "\tbad\n");

    for format in ["json", "jsonl"] {
        let output = support::marrow_sub("check", &["--format", format, path.to_str().unwrap()]);

        assert_eq!(output.status.code(), Some(2), "{format}: {output:?}");
        assert!(
            output.stdout.is_empty(),
            "{format} unexpected stdout: {:?}",
            output.stdout
        );
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            stderr.contains("project directory") && stderr.contains("marrow.json"),
            "{format}: {stderr}"
        );
        assert!(
            !stderr.contains("parse.syntax"),
            "{format} should fail as usage before parsing: {stderr}"
        );
    }

    fs::remove_file(&path).ok();
}

#[test]
fn check_accepts_valid_project_source() {
    let dir = project_with_source(
        "valid",
        "src/app.mw",
        r#"module app
pub fn main()
    print("ok")
"#,
    );

    let output = support::marrow_sub("check", &[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("ok"), "{stdout}");
    assert!(stdout.contains("checked"), "{stdout}");
}

#[test]
fn check_text_renders_suggested_index_add_line() {
    let dir = project_with_source(
        "suggested-index-add-line",
        "src/app.mw",
        "module app\n\
         resource Book\n\
         \x20   shelf: string\n\
         store ^books(id: int): Book\n\
         pub fn countByShelf(shelf: string)\n\
         \x20   const n = count(^books.byShelf(shelf))\n",
    );

    let output = support::marrow_sub("check", &[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.collection_unsupported"), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line == "add: index byShelf(shelf, id)"),
        "{stderr}"
    );
}

#[test]
fn check_text_does_not_render_suggested_index_for_coalesce_value_context() {
    let dir = project_with_source(
        "suggested-index-deferred-coalesce",
        "src/app.mw",
        "module app\n\
         resource Book\n\
         \x20   required isbn: string\n\
         store ^books(id: int): Book\n\
         pub fn lookup(isbn: string, fallback: Id(^books)): Id(^books)\n\
         \x20   return ^books.byIsbn(isbn) ?? fallback\n",
    );

    let output = support::marrow_sub("check", &[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        !stderr.lines().any(|line| line.starts_with("add: ")),
        "{stderr}"
    );
}

#[test]
fn check_reports_parse_diagnostics() {
    let dir = project_with_source("invalid", "src/app.mw", "module app\n\tpub fn main()\n");

    let output = check_jsonl(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let records = diagnostic_records(output);
    // The tab is rejected as a parse error at its own position (the leading tab on
    // line 2, column 1), asserted by code and span rather than by its rendered prose.
    let tab = records
        .iter()
        .find(|record| record["source_span"]["line"] == 2 && record["source_span"]["column"] == 1)
        .expect("a diagnostic at the tab position");
    assert_eq!(tab["code"], "parse.syntax", "{tab}");
}

#[test]
fn check_allows_out_as_an_ordinary_binding_name() {
    let dir = temp_project_dir("out-binding");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    fs::write(
        dir.join("src/m.mw"),
        "module m\n\npub fn f(): int\n    var out: int = 0\n    return out\n",
    )
    .expect("write source");

    let output = check_jsonl(dir.path());

    assert_eq!(output.status.code(), Some(0));
    let records = diagnostic_records(output);
    assert!(records.is_empty(), "{records:#?}");
}

#[test]
fn check_reports_obsolete_operators_in_function_bodies() {
    let dir = project_with_source(
        "obsolete-op-body",
        "src/app.mw",
        "module app\nfn main()\n    return a && b\n",
    );

    let output = check_jsonl(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let records = diagnostic_records(output);
    let obsolete = records
        .iter()
        .find(|record| record["code"] == "parse.syntax" && record["source_span"]["line"] == 3)
        .expect("an obsolete-operator diagnostic on the body line");
    assert_eq!(obsolete["code"], "parse.syntax", "{obsolete}");
}

#[test]
fn check_jsonl_reports_diagnostics_and_summary() {
    let dir = project_with_source(
        "jsonl-invalid",
        "src/app.mw",
        "module app\n\tpub fn main()\n",
    );

    let output = support::marrow_sub("check", &["--format", "jsonl", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let records = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("jsonl record"))
        .collect::<Vec<_>>();
    // The tab-indented declaration parses to two diagnostics: the tab is rejected,
    // and the function is then left without a parseable indented body. Each parse
    // record precedes the trailing summary.
    assert_eq!(records.len(), 3, "{records:#?}");
    assert_eq!(records[0]["code"], "parse.syntax");
    assert_eq!(records[0]["kind"], "parse");
    assert_eq!(records[0]["source_span"]["line"], 2);
    assert_eq!(records[0]["source_span"]["column"], 1);
    assert_eq!(records[1]["code"], "parse.syntax");
    assert_eq!(records[1]["kind"], "parse");
    assert_eq!(records[2]["kind"], "summary");
    assert_eq!(records[2]["status"], "failed");
    assert_eq!(records[2]["diagnostics"], 2);
}

/// A temporary project directory with an empty `src` (caller fills it), left
/// uncommitted so the checker runs against pending durable identity.
fn temp_project_dir(name: &str) -> support::TempProject {
    support::temp_project_uncommitted(name, |root| {
        fs::create_dir_all(root.join("src")).expect("create project src dir");
    })
}

fn project_with_source(name: &str, relative: &str, source: &str) -> support::TempProject {
    let dir = temp_project_dir(name);
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    support::write(dir.path(), relative, source);
    dir
}

#[test]
fn check_reports_schema_diagnostics_for_a_project_directory() {
    // Checking a project directory (one with marrow.json) runs the whole-project
    // checker, which surfaces schema diagnostics.
    let dir = temp_project_dir("schema-project");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    fs::write(
        dir.join("src/shelf.mw"),
        "module shelf\nresource Book\n    note: unknown\nstore ^books(id: int): Book\n",
    )
    .expect("write source");

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "schema.unknown_in_saved");
}

#[test]
fn check_reports_reserved_merge_and_lock_as_parse_errors() {
    let dir = temp_project_dir("reserved-merge-lock");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    fs::write(
        dir.join("src/shelf.mw"),
         "module shelf\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: int)\n    lock ^books(id)\n        print(\"locked\")\n    merge ^books(id) = ^books(id)\n",
    )
    .expect("write source");

    let output = check_jsonl(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let records = diagnostic_records(output);
    // `lock` and `merge` are parse-rejected, not checker-rejected: the reserved
    // surface never reaches `check.rejected_surface`. Each reserved statement is
    // rejected exactly at its own line (the `lock` on line 7 and the `merge` on line
    // 9), asserted by code and span rather than by the rendered reserved-word prose.
    assert!(
        !has_code(&records, "check.rejected_surface"),
        "{records:#?}"
    );
    let reserved_lines: Vec<i64> = records
        .iter()
        .filter(|record| record["code"] == "parse.syntax")
        .filter_map(|record| record["source_span"]["line"].as_i64())
        .collect();
    assert!(reserved_lines.contains(&7), "{records:#?}");
    assert!(reserved_lines.contains(&9), "{records:#?}");
}

#[test]
fn check_rejects_removed_inout_syntax_for_a_project_directory() {
    let dir = temp_project_dir("removed-argument-mode");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    fs::write(
        dir.join("src/shelf.mw"),
        "module shelf\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn normalize(book: Book)\n    return\n\n\
         fn f(id: int)\n    normalize(inout ^books(id))\n",
    )
    .expect("write source");

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "parse.syntax");
    let expected_file = dir.join("src/shelf.mw").display().to_string();
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "parse.syntax"
                && diagnostic["kind"] == "parse"
                && diagnostic["source_span"]["file"]
                    .as_str()
                    .is_some_and(|file| file == expected_file)
                && diagnostic["source_span"]["line"] == 10),
        "{report:#?}"
    );
}

#[test]
fn check_reports_return_type_errors_for_a_project() {
    let dir = project_with_source(
        "return-type",
        "src/m.mw",
        "module m\nfn f(): int\n    return \"nope\"\n",
    );

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.return_type");
    assert!(
        report["diagnostics"][0]["source_span"]["file"]
            .as_str()
            .is_some_and(|file| file == dir.join("src/m.mw").display().to_string()),
        "diagnostic should point at the project source file: {report:#?}"
    );
}

#[test]
fn check_reports_assignment_type_errors_for_a_project() {
    let dir = project_with_source(
        "assignment-type",
        "src/m.mw",
        "module m\nfn f()\n    var x: int = \"str\"\n",
    );

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.assignment_type");
}

#[test]
fn check_reports_operator_type_errors_for_a_project() {
    let dir = project_with_source(
        "operator-type",
        "src/m.mw",
        "module m\nfn f()\n    var x = 1 + true\n",
    );

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.operator_type");
}

#[test]
fn check_reports_type_errors_in_a_module_less_script() {
    let dir = project_with_source(
        "script-type",
        "src/main.mw",
        "fn f(): int\n    return \"nope\"\n",
    );

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.return_type");
}

#[test]
fn check_accepts_a_type_correct_project() {
    let dir = project_with_source(
        "type-correct",
        "src/m.mw",
        "module m\nfn f(): int\n    return 1\n\nfn g()\n    var x: int = f()\n",
    );

    let output = support::marrow_sub("check", &[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn check_json_reports_type_errors_for_a_project() {
    let dir = project_with_source(
        "json-return-type",
        "src/m.mw",
        "module m\nfn f(): int\n    return \"nope\"\n",
    );

    let output = support::marrow_sub("check", &["--format", "json", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let record: Value = serde_json::from_str(stdout.trim()).expect("json object");
    assert_eq!(record["status"], "failed", "{stdout}");
    assert_eq!(
        record["diagnostics"][0]["code"], "check.return_type",
        "{stdout}"
    );
}

#[test]
fn check_module_less_project_script_string_into_an_int_field_errors() {
    let dir = project_with_source(
        "script-string-into-int",
        "src/main.mw",
        "resource Order\n    required count: int\nstore ^orders(id: int): Order\n\npub fn main()\n    var o: Order\n    o.count = \"alsobad\"\n    ^orders(1) = o\n",
    );

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.assignment_type");
}

#[test]
fn check_rejects_a_project_with_two_module_less_scripts() {
    // A project may hold at most one module-less file; library files declare a
    // `module`. Two scripts share the empty module name, so the checker rejects
    // the project rather than alias one script's names against the other's.
    let dir = temp_project_dir("two-scripts");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    fs::write(
        dir.join("src/one.mw"),
        "pub fn main()\n    print(\"one\")\n",
    )
    .expect("write one");
    fs::write(
        dir.join("src/two.mw"),
        "pub fn other()\n    print(\"two\")\n",
    )
    .expect("write two");

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.multiple_scripts");
}

#[test]
fn check_rejects_module_declarations_named_like_builtins() {
    let dir = temp_project_dir("builtin-shadow");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
    )
    .expect("write config");
    fs::write(
        dir.join("src/app.mw"),
        "module app\n\nfn exists(x: int): int\n    return x\n\nconst keys = 1\n",
    )
    .expect("write source");

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.duplicate_declaration");
}
