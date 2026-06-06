use std::fs;

use serde_json::Value;

mod support;

use support::temp_source;

fn check_json(path: impl AsRef<std::ffi::OsStr>) -> std::process::Output {
    let path = path.as_ref().to_str().expect("utf8 path");
    support::marrow_sub("check", &["--format", "json", path])
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

#[test]
fn check_accepts_valid_mw_source() {
    let path = temp_source(
        "valid",
        r#"module app
pub fn main()
    print("ok")
"#,
    );

    let output = support::marrow_sub("check", &[path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn check_reports_parse_diagnostics() {
    let path = temp_source("invalid", "module app\n\tpub fn main()\n");

    let output = support::marrow_sub("check", &[path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    assert!(stderr.contains("tabs"), "{stderr}");
}

#[test]
fn check_reserved_word_binding_reports_parse_errors_without_control_flow_cascade() {
    let dir = temp_project_dir("reserved-binding");
    fs::write(dir.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
    fs::write(
        dir.join("src/m.mw"),
        "module m\n\npub fn f(): int\n    var out: int = 0\n    return out\n",
    )
    .expect("write source");

    let output = support::marrow_sub("check", &[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("expected variable name"), "{stderr}");
    assert!(
        stderr.contains("cannot be used as an expression"),
        "{stderr}"
    );
    assert!(!stderr.contains("expected a statement"), "{stderr}");
    assert!(!stderr.contains("check.missing_return"), "{stderr}");
}

#[test]
fn check_reports_obsolete_operators_in_function_bodies() {
    let path = temp_source(
        "obsolete-op-body",
        "module app\nfn main()\n    return a && b\n",
    );

    let output = support::marrow_sub("check", &[path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    assert!(stderr.contains("`&&`"), "{stderr}");
    assert!(stderr.contains("Use `and` for boolean and"), "{stderr}");
}

#[test]
fn check_jsonl_reports_diagnostics_and_summary() {
    let path = temp_source("jsonl-invalid", "module app\n\tpub fn main()\n");

    let output = support::marrow_sub("check", &["--format", "jsonl", path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
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

#[test]
fn check_reports_schema_diagnostics_for_a_project_directory() {
    // Checking a project directory (one with marrow.json) runs the whole-project
    // checker, which surfaces schema diagnostics that single-file parsing cannot.
    let dir = temp_project_dir("schema-project");
    fs::write(dir.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
    fs::write(
        dir.join("src/shelf.mw"),
        "module shelf\nresource Book at ^books(id: int)\n    note: unknown\n",
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
    fs::write(dir.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
    fs::write(
        dir.join("src/shelf.mw"),
         "module shelf\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f(id: int)\n    lock ^books(id)\n        print(\"locked\")\n    merge ^books(id) = ^books(id)\n",
    )
    .expect("write source");

    let output = support::marrow_sub("check", &[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    assert!(stderr.contains("`lock` is reserved"), "{stderr}");
    assert!(stderr.contains("`merge` is reserved"), "{stderr}");
    assert!(!stderr.contains("check.rejected_surface"), "{stderr}");
}

#[test]
fn check_rejects_saved_inout_for_a_project_directory() {
    let dir = temp_project_dir("saved-inout");
    fs::write(dir.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
    fs::write(
        dir.join("src/shelf.mw"),
        "module shelf\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn normalize(inout book: Book)\n    return\n\n\
         fn f(id: int)\n    normalize(inout ^books(id))\n",
    )
    .expect("write source");

    let output = check_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.rejected_surface");
}

#[test]
fn check_reports_return_type_errors_for_a_single_file() {
    // A single-file check runs the same type checker as a project check, with
    // diagnostics located at the source path passed to the CLI.
    let path = temp_source(
        "return-type",
        "module m\nfn f(): int\n    return \"nope\"\n",
    );

    let output = check_json(&path);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.return_type");
    assert!(
        report["diagnostics"][0]["source_span"]["file"]
            .as_str()
            .is_some_and(|file| file == path.display().to_string()),
        "diagnostic should point at the named file: {report:#?}"
    );
}

#[test]
fn check_reports_assignment_type_errors_for_a_single_file() {
    // Assigning a `string` to an `int` local is a `check.assignment_type` error
    // that single-file parsing alone never caught.
    let path = temp_source(
        "assignment-type",
        "module m\nfn f()\n    var x: int = \"str\"\n",
    );

    let output = check_json(&path);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.assignment_type");
}

#[test]
fn check_reports_operator_type_errors_for_a_single_file() {
    // `1 + true` is an int-plus-bool operator error in single-file and project
    // checks alike.
    let path = temp_source("operator-type", "module m\nfn f()\n    var x = 1 + true\n");

    let output = check_json(&path);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.operator_type");
}

#[test]
fn check_reports_type_errors_in_a_module_less_script() {
    // A module-less script (no `module` line) is still type-checked: it is placed
    // path-free in the synthesized project, so no spurious module-path error masks
    // the real `check.return_type` diagnostic.
    let path = temp_source("script-type", "fn f(): int\n    return \"nope\"\n");

    let output = check_json(&path);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let report = support::json(output.stdout);
    assert_has_code(&report, "check.return_type");
}

#[test]
fn check_accepts_a_type_correct_single_file() {
    // A file with a body that type-checks cleanly still exits 0 with the friendly
    // `ok` summary — the type checker adds no false positives on correct source.
    let path = temp_source(
        "type-correct",
        "module m\nfn f(): int\n    return 1\n\nfn g()\n    var x: int = f()\n",
    );

    let output = support::marrow_sub("check", &[path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn check_json_reports_type_errors_for_a_single_file() {
    // The `--format json` single-file path also surfaces type errors: a failed
    // status and a `check.return_type` diagnostic record.
    let path = temp_source(
        "json-return-type",
        "module m\nfn f(): int\n    return \"nope\"\n",
    );

    let output = support::marrow_sub("check", &["--format", "json", path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
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
fn check_single_module_less_script_string_into_an_int_field_errors() {
    // A module-less single file is a script. Its own `^orders` resource must be
    // nominally checked through the single-file `check` path: a `string` written
    // into the `int` field `count` is a type mismatch, exit 1.
    let path = temp_source(
        "single-script-string-into-int",
        "resource Order at ^orders(id: int)\n    required count: int\n\npub fn main()\n    var o: Order\n    o.count = \"alsobad\"\n    ^orders(1) = o\n",
    );

    let output = check_json(&path);

    fs::remove_file(&path).ok();
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
    fs::write(dir.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
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
    fs::write(dir.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
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
