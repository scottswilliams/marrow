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
    assert_eq!(records.len(), 2, "{records:#?}");
    assert_eq!(records[0]["code"], "parse.syntax");
    assert_eq!(records[0]["kind"], "parse");
    assert_eq!(records[0]["source_span"]["line"], 2);
    assert_eq!(records[0]["source_span"]["column"], 1);
    assert_eq!(records[1]["kind"], "summary");
    assert_eq!(records[1]["status"], "failed");
    assert_eq!(records[1]["diagnostics"], 1);
}
