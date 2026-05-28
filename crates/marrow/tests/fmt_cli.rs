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
