//! A tampered or crash-damaged native store must fail every store-opening command
//! closed with a typed `store.*` code and exit 1, never abort the process (exit
//! 101) on a redb panic. A truncated body is hard corruption; a store left needing
//! repair by an unclean shutdown is the typed recoverable status, with a Marrow
//! message rather than a raw redb string.

use crate::support;
use crate::support_data;
use std::path::Path;
use support::{
    corrupt_primary_slot_selector, find_code_segment, last_fault, marrow, native_config,
    redb_store_path as store_path, temp_project_uncommitted, write,
};
use support_data::{native_project, seeded_project};

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

/// A seed that writes thousands of records, so the data btree spans interior pages
/// rather than living entirely in its root. Internal-page damage on such a store is
/// invisible to a probe that only opens the tables; it surfaces only when a command
/// walks the tree.
fn bulk_counter_source() -> &'static str {
    "module app\n\
     \n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     \n\
     pub fn seed()\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20for i in 1..=4000\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20c.value = i\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^counter(i) = c\n"
}

/// Seed a multi-page native store, then flip one byte of the body at `offset`. The
/// damage hides below the table roots, so opening the meta and data tables still
/// succeeds; only walking the tree reaches it. This is the corruption that read
/// commands, `run`, and `recover` formerly opened cleanly and then panicked on.
fn bulk_seeded_corrupt_store(name: &str, offset: usize) -> (support::TempProject, String) {
    let project = temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", bulk_counter_source());
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "seed a multi-page store",
    );
    let path = store_path(&project);
    let mut bytes = std::fs::read(&path).expect("read store body");
    assert!(
        bytes.len() > offset,
        "a 4000-record store should span past offset {offset}",
    );
    bytes[offset] ^= 0xff;
    std::fs::write(&path, &bytes).expect("write corrupted store body");
    (project, dir)
}

/// The dotted code a `marrow` command printed on stderr, located the same way the
/// CLI's own fault grammar reports it. The fault is the shared [`last_fault`] stderr
/// line, so any preamble cannot displace the located code.
fn stderr_code(output: &std::process::Output) -> String {
    let fault = last_fault(&output.stderr);
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

/// Damage below the table roots opens cleanly — every read-only data command,
/// `run`, and `recover` walk the tree and so must fail closed with exit 1 and a
/// typed code, never a redb traversal panic (exit 101). The data commands and
/// `recover` report `store.corruption`; `run` anchors the same corruption at its
/// source path as `run.store`, the runtime's code for a store fault. `recover` in
/// particular must not report a false success on a store it cannot make readable.
/// Run across a sweep of byte offsets, including 20484, which a probe that walks
/// only an unbounded range misses while the bounded-prefix reads still panic.
#[test]
fn interior_page_corruption_reports_corruption_on_every_traversal_command() {
    for offset in [8192usize, 12288, 16384, 20484, 24576] {
        let (_project, dir) = bulk_seeded_corrupt_store(
            &format!("cli-store-interior-page-corruption-{offset}"),
            offset,
        );

        // The data tools and `recover` report the store code directly. `run`
        // re-opens write-capable, so it reports `store.corruption` when the damage is
        // caught at open and the runtime-anchored `run.store` when it surfaces during
        // a read; both carry the same corruption fault.
        let commands: &[(&[&str], &[&str])] = &[
            (&["data", "integrity", &dir], &["store.corruption"]),
            (&["data", "dump", &dir], &["store.corruption"]),
            (&["data", "stats", &dir], &["store.corruption"]),
            (&["data", "roots", &dir], &["store.corruption"]),
            (&["data", "get", &dir, "^counter(1)"], &["store.corruption"]),
            (
                &["run", "--entry", "app::seed", &dir],
                &["run.store", "store.corruption"],
            ),
            (&["data", "recover", &dir], &["store.corruption"]),
        ];

        for (command, expected_codes) in commands {
            let output = marrow(command);
            let code = output.status.code();
            assert_ne!(
                code,
                Some(101),
                "offset {offset}: `marrow {}` must not abort on a redb traversal panic: {output:?}",
                command.join(" ")
            );
            // A flip redb tolerates leaves a readable store, so a command may still
            // succeed; the contract is that it never panics and, when it does fault,
            // reports a typed corruption code with exit 1.
            if code != Some(0) {
                assert_eq!(
                    code,
                    Some(1),
                    "offset {offset}: `marrow {}` must exit 1 on corruption: {output:?}",
                    command.join(" ")
                );
                let reported = stderr_code(&output);
                assert!(
                    expected_codes.contains(&reported.as_str()),
                    "offset {offset}: `marrow {}` must report one of {expected_codes:?}, \
                     got {reported}: {output:?}",
                    command.join(" ")
                );
            }
            let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
            assert!(
                !stderr.contains("panicked at") && !stderr.contains("RUST_BACKTRACE"),
                "offset {offset}: `marrow {}` must not leak a redb panic backtrace: {stderr}",
                command.join(" ")
            );
        }
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

    corrupt_primary_slot_selector(&project);

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
    corrupt_primary_slot_selector(&project);
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
