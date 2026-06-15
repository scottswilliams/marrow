use crate::support;
use serde_json::Value;

use support::{json_records_in_stderr, marrow, temp_project, write};

fn broken_config_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": [] }"#);
    })
}

fn assert_config_invalid_stdout_envelope(output: std::process::Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let envelope: Value = serde_json::from_str(stdout.trim()).expect("json envelope");
    assert_eq!(envelope["code"], "config.invalid");
    assert_eq!(envelope["kind"], "tooling");
    assert_eq!(envelope["data"], serde_json::json!({}));
}

#[test]
fn check_format_json_renders_broken_config_as_an_envelope() {
    let project = broken_config_project("format-matrix-check-bad-config");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["check", "--format", "json", dir]);

    assert_config_invalid_stdout_envelope(output);
}

#[test]
fn evolve_format_json_renders_broken_config_as_an_envelope() {
    let project = broken_config_project("format-matrix-evolve-bad-config");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["evolve", "preview", "--format", "json", dir]);

    assert_config_invalid_stdout_envelope(output);
}

#[test]
fn test_format_json_renders_broken_config_as_an_envelope() {
    let project = broken_config_project("format-matrix-test-bad-config");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["test", "--format", "json", dir]);

    assert_config_invalid_stdout_envelope(output);
}

#[test]
fn data_format_json_renders_broken_config_as_an_envelope() {
    let project = broken_config_project("format-matrix-data-bad-config");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["data", "roots", "--format", "json", dir]);

    assert_config_invalid_stdout_envelope(output);
}

#[test]
fn run_dry_run_format_json_renders_broken_config_as_an_envelope() {
    let project = broken_config_project("format-matrix-run-bad-config");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["run", "--dry-run", "--format", "json", dir]);

    assert_config_invalid_stdout_envelope(output);
}

#[test]
fn run_rejects_jsonl_format() {
    let project = broken_config_project("format-matrix-run-jsonl");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["run", "--dry-run", "--format", "jsonl", dir]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown format: jsonl"), "{stderr}");
    assert!(!stderr.contains("config.invalid"), "{stderr}");
}

#[test]
fn run_trace_rejects_non_text_format() {
    let project = broken_config_project("format-matrix-run-trace-json");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["run", "--trace", "--format", "json", dir]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--trace") && stderr.contains("text"),
        "{stderr}"
    );
}

#[test]
fn run_trace_rejects_text_format_without_loading_project() {
    let project = broken_config_project("format-matrix-run-trace-text");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["run", "--trace", "--format", "text", dir]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--format") && stderr.contains("--dry-run"),
        "{stderr}"
    );
    assert!(!stderr.contains("config.invalid"), "{stderr}");
}

#[test]
fn backup_rejects_format_flag() {
    let project = broken_config_project("format-matrix-backup-format");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["backup", "--format", "json", dir, "backup.mar"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("unknown backup option: --format"),
        "{stderr}"
    );
}

#[test]
fn restore_rejects_format_flag() {
    let project = broken_config_project("format-matrix-restore-format");
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["restore", "--format", "json", dir, "backup.mar"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("unknown restore option: --format"),
        "{stderr}"
    );
}

#[test]
fn run_uncaught_error_json_envelope_carries_the_thrown_code_in_data() {
    let project = temp_project("format-matrix-run-uncaught-json", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let dir = project.to_str().expect("project path utf8");

    let output = marrow(&["run", "--dry-run", "--format", "json", dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = json_records_in_stderr(output.stderr);
    let envelope = records.last().expect("runtime error envelope");
    assert_eq!(envelope["code"], "run.uncaught_error");
    assert_eq!(envelope["kind"], "runtime");
    assert_eq!(envelope["data"]["code"], "book.absent");
}
