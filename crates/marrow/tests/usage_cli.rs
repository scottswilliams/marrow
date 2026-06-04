//! CLI usage-failure tier (exit code 2). Exit code 2 means a command-line
//! usage failure before the command body ran, so each case must exit 2 and
//! leave nothing created or executed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn marrow(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("create dir");
    dir
}

#[test]
fn an_unknown_subcommand_is_a_usage_failure() {
    let output = marrow(&["frobnicate"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    // Nothing executed: the help banner (the success path) was not printed.
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown command"), "{stderr}");
}

#[test]
fn the_catalog_command_is_absent() {
    // The catalog is written transparently by run and evolve apply; there is no
    // user-facing catalog command. It must be rejected like any unknown command,
    // before any project work.
    let output = marrow(&["catalog", "preview", "."]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown command"), "{stderr}");
}

#[test]
fn run_with_no_project_dir_is_a_usage_failure() {
    let output = marrow(&["run"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    // The command body never ran, so it produced no program output.
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("missing project directory"), "{stderr}");
}

#[test]
fn run_entry_with_no_value_is_a_usage_failure() {
    let output = marrow(&["run", "--entry"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("missing value for --entry"), "{stderr}");
}

#[test]
fn an_unknown_data_subcommand_is_a_usage_failure_that_opens_no_store() {
    // A native-store project: if the body ran, `data` would open/create the
    // store. The unknown subcommand must be rejected before any store access.
    let dir = temp_dir("usage-data");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "data" } }"#,
    )
    .expect("write config");

    let output = marrow(&["data", "bogus", dir.to_str().unwrap()]);
    let store_created = Path::new(&dir).join("data").exists();
    fs::remove_dir_all(&dir).ok();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown data subcommand"), "{stderr}");
    assert!(!store_created, "data dir should not have been created");
}
