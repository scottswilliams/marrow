use std::fs;

mod support;

use support::temp_source;

fn run_fmt(args: &[&str]) -> std::process::Output {
    support::marrow_sub("fmt", args)
}

#[test]
fn fmt_prints_canonical_source_to_stdout() {
    // Extra blank lines and tight spacing get normalized.
    let path = temp_source(
        "messy",
        "module app\n\n\nconst Max:int=5\nfn main()\n    return Max\n",
    );
    let output = run_fmt(&[path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(
        stdout,
        "module app\n\nconst Max: int = 5\n\nfn main()\n    return Max\n"
    );
}

#[test]
fn fmt_check_succeeds_on_already_formatted_source() {
    let formatted = "module app\n\nconst Max: int = 5\n";
    let path = temp_source("formatted", formatted);
    let output = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(0), "{:?}", output);
}

#[test]
fn fmt_check_fails_on_unformatted_source() {
    let path = temp_source("unformatted", "module app\nconst Max:int=5\n");
    let output = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("not formatted"), "{stderr}");
}

#[test]
fn fmt_write_rewrites_the_file_in_place() {
    let path = temp_source("write", "module app\nconst Max:int=5\n");
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();
    assert_eq!(written, "module app\n\nconst Max: int = 5\n");
}

#[test]
fn fmt_rejects_check_with_write_without_rewriting() {
    let source = "module app\nconst Max:int=5\n";
    let path = temp_source("check-write", source);
    let output = run_fmt(&["--check", "--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(written, source);
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--check"), "{stderr}");
    assert!(stderr.contains("--write"), "{stderr}");
}

#[test]
fn fmt_rejects_duplicate_mode_flags() {
    let path = temp_source("dupe-check", "module app\n\nconst Max: int = 5\n");
    let output = run_fmt(&["--check", "--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--check"), "{stderr}");
}

#[test]
fn fmt_refuses_to_format_source_with_errors() {
    let path = temp_source("broken", "module app\n\tconst Max: int = 5\n");
    let output = run_fmt(&[path.to_str().unwrap()]);
    let unchanged = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    assert_eq!(unchanged, "module app\n\tconst Max: int = 5\n");
}

#[test]
fn fmt_write_refuses_unexpected_indentation_without_rewriting() {
    let source = "module app\nfn main()\n    print(\"kept\")\n        print(\"over-indented\")\n";
    let path = temp_source("over-indented", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let unchanged = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    assert_eq!(unchanged, source);
}

fn temp_project(name: &str, files: &[(&str, &str)]) -> support::TempProject {
    support::temp_project_uncommitted(name, |root| {
        support::write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        for (relative, source) in files {
            support::write(root, relative, source);
        }
    })
}

#[test]
fn fmt_check_on_a_project_directory_passes_when_all_files_are_formatted() {
    let project = temp_project(
        "fmt-proj-ok",
        &[
            ("src/app.mw", "module app\n\nconst Max: int = 5\n"),
            ("src/lib.mw", "module lib\n\nconst Limit: int = 10\n"),
        ],
    );
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn fmt_check_on_a_project_directory_fails_when_a_file_is_unformatted() {
    let project = temp_project(
        "fmt-proj-bad",
        &[("src/app.mw", "module app\nconst Max:int=5\n")],
    );
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("not formatted"), "{stderr}");
}

#[test]
fn fmt_write_on_a_project_directory_rewrites_each_changed_file() {
    let project = temp_project(
        "fmt-proj-write",
        &[
            ("src/app.mw", "module app\nconst Max:int=5\n"),
            ("src/lib.mw", "module lib\nconst Limit:int=10\n"),
        ],
    );
    let output = run_fmt(&["--write", project.to_str().unwrap()]);
    let app = fs::read_to_string(project.join("src/app.mw")).expect("read app");
    let lib = fs::read_to_string(project.join("src/lib.mw")).expect("read lib");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(app, "module app\n\nconst Max: int = 5\n");
    assert_eq!(lib, "module lib\n\nconst Limit: int = 10\n");
}

#[test]
fn fmt_on_a_directory_with_no_config_reports_io_read_for_config() {
    let dir = support::temp_dir("fmt-noconfig");
    let output = run_fmt(&["--check", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("io.read"), "{stderr}");
    assert!(stderr.contains("marrow.json"), "{stderr}");
}

#[test]
fn fmt_on_a_directory_with_invalid_config_reports_a_config_error() {
    let project = temp_project("fmt-proj-badconfig", &[("src/app.mw", "module app\n")]);
    fs::write(project.join("marrow.json"), r#"{ "sourceRoots": [] }"#).expect("write config");
    let output = run_fmt(&["--check", project.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config."), "{stderr}");
}

#[test]
fn fmt_of_a_bare_directory_requires_check_or_write() {
    let project = temp_project("fmt-proj-bare", &[("src/app.mw", "module app\n")]);
    let output = run_fmt(&[project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn fmt_rejects_stdin_dash_cleanly() {
    let output = run_fmt(&["-"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("stdin"), "{stderr}");
}
