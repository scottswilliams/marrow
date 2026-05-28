use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn run_check(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("check")
        .args(args)
        .output()
        .expect("run marrow check")
}

#[test]
fn checks_a_clean_project_directory() {
    let root = temp_project("proj-clean", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(root, "src/shelf/books.mw", "module shelf::books\n");
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let output = run_check(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn reports_project_module_path_mismatch() {
    let root = temp_project("proj-mismatch", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_check(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.module_path"), "{stderr}");
}

#[test]
fn reports_project_check_as_jsonl() {
    let root = temp_project("proj-jsonl", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_check(&["--format", "jsonl", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let records: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("jsonl record"))
        .collect();
    assert_eq!(records[0]["code"], "check.module_path");
    assert_eq!(records.last().unwrap()["kind"], "summary");
    assert_eq!(records.last().unwrap()["status"], "failed");
}

#[test]
fn surfaces_a_parse_error_in_a_project_file_with_its_path() {
    let root = temp_project("proj-parse", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        // A tab is a lexical error.
        write(root, "src/bad.mw", "module bad\n\tconst X: int = 1\n");
    });
    let output = run_check(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    assert!(stderr.contains("bad.mw"), "{stderr}");
}

#[test]
fn reports_missing_marrow_json() {
    let root = temp_project("proj-noconfig", |root| {
        write(root, "src/main.mw", "fn main()\n    return\n");
    });
    let output = run_check(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("io.read"), "{stderr}");
}
