mod support;

use serde_json::{Value, json};

use support::{json_records_in_stderr, jsonl, marrow, temp_project, write};

/// Parse the trace record stream a `--format jsonl` run emits on stderr. Every
/// line is one JSON record; the pass/fail report stays on stdout, so the trace
/// stream needs no filtering.
fn jsonl_trace_records(stderr: Vec<u8>) -> Vec<Value> {
    jsonl(stderr)
}

fn faulting_print_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"before fault\")\n    const boom = 1 / 0\n",
        );
    })
}

#[test]
fn run_trace_interleaves_steps_and_writes() {
    // An entry that writes a field then prints. With `--trace`, the trace stream
    // reports the writing statement, the node write and field write it produced, and
    // the print — a step, then two writes, then a step — in that execution order, and
    // the program's own output still lands on stdout. The JSONL records preserve
    // emission order, so the interleaving is asserted as the typed record sequence
    // rather than by string offsets into the rendered text.
    let project = temp_project("trace-run", |root| {
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
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"done\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // The program's own output is unaffected and stays off the record stream.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "done\n", "stdout: {stdout}");
    let records = jsonl_trace_records(output.stderr);
    let [writing_step, node_write, write, print_step, summary] = records.as_slice() else {
        panic!("expected writing-step, node-write, write, print-step, summary: {records:?}");
    };
    // The writing statement (the `^books(1).title = ...` line, line 8) is reported,
    // then the node and field writes it produced, then the `print("done")` step (line 9).
    assert_eq!(writing_step["kind"], json!("step"));
    assert_eq!(writing_step["line"], json!(8));
    assert!(
        writing_step["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("app.mw")),
        "{writing_step}"
    );
    assert_eq!(node_write["kind"], json!("write"));
    assert_eq!(node_write["op"], json!("write"));
    assert_eq!(node_write["path"], json!("^books(1)"));
    assert_eq!(node_write["value_b64"], Value::Null);
    assert_eq!(write["kind"], json!("write"));
    assert_eq!(write["op"], json!("write"));
    assert_eq!(write["path"], json!("^books(1).title"));
    assert_eq!(write["target"]["store"], json!("books"));
    assert_eq!(print_step["kind"], json!("step"));
    assert_eq!(print_step["line"], json!(9));
    assert_eq!(summary["kind"], json!("summary"));
}

#[test]
fn run_trace_renders_a_bool_write_as_its_typed_value() {
    // A managed write of a `bool` field traces as `true`, not the codec byte `1`: the
    // text trace renders the leaf value through its declared scalar type. The JSON
    // record deliberately carries the raw codec bytes, so the typed oracle here is the
    // write target (the `on` field of `^flags(1)`) plus the stored bool codec byte
    // `value_b64 == "MQ=="`; the `= true` rendering is the human text render contract.
    let project = temp_project("trace-bool", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Flag\n\
             \x20\x20\x20\x20on: bool\n\
             store ^flags(id: int): Flag\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^flags(1).on = true\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    // Typed oracle: the bool write targets the `on` member of `^flags(1)` and stores
    // the codec byte for `true`.
    let json_run = marrow(&["run", "--trace", "--format", "jsonl", &dir]);
    assert_eq!(json_run.status.code(), Some(0), "{json_run:?}");
    let records = jsonl_trace_records(json_run.stderr);
    let write = records
        .iter()
        .find(|record| record["kind"] == "write" && record["path"] == "^flags(1).on")
        .expect("a field write record");
    assert_eq!(write["op"], json!("write"));
    assert_eq!(write["target"]["store"], json!("flags"));
    assert_eq!(write["target"]["identity"], json!(["1"]));
    assert_eq!(write["target"]["path"], json!([{ "member": "on" }]));
    assert_eq!(write["value_b64"], json!("MQ=="), "stored bool codec byte");

    // Render contract: the human text trace renders that codec byte as the typed
    // scalar, never leaking the byte. The line the render must produce and the byte
    // leak it must never produce are pinned as golden fragments of the debug render;
    // the typed value above is the oracle for the stored bytes.
    const BOOL_WRITE_RENDER: &str = "write ^flags(1).on = true";
    const BOOL_CODEC_BYTE_LEAK: &str = "^flags(1).on = 1";
    let text_run = marrow(&["run", "--trace", &dir]);
    assert_eq!(text_run.status.code(), Some(0), "{text_run:?}");
    let stderr = String::from_utf8(text_run.stderr).expect("utf8");
    assert!(
        stderr.contains(BOOL_WRITE_RENDER),
        "a bool must trace as `true`, not `1`: {stderr}"
    );
    assert!(
        !stderr.contains(BOOL_CODEC_BYTE_LEAK),
        "the bool must not leak the codec byte `1`: {stderr}"
    );
}

#[test]
fn run_trace_renders_an_int_write_as_canonical_digits() {
    // A managed write of a non-bool scalar stores its canonical digit bytes with no
    // decode/encode round-trip: an `int` 42 is stored as the bytes `"42"`. The typed
    // record carries those bytes in `value_b64`, so decoding it back to `"42"` asserts
    // the canonical-digits contract reword-proof, on the bytes rather than the rendered
    // text.
    let project = temp_project("trace-int", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Counter\n\
             \x20\x20\x20\x20total: int\n\
             store ^counters(id: int): Counter\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^counters(1).total = 42\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = jsonl_trace_records(output.stderr);
    let write = records
        .iter()
        .find(|record| record["kind"] == "write" && record["path"] == "^counters(1).total")
        .expect("a field write record");
    assert_eq!(write["op"], json!("write"));
    assert_eq!(write["target"]["store"], json!("counters"));
    assert_eq!(write["target"]["path"], json!([{ "member": "total" }]));
    let bytes = marrow_run::base64::decode(write["value_b64"].as_str().expect("value_b64"))
        .expect("base64 value");
    assert_eq!(
        bytes, b"42",
        "an int stores its canonical digit bytes: {write}"
    );
}

#[test]
fn run_trace_reports_non_root_deletes() {
    let project = temp_project("trace-delete", |root| {
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
             \x20\x20\x20\x20details\n\
             \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
             store ^books(id: int): Book\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).details.note = \"gone\"\n\n\
             pub fn dropDetails()\n\
             \x20\x20\x20\x20delete ^books(1).details\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let output = marrow(&[
        "run",
        "--trace",
        "--format",
        "jsonl",
        "--entry",
        "app::dropDetails",
        &dir,
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = jsonl_trace_records(output.stderr);
    // The group delete is a typed delete op on the `details` member of `^books(1)`,
    // asserted on the record's op and target rather than the rendered path text.
    assert!(
        records.iter().any(|record| {
            record["kind"] == "write"
                && record["op"] == "delete"
                && record["target"]["kind"] == "data"
                && record["target"]["store"] == "books"
                && record["target"]["identity"] == json!(["1"])
                && record["target"]["path"] == json!([{ "member": "details" }])
        }),
        "trace must report the group delete: {records:?}"
    );
}

#[test]
fn an_untraced_run_emits_no_trace_and_matches_plain_run() {
    // Without `--trace` a run produces no trace and its stdout is exactly the
    // program's output — byte-identical to a plain run.
    let project = temp_project("trace-none", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hello\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let plain = marrow(&["run", &dir]);
    let traced_off = marrow(&["run", &dir]);

    assert_eq!(plain.status.code(), Some(0), "{plain:?}");
    assert_eq!(plain.stdout, traced_off.stdout);
    let stdout = String::from_utf8(plain.stdout).expect("utf8");
    assert_eq!(stdout, "hello\n");
    // Without --trace the trace stream is silent: a plain run emits nothing on stderr,
    // so no trace records leak into a consumer reading it.
    assert!(
        plain.stderr.is_empty(),
        "plain run emitted a trace: {:?}",
        String::from_utf8_lossy(&plain.stderr)
    );
}

#[test]
fn run_trace_json_emits_step_and_write_records() {
    let project = temp_project("trace-run-json", |root| {
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
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Trace records are tooling output on stderr; the program's stdout stays its own.
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    let records = jsonl(output.stderr);
    let [step, node_write, write, summary] = records.as_slice() else {
        panic!("expected step, node write, field write, and summary records: {records:?}");
    };

    assert_eq!(step["kind"], json!("step"));
    assert_eq!(step["trace"], json!(""));
    assert_eq!(step["line"], json!(8));
    assert_eq!(step["depth"], json!(1));
    assert!(
        step["file"]
            .as_str()
            .expect("step file")
            .ends_with("src/app.mw"),
        "{step}"
    );

    assert_eq!(node_write["kind"], json!("write"));
    assert_eq!(node_write["trace"], json!(""));
    assert_eq!(node_write["op"], json!("write"));
    assert_eq!(node_write["path"], json!("^books(1)"));
    assert_eq!(node_write["value_b64"], Value::Null);
    assert_eq!(node_write["depth"], json!(1));
    assert_eq!(node_write["target"]["kind"], json!("data"));
    assert_eq!(node_write["target"]["store"], json!("books"));
    assert_eq!(node_write["target"]["identity"], json!(["1"]));
    assert_eq!(node_write["target"]["path"], json!([]));

    assert_eq!(write["kind"], json!("write"));
    assert_eq!(write["trace"], json!(""));
    assert_eq!(write["op"], json!("write"));
    assert_eq!(write["path"], json!("^books(1).title"));
    assert_eq!(write["value_b64"], json!("TW9ydA=="));
    assert_eq!(write["depth"], json!(1));
    assert_eq!(write["target"]["kind"], json!("data"));
    assert_eq!(write["target"]["store"], json!("books"));
    assert_eq!(write["target"]["identity"], json!(["1"]));
    assert_eq!(write["target"]["path"], json!([{ "member": "title" }]));

    assert_eq!(summary["kind"], json!("summary"));
    assert_eq!(summary["trace"], json!(""));
    assert_eq!(summary["events"], json!(3));
}

#[test]
fn run_trace_jsonl_keeps_program_output_off_the_record_stream() {
    // A traced run that also prints must keep the two streams apart: stdout is the
    // program's own `print` output, and the JSONL trace records land on stderr.
    // A consumer parsing stdout as JSONL would otherwise choke on the program line.
    let project = temp_project("trace-jsonl-streams", |root| {
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
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"done\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "jsonl", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // The program output is exactly its `print`; no JSON record reached stdout.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "done\n", "program output must own stdout: {stdout}");

    // Every line of stderr is one trace record; the write record is present.
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    let records: Vec<Value> = stderr
        .lines()
        .map(|line| serde_json::from_str(line).expect("each stderr line is one JSONL record"))
        .collect();
    assert!(
        records
            .iter()
            .any(|record| record["kind"] == "write" && record["path"] == "^books(1).title"),
        "the write record must be on stderr: {records:?}"
    );
    assert!(
        records.iter().any(|record| record["kind"] == "summary"),
        "the summary record must be on stderr: {records:?}"
    );
}

#[test]
fn run_trace_jsonl_flushes_records_when_the_run_faults() {
    let project = faulting_print_project("trace-jsonl-fault");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "before fault\n");
    let records = json_records_in_stderr(output.stderr);
    assert!(
        records
            .iter()
            .any(|record| record["kind"] == "step" && record["line"] == 4),
        "faulting trace must include the print step: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|record| record["kind"] == "summary" && record["events"] == 2),
        "faulting trace must include a summary: {records:?}"
    );
}

#[test]
fn run_trace_json_flushes_the_envelope_when_the_run_faults() {
    let project = faulting_print_project("trace-json-fault");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "before fault\n");
    let records = json_records_in_stderr(output.stderr);
    let [trace] = records.as_slice() else {
        panic!("expected one trace JSON envelope before the fault: {records:?}");
    };
    let events = trace["events"].as_array().expect("trace events");
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "step" && event["line"] == 4),
        "faulting trace must include the print step: {trace}"
    );
    assert_eq!(events.len(), 2, "{trace}");
}

#[test]
fn test_trace_labels_each_test() {
    // Two tests, each traced; the trace stream attributes events to the right test
    // by name.
    let project = temp_project("trace-test", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
        );
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/suite.mw",
            "pub fn first()\n\
             \x20\x20\x20\x20std::assert::isTrue(true)\n\n\
             pub fn second()\n\
             \x20\x20\x20\x20std::assert::isTrue(true)\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["test", "--trace", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = jsonl_trace_records(output.stderr);
    let step_labels = records
        .iter()
        .filter(|record| record["kind"] == "step")
        .filter_map(|record| record["trace"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let summary_labels = records
        .iter()
        .filter(|record| record["kind"] == "summary")
        .filter_map(|record| record["trace"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for label in ["tests::suite::first", "tests::suite::second"] {
        assert!(step_labels.contains(label), "{records:?}");
        assert!(summary_labels.contains(label), "{records:?}");
    }
}

#[test]
fn run_trace_appears_in_help() {
    // `run --help` is human-rendered text; the golden here is the one fragment that
    // proves the trace flag is documented, not the whole help body.
    const TRACE_FLAG_IN_HELP: &str = "--trace";
    let output = marrow(&["run", "--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains(TRACE_FLAG_IN_HELP), "{stdout}");
}
