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

#[test]
fn check_rejects_a_bare_file_target_as_not_a_project() {
    let path = support::temp_source(
        "file-target",
        r#"module app
pub fn main()
    print("ok")
"#,
    );

    let output = support::marrow_sub("check", &[path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    // A bare file is the same not-a-project mistake as a directory missing marrow.json: the loader
    // owns it as `config.not_a_project` and exits 1, the same code every command uses, rather than
    // a command-local usage failure.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("config.not_a_project") && stderr.contains("bare file"),
        "{stderr}"
    );
    assert!(stderr.contains("marrow.json"), "{stderr}");
    assert!(!stderr.contains("os error"), "{stderr}");
}

#[test]
fn a_bare_file_projectdir_reads_as_not_a_project_with_one_message_across_commands() {
    // A regular file passed where a project directory is expected is one mistake with one owner:
    // the project loader classifies it as `config.not_a_project`, so every command emits the
    // identical not-a-project prose at the identical exit code, naming marrow.json with no raw OS
    // errno and no misleading `marrow init <file>` remedy (which would fail on a bare file). The
    // commands that take extra arguments still fault at the project path before reaching them.
    let path = support::temp_source(
        "bare-file-projectdir",
        r#"module app
pub fn main()
    print("ok")
"#,
    );
    let target = path.to_str().unwrap();
    let backup = support::unique_temp_path("bare-file-backup");
    let backup = backup.to_str().unwrap();

    let invocations: [(&str, Vec<&str>); 8] = [
        ("run", vec![target]),
        ("test", vec![target]),
        ("data", vec!["stats", target]),
        ("evolve", vec!["preview", target]),
        ("backup", vec![target, backup]),
        ("restore", vec![target, backup]),
        ("check", vec![target]),
        ("client", vec!["typescript", target]),
    ];

    let mut messages = Vec::new();
    let mut codes = Vec::new();
    for (command, args) in &invocations {
        let output = support::marrow_sub(command, args);
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            stderr.contains("marrow.json") && stderr.contains("bare file"),
            "{command}: a bare-file projectdir must read as not-a-project naming marrow.json: {stderr}"
        );
        assert!(
            !stderr.contains("os error"),
            "{command}: a bare-file projectdir must not leak a raw OS errno: {stderr}"
        );
        assert!(
            !stderr.contains("marrow init"),
            "{command}: a bare file cannot be initialized in place, so no init remedy: {stderr}"
        );
        messages.push(stderr);
        codes.push(output.status.code());
    }

    let first_message = messages[0].clone();
    let first_code = codes[0];
    for ((command, _), (message, code)) in invocations.iter().zip(messages.iter().zip(&codes)) {
        assert_eq!(
            *message, first_message,
            "{command}: every command must emit the identical not-a-project prose"
        );
        assert_eq!(
            *code, first_code,
            "{command}: every command must exit with the identical code"
        );
    }

    // `doctor` reports the same not-a-project condition as a structured finding rather than a bare
    // stderr line, so it carries the canonical message and code in the finding payload, never a raw
    // errno.
    let doctor = support::marrow_sub("doctor", &["--format", "json", target]);
    assert_eq!(doctor.status.code(), Some(1), "{doctor:?}");
    let report: Value = serde_json::from_slice(&doctor.stdout).expect("doctor json");
    let config_finding = report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .find(|finding| finding["code"] == serde_json::json!("doctor.config_invalid"))
        .expect("a config finding");
    assert_eq!(
        config_finding["data"]["underlying_code"],
        serde_json::json!("config.not_a_project"),
        "{report:#?}"
    );
    let doctor_stdout = String::from_utf8(doctor.stdout).expect("doctor stdout utf8");
    assert!(
        doctor_stdout.contains("bare file") && !doctor_stdout.contains("os error"),
        "doctor must carry the not-a-project prose with no raw errno: {doctor_stdout}"
    );

    fs::remove_file(&path).ok();
}

#[test]
fn check_rejects_a_bare_file_target_before_parsing_its_contents() {
    let path = support::temp_source("json-file-target", "\tbad\n");

    for format in ["json", "jsonl"] {
        let output = support::marrow_sub("check", &["--format", format, path.to_str().unwrap()]);

        // The bare file faults at the project loader, not the parser: a `config.not_a_project`
        // envelope at exit 1, never a `parse.syntax` diagnostic over the file's bad contents.
        assert_eq!(output.status.code(), Some(1), "{format}: {output:?}");
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let envelope: Value = serde_json::from_str(stdout.lines().next().unwrap_or(""))
            .unwrap_or_else(|_| panic!("{format}: json envelope: {stdout}"));
        assert_eq!(
            envelope["code"],
            serde_json::json!("config.not_a_project"),
            "{format}: {stdout}"
        );
        assert!(
            envelope["message"]
                .as_str()
                .is_some_and(|message| message.contains("marrow.json")),
            "{format}: {stdout}"
        );
        assert!(
            !stdout.contains("parse.syntax"),
            "{format} should fault at the loader before parsing: {stdout}"
        );
    }

    fs::remove_file(&path).ok();
}

#[test]
fn check_on_a_missing_directory_reports_a_missing_project() {
    let missing = support::unique_temp_path("check-missing-dir");

    let output = support::marrow_sub("check", &[missing.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    // A directory with no marrow.json is the everyday "wrong dir or not initialized" mistake, not
    // a low-level read fault: it names the missing project and points at marrow init, with no raw
    // OS error code.
    assert!(
        stderr.contains("config.missing") && stderr.contains("marrow.json"),
        "a missing project must read as config.missing, not a raw read fault: {stderr}"
    );
    assert!(
        stderr.contains("marrow init"),
        "the message must point at marrow init: {stderr}"
    );
    assert!(
        !stderr.contains("os error"),
        "the message must not leak a raw OS error code: {stderr}"
    );
    assert!(
        !stderr.contains("not a bare file"),
        "a missing directory is not a bare file: {stderr}"
    );
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
    let records = support::diagnostic_records(output.stdout);
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
    let records = support::diagnostic_records(output.stdout);
    records
        .iter()
        .find(|record| record["code"] == "parse.syntax" && record["source_span"]["line"] == 3)
        .expect("an obsolete-operator diagnostic on the body line");
}

#[test]
fn check_jsonl_reports_diagnostics_and_summary() {
    let dir = project_with_source(
        "jsonl-invalid",
        "src/app.mw",
        "module app\n\tpub fn main()\n",
    );

    let output = check_jsonl(dir.path());

    assert_eq!(output.status.code(), Some(1));
    let records = support::jsonl(output.stdout);
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
    let records = support::diagnostic_records(output.stdout);
    // `lock` and `merge` are parse-rejected, not checker-rejected: the reserved
    // surface never reaches `check.rejected_surface`. Each reserved statement is
    // rejected exactly at its own line (the `lock` on line 7 and the `merge` on line
    // 9), asserted by code and span rather than by the rendered reserved-word prose.
    assert!(
        !support::codes(&records).contains(&"check.rejected_surface"),
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

    let output = check_json(dir.path());

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
