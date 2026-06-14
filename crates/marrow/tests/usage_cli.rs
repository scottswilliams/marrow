//! CLI usage-failure tier (exit code 2). Exit code 2 means a command-line
//! usage failure before the command body ran, so each case must exit 2 and
//! leave nothing created or executed.

use std::fs;
use std::path::Path;

mod support;

use support::marrow;

fn project_that_must_not_be_loaded(name: &str) -> support::TempProject {
    let dir = support::temp_dir(name);
    fs::write(dir.join("marrow.json"), support::native_config()).expect("write config");
    fs::create_dir_all(dir.join("src")).expect("create src dir");
    fs::write(dir.join("src/app.mw"), "module app\npub fn broken(\n")
        .expect("write invalid source");
    dir
}

#[test]
fn an_unknown_subcommand_is_a_usage_failure() {
    let output = marrow(&["frobnicate"]);
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
fn removed_server_commands_are_usage_failures() {
    for command in ["serve", "lsp"] {
        let output = marrow(&[command]);
        assert_eq!(output.status.code(), Some(2), "{command}: {output:?}");
        assert!(
            output.stdout.is_empty(),
            "{command} unexpected stdout: {:?}",
            output.stdout
        );
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            stderr.contains("unknown command"),
            "{command} should be removed from dispatch: {stderr}"
        );
    }
}

#[test]
fn help_does_not_advertise_removed_server_commands() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    for command in ["serve", "lsp"] {
        assert!(!stdout.contains(&format!("marrow {command}")), "{stdout}");
    }
}

#[test]
fn top_level_help_advertises_backup_read_targets() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains(
            "marrow data <roots|stats|dump|integrity> [--backup <artifact>] [--format text|json|jsonl] <projectdir>"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("marrow data recover [--format text|json|jsonl] <projectdir>"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "marrow data get [--backup <artifact>] [--format text|json|jsonl] <projectdir> <path>"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>"
        ),
        "{stdout}"
    );
}

#[test]
fn run_with_no_project_dir_is_a_usage_failure() {
    let output = marrow(&["run"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
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
fn test_help_names_stdout_report_and_stderr_trace_streams() {
    let output = marrow(&["test", "--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("--format"),
        "help should describe --format: {stdout}"
    );
    assert!(
        stdout.contains("test report on stdout"),
        "help should name stdout report stream: {stdout}"
    );
    assert!(
        stdout.contains("trace") && stdout.contains("stderr"),
        "help should name stderr trace stream: {stdout}"
    );
}

#[test]
fn an_unknown_data_subcommand_is_a_usage_failure_that_opens_no_store() {
    // A native-store project: if the body ran, `data` would open/create the
    // store. The unknown subcommand must be rejected before any store access.
    let dir = support::temp_dir("usage-data");
    fs::write(
        dir.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "data" } }"#,
    )
    .expect("write config");

    let output = marrow(&["data", "bogus", dir.to_str().unwrap()]);
    let store_created = Path::new(dir.path()).join("data").exists();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown data subcommand"), "{stderr}");
    assert!(!store_created, "data dir should not have been created");
}

#[test]
fn removed_data_flag_is_an_unknown_option_before_project_load() {
    let dir = project_that_must_not_be_loaded("usage-check-data");

    let output = marrow(&["check", "--data", dir.to_str().unwrap()]);
    let store_created = dir.join(".data").exists();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unknown check option must not render a check body report: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown check option: --data"), "{stderr}");
    assert!(
        !stderr.contains("check."),
        "usage parsing should fail before project diagnostics render: {stderr}"
    );
    assert!(
        !store_created,
        "unknown option must not open/create the store"
    );
}

#[test]
fn removed_json_data_flag_is_an_unknown_option_before_project_load() {
    let dir = project_that_must_not_be_loaded("usage-check-json-data");

    let output = marrow(&["check", "--format", "json", "--data", dir.to_str().unwrap()]);
    let store_created = dir.join(".data").exists();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unknown check option must not render JSON data-check output: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown check option: --data"), "{stderr}");
    assert!(
        !stderr.trim_start().starts_with('{'),
        "usage errors stay on stderr as usage text, not JSON: {stderr}"
    );
    assert!(
        !store_created,
        "unknown option must not open/create the store"
    );
}
