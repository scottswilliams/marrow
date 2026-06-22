//! CLI usage-failure tier (exit code 2). Exit code 2 means a command-line
//! usage failure before the command body ran, so each case must exit 2 and
//! leave nothing created or executed.

use crate::support;
use std::fs;
use std::path::Path;
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
fn bare_marrow_is_a_usage_failure() {
    // A bare `marrow` ran no command, so it must exit 2 (usage), not 0: a CI line that forgot the
    // subcommand word must fail rather than pass green. The usage text goes to stderr.
    let output = marrow(&[]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "bare marrow prints usage on stderr, not stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("Usage:"),
        "bare marrow prints usage: {stderr}"
    );
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
fn removed_surface_and_lsp_commands_are_usage_failures() {
    for command in ["surface", "lsp"] {
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
            "{command} should stay removed from dispatch: {stderr}"
        );
    }
}

#[test]
fn help_does_not_advertise_removed_lsp_or_surface_alias_commands() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(!stdout.contains("marrow lsp"), "{stdout}");
    assert!(!stdout.contains("surface serve"), "{stdout}");
    assert!(!stdout.contains("surface client"), "{stdout}");
    assert!(
        stdout.contains("marrow serve "),
        "serve should be canonical top-level help: {stdout}"
    );
    assert!(
        stdout.contains("marrow client typescript "),
        "client generation should be canonical top-level help: {stdout}"
    );
}

#[test]
fn version_prints_engine_profile_tuple() {
    let output = marrow(&["--version"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let profile = marrow_run::evolution::current_engine_profile();
    assert_eq!(
        stdout,
        format!(
            "marrow 0.1.0 engine-profile=(key=v{}, layout-epoch={}, digest={})\n",
            profile.key_profile_version(),
            profile.layout_epoch(),
            profile.digest_hex()
        )
    );
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
fn top_level_help_advertises_doctor() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("marrow doctor [--format text|json|jsonl] <projectdir>"),
        "{stdout}"
    );
}

#[test]
fn top_level_help_advertises_check_locked() {
    // The top-level usage must list `--locked` for check, the CI lockfile gate, just as the
    // `check --help` subcommand help does; otherwise the flag is discoverable only by reading
    // the subcommand help.
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let check_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("marrow check "))
        .expect("a check usage line");
    assert!(
        check_line.contains("[--locked]"),
        "the top-level check usage must advertise --locked: {stdout}"
    );
}

#[test]
fn top_level_help_advertises_run_arg_and_test_filter() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let run_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("marrow run "))
        .expect("a run usage line");
    assert!(
        run_line.contains("[--arg name=value]..."),
        "run usage must advertise --arg: {stdout}"
    );
    let test_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("marrow test "))
        .expect("a test usage line");
    assert!(
        test_line.contains("[--filter <substring>]"),
        "test usage must advertise --filter: {stdout}"
    );
}

#[test]
fn top_level_help_teaches_field_path_form_of_approve_retire() {
    // The everyday teaching form of --approve-retire is the field path the rest of the surface
    // resolves, never the internal catalog id, so the top-level synopsis must match.
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let apply_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("marrow evolve apply "))
        .expect("an evolve apply usage line");
    assert!(
        apply_line.contains("[--approve-retire <field-path>:<count>]"),
        "evolve apply usage must teach the field-path form of --approve-retire: {stdout}"
    );
    assert!(
        !apply_line.contains("<catalog-id>"),
        "evolve apply usage must not teach the internal catalog id: {stdout}"
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
