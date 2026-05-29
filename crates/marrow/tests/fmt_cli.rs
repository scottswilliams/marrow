use std::fs;
use std::process::Command;

fn temp_source(name: &str, source: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    path.push(format!("marrow-{name}-{}-{nanos}.mw", std::process::id()));
    fs::write(&path, source).expect("write source");
    path
}

fn run_fmt(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("fmt")
        .args(args)
        .output()
        .expect("run marrow fmt")
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
fn fmt_refuses_to_format_source_with_errors() {
    let path = temp_source("broken", "module app\n\tconst Max: int = 5\n");
    let output = run_fmt(&[path.to_str().unwrap()]);
    let unchanged = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse.syntax"), "{stderr}");
    // The file is left untouched when it does not parse.
    assert_eq!(unchanged, "module app\n\tconst Max: int = 5\n");
}

/// A temp project directory with a `marrow.json` selecting `src` and one `.mw`
/// file written there.
fn temp_project(name: &str, relative: &str, source: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project");
    fs::write(root.join("marrow.json"), r#"{ "sourceRoots": ["src"] }"#).expect("write config");
    let file = root.join(relative);
    fs::create_dir_all(file.parent().unwrap()).expect("create dirs");
    fs::write(&file, source).expect("write source");
    root
}

#[test]
fn fmt_check_on_a_project_directory_passes_when_all_files_are_formatted() {
    let project = temp_project(
        "fmt-proj-ok",
        "src/app.mw",
        "module app\n\nconst Max: int = 5\n",
    );
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    fs::remove_dir_all(&project).ok();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn fmt_check_on_a_project_directory_fails_when_a_file_is_unformatted() {
    let project = temp_project(
        "fmt-proj-bad",
        "src/app.mw",
        "module app\nconst Max:int=5\n",
    );
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    fs::remove_dir_all(&project).ok();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("not formatted"), "{stderr}");
}

#[test]
fn fmt_write_on_a_project_directory_rewrites_each_changed_file() {
    let project = temp_project(
        "fmt-proj-write",
        "src/app.mw",
        "module app\nconst Max:int=5\n",
    );
    let output = run_fmt(&["--write", project.to_str().unwrap()]);
    let written = fs::read_to_string(project.join("src/app.mw")).expect("read back");
    fs::remove_dir_all(&project).ok();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(written, "module app\n\nconst Max: int = 5\n");
}

#[test]
fn fmt_on_a_directory_with_no_config_reports_a_typed_error_not_is_a_directory() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "marrow-fmt-noconfig-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create dir");
    let output = run_fmt(&["--check", dir.to_str().unwrap()]);
    fs::remove_dir_all(&dir).ok();

    // A directory with no marrow.json is a typed error about the config file
    // (exit 1, matching `check`'s `io.read` precedent), not a raw OS "Is a
    // directory" from blindly reading the directory as a source file.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("marrow.json"), "{stderr}");
    assert!(!stderr.contains("Is a directory"), "{stderr}");
}

#[test]
fn fmt_on_a_directory_with_invalid_config_reports_a_config_error() {
    let project = temp_project("fmt-proj-badconfig", "src/app.mw", "module app\n");
    // Overwrite with an invalid config (no source roots) to force a `config.*`.
    fs::write(project.join("marrow.json"), r#"{ "sourceRoots": [] }"#).expect("write config");
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config."), "{stderr}");
}

#[test]
fn fmt_of_a_bare_directory_requires_check_or_write() {
    let project = temp_project("fmt-proj-bare", "src/app.mw", "module app\n");
    let output = run_fmt(&[project.to_str().unwrap()]);
    fs::remove_dir_all(&project).ok();
    // Concatenating many formatted files to stdout is meaningless; require a mode.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn fmt_rejects_stdin_dash_cleanly() {
    let output = run_fmt(&["-"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("stdin"), "{stderr}");
}
