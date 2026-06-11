//! A tampered or crash-damaged native store must fail every store-opening command
//! closed with a typed `store.*` code and exit 1, never abort the process (exit
//! 101) on a redb panic. A truncated body is hard corruption; a store left needing
//! repair by an unclean shutdown is the typed recoverable status, with a Marrow
//! message rather than a raw redb string.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

mod support;
mod support_data;

use support::{find_code_segment, marrow, write};
use support_data::{native_project, seeded_project};

/// The native store file a seeded project writes its data into.
fn store_path(project: &Path) -> std::path::PathBuf {
    project.join(".data").join("marrow.redb")
}

/// Truncate the store body below its recorded length: a valid header over a damaged
/// body, the shape that drives redb's open path into a panic without the backstop.
fn truncate_store_body(project: &Path) {
    let path = store_path(project);
    let len = std::fs::metadata(&path).expect("store metadata").len();
    assert!(len > 4096, "a seeded store should exceed one redb page");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open store for truncation");
    file.set_len(4096).expect("truncate store body");
}

fn flip_recovery_flag(project: &Path) {
    let path = store_path(project);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open store header");
    file.seek(SeekFrom::Start(9))
        .expect("seek to recovery flag");
    let mut byte = [0u8; 1];
    {
        use std::io::Read;
        file.read_exact(&mut byte).expect("read recovery flag");
    }
    file.seek(SeekFrom::Start(9)).expect("seek back");
    file.write_all(&[byte[0] ^ 0x01])
        .expect("flip recovery flag");
}

/// The dotted code a `marrow` command printed on stderr, located the same way the
/// CLI's own fault grammar reports it. The fault is the last non-empty stderr line,
/// so any preamble cannot displace the located code.
fn stderr_code(output: &std::process::Output) -> String {
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
    let fault = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a fault line on stderr");
    let segments: Vec<&str> = fault.split(": ").collect();
    let (_, code) = find_code_segment(&segments);
    code.to_string()
}

fn assert_locked_output(command: &[&str], output: &std::process::Output) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "`marrow {}` must exit 1 when the store is locked: {output:?}",
        command.join(" ")
    );
    assert_eq!(
        stderr_code(output),
        "store.locked",
        "`marrow {}` must report store.locked: {output:?}",
        command.join(" ")
    );
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
    assert!(
        stderr.contains("held open by another process"),
        "`marrow {}` must explain the cross-process holder: {stderr}",
        command.join(" ")
    );
    assert!(
        stderr.contains("writer or a read-only inspection"),
        "`marrow {}` must name both lock holder classes: {stderr}",
        command.join(" ")
    );
}

/// A store-opening command over a truncated store exits 1 with `store.corruption`,
/// never 101 from a redb panic. Run over every store-opening verb, read and write.
#[test]
fn store_opening_commands_report_corruption_not_a_panic() {
    let (project, dir) = seeded_project("cli-store-corruption");
    truncate_store_body(&project);

    let backup_target = project.join("backup.mw-backup");
    let backup_target = backup_target.to_str().expect("backup path utf8");
    let commands: &[&[&str]] = &[
        &["data", "dump", &dir],
        &["data", "recover", &dir],
        &["data", "integrity", &dir],
        &["data", "stats", &dir],
        &["run", "--entry", "app::seed", &dir],
        &["backup", &dir, backup_target],
    ];

    for command in commands {
        let output = marrow(command);
        let code = output.status.code();
        assert_ne!(
            code,
            Some(101),
            "`marrow {}` must not abort on a redb panic: {output:?}",
            command.join(" ")
        );
        assert_eq!(
            code,
            Some(1),
            "`marrow {}` must exit 1 on a corrupt store: {output:?}",
            command.join(" ")
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`marrow {}` must report store.corruption: {output:?}",
            command.join(" ")
        );
        let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
        assert!(
            !stderr.contains("panicked at") && !stderr.contains("RUST_BACKTRACE"),
            "`marrow {}` must not leak a redb panic backtrace: {stderr}",
            command.join(" ")
        );
    }
}

/// A write-capable native handle excludes read-only CLI inspections. Every read-only
/// store-opening command must surface the typed contract, not a redb-specific string
/// or a generic I/O failure.
#[test]
fn read_only_cli_commands_report_locked_while_writer_is_open() {
    let (project, dir) = seeded_project("cli-store-writer-locks-readers");
    let _writer = marrow_store::tree::TreeStore::open(&store_path(&project))
        .expect("hold the native writer open");
    let backup_target = project.join("held.mw-backup");
    let backup_target = backup_target.to_str().expect("backup path utf8");
    let commands: &[&[&str]] = &[
        &["data", "dump", &dir],
        &["data", "stats", &dir],
        &["data", "integrity", &dir],
        &["backup", &dir, backup_target],
    ];

    for command in commands {
        let output = marrow(command);
        assert_locked_output(command, &output);
    }
}

/// A read-only native holder also excludes write-capable CLI commands. The contract
/// is symmetric at the process boundary even though many read-only handles can coexist.
#[test]
fn write_cli_commands_report_locked_while_read_only_holder_lives() {
    let (project, dir) = seeded_project("cli-store-reader-locks-writers");
    let _reader = marrow_store::tree::TreeStore::open_read_only(&store_path(&project))
        .expect("hold the native reader open");
    let commands: &[&[&str]] = &[
        &["run", "--entry", "app::seed", &dir],
        &["data", "recover", &dir],
    ];

    for command in commands {
        let output = marrow(command);
        assert_locked_output(command, &output);
    }
}

/// A store left needing repair by an unclean shutdown surfaces the typed
/// `store.recovery_required` on a read-only command, with a Marrow-authored guiding
/// message — not redb's raw `Database repair aborted.` string — and exits 1.
#[test]
fn read_only_command_reports_recovery_required_for_an_unclean_store() {
    let (project, dir) = seeded_project("cli-store-recovery");

    // redb records "recovery required" in the file header; flipping that flag bit
    // reproduces an unclean shutdown without touching tree data.
    flip_recovery_flag(&project);

    let output = marrow(&["data", "dump", &dir]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        stderr_code(&output),
        "store.recovery_required",
        "{output:?}"
    );
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8");
    assert!(
        !stderr.to_lowercase().contains("repair aborted"),
        "recovery message must be Marrow-authored, not a raw redb string: {stderr}"
    );
}

/// `marrow data recover` is the explicit write-capable repair path for a store
/// that read-only commands report as `store.recovery_required`. It must not check
/// project source before repair: damaged source text must not block the store open.
#[test]
fn data_recover_repairs_an_unclean_store_without_checking_source_first() {
    let (project, dir) = seeded_project("cli-store-recover");
    flip_recovery_flag(&project);
    write(&project, "src/app.mw", "module app\npub fn main(\n");

    let output = marrow(&["data", "recover", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    marrow_store::tree::TreeStore::open_read_only(&store_path(&project))
        .expect("recover should leave the store readable");
}

/// A healthy seeded store still serves a read-only command, so the backstop and the
/// new error mapping add no regression for the clean path.
#[test]
fn a_clean_store_still_serves_a_read_only_command() {
    let (_project, dir) = seeded_project("cli-store-clean");
    let output = marrow(&["data", "stats", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

/// A project with no store file yet is a first run, not a corrupt store: a read-only
/// command reports an empty store rather than a corruption fault.
#[test]
fn a_missing_store_file_is_a_first_run_not_corruption() {
    let project = native_project("cli-store-missing");
    let dir = project.to_str().expect("dir utf8").to_string();
    // No `run` has created the store yet.
    assert!(!store_path(&project).exists());

    let output = marrow(&["data", "stats", &dir]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a first run with no store is not corruption: {output:?}"
    );
    // A sanity check that the fixture really configures a native store path.
    write(&project, ".keep", "");
}

/// Recovery is a repair operation over an existing store, not a first-run creator:
/// with no native file on disk it exits successfully and leaves the store absent.
#[test]
fn data_recover_on_a_missing_store_does_not_create_one() {
    let project = native_project("cli-store-recover-missing");
    let dir = project.to_str().expect("dir utf8").to_string();
    assert!(!store_path(&project).exists());

    let output = marrow(&["data", "recover", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !store_path(&project).exists(),
        "recover must not create a missing store"
    );
}

/// An existing empty native file is not an absent first-run store. Recovery must
/// reject it as non-store data instead of initializing a fresh redb database.
#[test]
fn data_recover_rejects_an_empty_native_store_file() {
    let project = native_project("cli-store-recover-empty");
    let dir = project.to_str().expect("dir utf8").to_string();
    let path = store_path(&project);
    std::fs::create_dir_all(path.parent().expect("store parent")).expect("create store dir");
    std::fs::File::create(&path).expect("create empty store file");
    assert_eq!(std::fs::metadata(&path).expect("store metadata").len(), 0);

    let output = marrow(&["data", "recover", &dir]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(stderr_code(&output), "store.corruption", "{output:?}");
    assert_eq!(
        std::fs::metadata(&path).expect("store metadata").len(),
        0,
        "recover must not initialize an empty native file"
    );
}
