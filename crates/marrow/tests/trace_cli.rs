use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    support::commit_catalog_if_clean(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
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
    fs::remove_dir_all(&project).ok();

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
    fs::remove_dir_all(&project).ok();

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
    fs::remove_dir_all(&project).ok();

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
    fs::remove_dir_all(&project).ok();

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
    fs::remove_dir_all(&project).ok();

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
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let kinds: Vec<String> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| value["kind"].as_str().map(str::to_string))
        .collect();
    assert!(kinds.iter().any(|k| k == "step"), "{stdout}");
    assert!(kinds.iter().any(|k| k == "write"), "{stdout}");
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
    let output = marrow(&["test", "--trace", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let combined = format!(
        "{}{}",
        String::from_utf8(output.stdout).expect("utf8"),
        String::from_utf8(output.stderr).expect("utf8")
    );
    assert!(combined.contains("::first"), "{combined}");
    assert!(combined.contains("::second"), "{combined}");
}

#[test]
fn run_trace_appears_in_help() {
    let output = marrow(&["run", "--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("--trace"), "{stdout}");
}
