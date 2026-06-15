use crate::support;
use support::{marrow_sub, parse_result_line, temp_project, write};

/// The diagnostic code of a run fault, read from the structured position of the
/// rendered fault line rather than matched anywhere in the stderr blob. A run
/// reports an entry-resolution fault as `code: message` on its last stderr line; the
/// code is the dotted token before the first `: ` separator (codes carry no spaces,
/// so the split is unambiguous). Asserting the code in this position keeps the oracle
/// reword-proof against changes to the human message that follows it.
fn fault_code(stderr: &[u8]) -> String {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a fault line");
    parse_result_line(line).code
}

#[test]
fn runs_the_default_entry_and_prints_its_output() {
    let root = temp_project("run-default", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hello from marrow\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "hello from marrow\n");
}

#[test]
fn failing_run_keeps_program_output_written_before_the_fault() {
    let root = temp_project("run-fault-output", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"before fault\")\n    const boom = 1 / 0\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "before fault\n");
    assert_eq!(fault_code(&output.stderr), "run.divide_by_zero");
}

#[test]
fn entry_flag_overrides_the_default_entry() {
    let root = temp_project("run-entry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"main\")\n\npub fn greet()\n    print(\"greet\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::greet", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "greet\n");
}

#[test]
fn bare_entry_flag_resolves_a_unique_public_function() {
    let root = temp_project("run-bare-entry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/util.mw",
            "module util\n\npub fn helper(): int\n    print(\"helper ran\")\n    return 1\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "helper", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "helper ran\n");
}

#[test]
fn bare_entry_flag_rejects_ambiguous_public_functions() {
    let root = temp_project("run-ambiguous-entry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"app\")\n",
        );
        write(
            root,
            "src/admin.mw",
            "module admin\n\npub fn main()\n    print(\"admin\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "main", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.ambiguous_function");
}

#[test]
fn entry_flag_rejects_private_functions() {
    let root = temp_project("run-private-entry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\nfn main()\n    print(\"private\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::main", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.private_function");
}

#[test]
fn run_rejects_duplicate_format_flag() {
    let output = marrow_sub(
        "run",
        &["--format", "json", "--format", "text", "missing-project"],
    );

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}

#[test]
fn reports_a_missing_entry() {
    let root = temp_project("run-noentry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.no_entry");
}

#[test]
fn maps_an_unknown_entry_to_a_runtime_code() {
    let root = temp_project("run-unknown", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::nope", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.unknown_function");
}

#[test]
fn runs_a_module_less_script_bare_entry() {
    // A module-less script is self-resolvable: its `pub fn main` lives in the
    // empty module, so the bare entry `main` resolves to it and runs. This is the
    // legitimate path for this construction: no `run.no_entry`.
    let root = temp_project("run-module-less-script", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "pub fn main()\n    print(\"from a script\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "from a script\n");
}

#[test]
fn entry_args_decode_scalars_sequences_enums_and_identities() {
    let root = temp_project("run-entry-args", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\
             resource Author\n\
             \x20\x20\x20\x20name: string\n\
             store ^authors(id: int): Author\n\n\
             pub fn show(n: int, label: string, status: Status, author: Id(^authors), xs: sequence[int])\n\
             \x20\x20\x20\x20var total = n\n\
             \x20\x20\x20\x20for x in xs\n\
             \x20\x20\x20\x20\x20\x20\x20\x20total = total + x\n\
             \x20\x20\x20\x20if status == Status::archived\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print($\"{label}:{total}:archived\")\n\
             \x20\x20\x20\x20else\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print($\"{label}:{total}:active\")\n",
        );
    });

    let output = marrow_sub(
        "run",
        &[
            "--entry",
            "app::show",
            "--arg",
            "n=3",
            "--arg",
            "label=a=b",
            "--arg",
            "status=archived",
            "--arg",
            "author=7",
            "--arg",
            "xs=4",
            "--arg",
            "xs=5",
            root.to_str().unwrap(),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout utf8"),
        "a=b:12:archived\n"
    );
}

#[test]
fn entry_args_collect_repeated_enum_sequence_values_in_argv_order() {
    let root = temp_project("run-entry-enum-sequence", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\n\
             pub fn countArchived(statuses: sequence[Status])\n\
             \x20\x20\x20\x20var total = 0\n\
             \x20\x20\x20\x20for status in statuses\n\
             \x20\x20\x20\x20\x20\x20\x20\x20if status == Status::archived\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20total = total + 1\n\
             \x20\x20\x20\x20print($\"{total}\")\n",
        );
    });

    let output = marrow_sub(
        "run",
        &[
            "--entry",
            "app::countArchived",
            "--arg",
            "statuses=archived",
            "--arg",
            "statuses=active",
            "--arg",
            "statuses=archived",
            root.to_str().unwrap(),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout utf8"),
        "2\n"
    );
}

#[test]
fn entry_args_accept_empty_sequence_and_reject_args_json() {
    let root = temp_project("run-entry-empty-sequence", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             pub fn countArgs(xs: sequence[int])\n\
             \x20\x20\x20\x20var total = 0\n\
             \x20\x20\x20\x20for x in xs\n\
             \x20\x20\x20\x20\x20\x20\x20\x20total = total + 1\n\
             \x20\x20\x20\x20print($\"{total}\")\n",
        );
    });

    let empty = marrow_sub(
        "run",
        &[
            "--entry",
            "app::countArgs",
            "--arg",
            "xs=[]",
            root.to_str().unwrap(),
        ],
    );
    assert_eq!(empty.status.code(), Some(0), "{empty:?}");
    assert_eq!(String::from_utf8(empty.stdout).expect("stdout utf8"), "0\n");

    let json = marrow_sub(
        "run",
        &[
            "--entry",
            "app::countArgs",
            "--args-json",
            r#"{"xs":[]}"#,
            root.to_str().unwrap(),
        ],
    );
    assert_eq!(json.status.code(), Some(2), "{json:?}");
    let stderr = String::from_utf8(json.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("unknown run option: --args-json"),
        "{stderr}"
    );
}

#[test]
fn entry_args_reject_composite_identity_params() {
    let root = temp_project("run-entry-composite-id", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Enrollment\n\
             \x20\x20\x20\x20status: string\n\
             store ^enrollments(student: string, course: string): Enrollment\n\n\
             pub fn mark(id: Id(^enrollments))\n\
             \x20\x20\x20\x20print(\"unused\")\n",
        );
    });

    let output = marrow_sub(
        "run",
        &[
            "--entry",
            "app::mark",
            "--arg",
            "id=student-1",
            root.to_str().unwrap(),
        ],
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.entry_argument");
}

#[test]
fn json_run_envelope_captures_output_return_and_read_only_stamp() {
    let root = temp_project("run-json-envelope", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main(): int\n\
             \x20\x20\x20\x20print(\"captured\")\n\
             \x20\x20\x20\x20return 7\n",
        );
    });

    let output = marrow_sub("run", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "read-only JSON run should not render tooling stderr: {output:?}"
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("run JSON envelope");
    assert_eq!(envelope["output"], "captured\n", "{envelope}");
    assert_eq!(
        envelope["return"],
        serde_json::json!({ "kind": "int", "value": 7 })
    );
    assert_eq!(envelope["signature_digest"], serde_json::Value::Null);
    assert_eq!(envelope["raises"], serde_json::Value::Null);
    assert!(envelope.get("committed").is_none(), "{envelope}");
    assert!(
        envelope["store_stamp"]["store_uid"].is_string(),
        "{envelope}"
    );
    assert!(
        envelope["store_stamp"]["catalog_epoch"].is_number(),
        "{envelope}"
    );
    assert!(
        envelope["store_stamp"]["commit_id"].is_number(),
        "{envelope}"
    );
}

#[test]
fn json_run_envelope_marks_committed_write_invocations() {
    let root = temp_project("run-json-committed", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main(): string\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20return \"ok\"\n",
        );
    });

    let output = marrow_sub("run", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("run JSON envelope");
    assert_eq!(envelope["committed"], true, "{envelope}");
    assert_eq!(
        envelope["return"],
        serde_json::json!({ "kind": "string", "value": "ok" })
    );
}

#[test]
fn json_run_surface_errors_after_commit_report_mutation_truth() {
    let root = temp_project("run-json-committed-entry-surface", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn writeThenResource(): Book\n\
             \x20\x20\x20\x20^books(1).title = \"committed before render\"\n\
             \x20\x20\x20\x20var book: Book\n\
             \x20\x20\x20\x20book.title = \"not json\"\n\
             \x20\x20\x20\x20return book\n\n\
             pub fn writeThenSequence(): sequence[Book]\n\
             \x20\x20\x20\x20^books(2).title = \"nested committed before render\"\n\
             \x20\x20\x20\x20var book: Book\n\
             \x20\x20\x20\x20book.title = \"nested not json\"\n\
             \x20\x20\x20\x20var books: sequence[Book]\n\
             \x20\x20\x20\x20books(1) = book\n\
             \x20\x20\x20\x20return books\n",
        );
    });

    for (entry, expected_path) in [
        ("app::writeThenResource", "^books(1).title"),
        ("app::writeThenSequence", "^books(2).title"),
    ] {
        let output = marrow_sub(
            "run",
            &["--format", "json", "--entry", entry, root.to_str().unwrap()],
        );

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            output.stdout.is_empty(),
            "post-commit return-surface failure must not look like a successful JSON run: {output:?}"
        );
        let faults = support::json_records_in_stderr(output.stderr);
        let fault = faults.last().expect("json fault");
        assert_eq!(fault["code"], "run.entry_surface", "{fault}");
        assert_eq!(fault["committed"], true, "{fault}");
        assert!(fault["store_stamp"]["store_uid"].is_string(), "{fault}");
        assert!(fault["store_stamp"]["catalog_epoch"].is_number(), "{fault}");
        assert!(fault["store_stamp"]["commit_id"].is_number(), "{fault}");

        let dump = marrow_sub(
            "data",
            &["dump", "--format", "json", root.to_str().unwrap()],
        );
        assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
        let dump_json: serde_json::Value =
            serde_json::from_slice(&dump.stdout).expect("dump JSON envelope");
        assert!(
            dump_json["records"]
                .as_array()
                .expect("records array")
                .iter()
                .any(|record| record["path"] == expected_path),
            "the durable write must have committed before return rendering failed: {dump_json}"
        );
    }
}

#[test]
fn json_run_errors_carry_uncaught_error_data_code() {
    let root = temp_project("run-json-uncaught-error-code", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20throw Error(code: \"app.boom\", message: \"boom\")\n",
        );
    });

    let output = marrow_sub("run", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::json_records_in_stderr(output.stderr);
    let fault = records.last().expect("json fault");
    assert_eq!(fault["code"], "run.uncaught_error", "{fault}");
    assert_eq!(fault["data"]["code"], "app.boom", "{fault}");
}

#[test]
fn json_runtime_fault_after_commit_reports_mutation_truth() {
    let root = temp_project("run-json-committed-runtime-fault", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"committed\"\n\
             \x20\x20\x20\x20const boom = 1 / 0\n",
        );
    });

    let output = marrow_sub("run", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "runtime fault must not look like a successful JSON run: {output:?}"
    );
    let faults = support::json_records_in_stderr(output.stderr);
    let fault = faults.last().expect("json fault");
    assert_eq!(fault["code"], "run.divide_by_zero", "{fault}");
    assert_eq!(fault["committed"], true, "{fault}");
    assert!(fault["store_stamp"]["store_uid"].is_string(), "{fault}");
    assert!(fault["store_stamp"]["catalog_epoch"].is_number(), "{fault}");
    assert!(fault["store_stamp"]["commit_id"].is_number(), "{fault}");

    let dump = marrow_sub(
        "data",
        &["dump", "--format", "json", root.to_str().unwrap()],
    );
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_json: serde_json::Value =
        serde_json::from_slice(&dump.stdout).expect("dump JSON envelope");
    assert!(
        dump_json["records"]
            .as_array()
            .expect("records array")
            .iter()
            .any(|record| record["path"] == "^books(1).title"),
        "the durable write must have committed before the runtime fault: {dump_json}"
    );
}

#[test]
fn json_run_envelope_renders_identity_return_and_rejects_resource_return() {
    let root = temp_project("run-json-identity-return", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Author\n\
             \x20\x20\x20\x20name: string\n\
             store ^authors(slug: string): Author\n\n\
             pub fn identity(): Id(^authors)\n\
             \x20\x20\x20\x20return Id(^authors, \"ada\")\n\n\
             pub fn resourceReturn(): Author\n\
             \x20\x20\x20\x20var author: Author\n\
             \x20\x20\x20\x20author.name = \"Ada\"\n\
             \x20\x20\x20\x20return author\n",
        );
    });

    let identity = marrow_sub(
        "run",
        &[
            "--format",
            "json",
            "--entry",
            "app::identity",
            root.to_str().unwrap(),
        ],
    );
    assert_eq!(identity.status.code(), Some(0), "{identity:?}");
    let envelope: serde_json::Value =
        serde_json::from_slice(&identity.stdout).expect("identity envelope");
    assert_eq!(
        envelope["return"],
        serde_json::json!({
            "kind": "identity",
            "root": "authors",
            "keys": [{ "type": "string", "value": "ada" }]
        }),
        "{envelope}"
    );

    let resource = marrow_sub(
        "run",
        &[
            "--format",
            "json",
            "--entry",
            "app::resourceReturn",
            root.to_str().unwrap(),
        ],
    );
    assert_eq!(resource.status.code(), Some(1), "{resource:?}");
    let faults = support::json_records_in_stderr(resource.stderr);
    assert_eq!(faults.last().unwrap()["code"], "run.entry_surface");
}
