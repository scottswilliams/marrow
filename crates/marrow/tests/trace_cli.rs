mod support;

use serde_json::{Value, json};

use support::{jsonl, marrow, temp_project, write};

/// Parse the trace record stream a `--format jsonl` run emits on stderr. Every
/// line is one JSON record; the pass/fail report stays on stdout, so the trace
/// stream needs no filtering.
fn jsonl_trace_records(stderr: Vec<u8>) -> Vec<Value> {
    jsonl(stderr)
}

#[test]
fn run_trace_interleaves_steps_and_writes() {
    // An entry that writes a field then returns. With `--trace`, the trace stream
    // reports the writing statement, the write it produced, and the return — a
    // step, then a write, then a step — and the program's own output still lands on
    // stdout. The trace goes to stderr under text format.
    let project = temp_project("trace-run", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20title: string\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"done\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    // The program's own output is unaffected.
    assert!(stdout.contains("done"), "stdout: {stdout}");
    // The trace names the file and the write to the title field, with the write
    // appearing after the statement that produced it.
    assert!(stderr.contains("app.mw"), "trace: {stderr}");
    assert!(stderr.contains("^books(1).title"), "trace: {stderr}");
    // The writing statement (the `^books(1).title = ...` line) is reported before
    // the write it produces, which is in turn before the `print("done")` step.
    let write_at = stderr.find("write ^books(1).title").expect("write line");
    let writing_step = stderr
        .find("app.mw:7")
        .expect("the writing statement's line");
    let print_step = stderr.find("app.mw:8").expect("the print statement's line");
    assert!(
        writing_step < write_at && write_at < print_step,
        "step, then its write, then the next step: {stderr}"
    );
}

#[test]
fn run_trace_renders_a_bool_write_as_its_typed_value() {
    // A managed write of a `bool` field traces as `true`, not the codec byte `1`:
    // the trace renders the leaf value through its declared scalar type.
    let project = temp_project("trace-bool", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Flag at ^flags(id: int)\n\
             \x20\x20\x20\x20on: bool\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^flags(1).on = true\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("write ^flags(1).on = true"),
        "a bool must trace as `true`, not `1`: {stderr}"
    );
    assert!(
        !stderr.contains("^flags(1).on = 1"),
        "the bool must not leak the codec byte `1`: {stderr}"
    );
}

#[test]
fn run_trace_renders_an_int_write_as_canonical_digits() {
    // A managed write of a non-bool scalar renders straight from its stored bytes,
    // with no decode/encode round-trip: an `int` traces as its canonical digits.
    let project = temp_project("trace-int", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Counter at ^counters(id: int)\n\
             \x20\x20\x20\x20total: int\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^counters(1).total = 42\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("write ^counters(1).total = 42"),
        "an int must trace as its canonical digits: {stderr}"
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
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20details\n\
             \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).details.note = \"gone\"\n\n\
             pub fn dropDetails()\n\
             \x20\x20\x20\x20delete ^books(1).details\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let output = marrow(&["run", "--trace", "--entry", "app::dropDetails", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("delete ^books(1).details"),
        "trace must report the group delete: {stderr}"
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
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
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

    assert_eq!(plain.stdout, traced_off.stdout);
    let stdout = String::from_utf8(plain.stdout).expect("utf8");
    assert_eq!(stdout, "hello\n");
    let stderr = String::from_utf8(plain.stderr).expect("utf8");
    // No trace lines on a plain run.
    assert!(
        !stderr.contains("step"),
        "plain run emitted a trace: {stderr}"
    );
}

#[test]
fn run_trace_json_emits_step_and_write_records() {
    let project = temp_project("trace-run-json", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20title: string\n\n\
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
    let [step, write, summary] = records.as_slice() else {
        panic!("expected step, write, and summary records: {records:?}");
    };

    assert_eq!(step["kind"], json!("step"));
    assert_eq!(step["trace"], json!(""));
    assert_eq!(step["line"], json!(7));
    assert_eq!(step["depth"], json!(1));
    assert!(
        step["file"]
            .as_str()
            .expect("step file")
            .ends_with("src/app.mw"),
        "{step}"
    );

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
    assert_eq!(summary["events"], json!(2));
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
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20title: string\n\n\
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
    let output = marrow(&["run", "--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("--trace"), "{stdout}");
}
