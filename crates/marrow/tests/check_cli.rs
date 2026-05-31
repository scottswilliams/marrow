use std::fs;
use std::process::Command;

use serde_json::Value;

fn temp_source(name: &str, source: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    path.push(format!("marrow-{name}-{}-{nanos}.mw", std::process::id(),));
    fs::write(&path, source).expect("write source");
    path
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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn check_reports_parse_diagnostics() {
    let path = temp_source("invalid", "module app\n\tpub fn main()\n");

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&dir)
        .output()
        .expect("run marrow check");

    fs::remove_dir_all(&dir).ok();
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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg("--format")
        .arg("jsonl")
        .arg(&path)
        .output()
        .expect("run marrow check");

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

/// Create an empty temporary project directory (caller fills it).
fn temp_project_dir(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(dir.join("src")).expect("create project src dir");
    dir
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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&dir)
        .output()
        .expect("run marrow check");

    fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("schema.unknown_in_saved"), "{stderr}");
}

#[test]
fn check_reports_return_type_errors_for_a_single_file() {
    // A single file that parses but returns the wrong type used to print "ok" and
    // exit 0 because the single-file path only parsed. It now runs the full type
    // checker, so the `check.return_type` diagnostic surfaces and the exit is
    // non-zero — located at the operator's real file path, not a scratch path.
    let path = temp_source(
        "return-type",
        "module m\nfn f(): int\n    return \"nope\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.return_type"), "{stderr}");
    assert!(
        stderr.contains(&path.display().to_string()),
        "diagnostic should point at the named file: {stderr}"
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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.assignment_type"), "{stderr}");
}

#[test]
fn check_reports_operator_type_errors_for_a_single_file() {
    // `1 + true` is an int-plus-bool operator error; the single-file path now
    // reaches the operator type rule.
    let path = temp_source("operator-type", "module m\nfn f()\n    var x = 1 + true\n");

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.operator_type"), "{stderr}");
}

#[test]
fn check_reports_type_errors_in_a_module_less_script() {
    // A module-less script (no `module` line) is still type-checked: it is placed
    // path-free in the synthesized project, so no spurious module-path error masks
    // the real `check.return_type` diagnostic.
    let path = temp_source("script-type", "fn f(): int\n    return \"nope\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.return_type"), "{stderr}");
}

#[test]
fn check_accepts_a_type_correct_single_file() {
    // A file with a body that type-checks cleanly still exits 0 with the friendly
    // `ok` summary — the type checker adds no false positives on correct source.
    let path = temp_source(
        "type-correct",
        "module m\nfn f(): int\n    return 1\n\nfn g()\n    var x: int = f()\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg(&path)
        .output()
        .expect("run marrow check");

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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run marrow check");

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.assignment_type"), "{stderr}");
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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&dir)
        .output()
        .expect("run marrow check");

    fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.multiple_scripts"), "{stderr}");
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

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .arg(&dir)
        .output()
        .expect("run marrow check");

    fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.duplicate_declaration"), "{stderr}");
    assert!(stderr.contains("builtin"), "{stderr}");
    assert!(stderr.contains("exists"), "{stderr}");
    assert!(stderr.contains("keys"), "{stderr}");
}
