//! A tampered or crash-damaged native store must fail every store-opening command
//! closed with a typed `store.*` code and exit 1, never abort the process (exit
//! 101) on a redb panic. A truncated body is hard corruption; a store left needing
//! repair by an unclean shutdown is the typed recoverable status, with a Marrow
//! message rather than a raw redb string.

use crate::support;
use crate::support_data;
use std::path::Path;
use support::{
    corrupt_primary_slot_selector, find_code_segment, is_code, last_fault, marrow, marrow_bounded,
    native_config, redb_store_path as store_path, temp_project_uncommitted, write,
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

/// Seed a multi-page native store whose data btree spans interior pages, so byte
/// damage below the table roots hides until a command walks the tree.
fn seeded_bulk_store(name: &str) -> (support::TempProject, String) {
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
    (project, dir)
}

/// Seed a multi-page store, then flip one body byte at `offset`. The damage hides below
/// the table roots, so opening the meta and data tables still succeeds; only walking the
/// tree reaches it — the corruption read commands, `run`, and `recover` formerly opened
/// cleanly and then panicked on.
fn bulk_seeded_corrupt_store(name: &str, offset: usize) -> (support::TempProject, String) {
    let (project, dir) = seeded_bulk_store(name);
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
/// `run`, `backup`, and `recover` walk the tree and so must fail closed with exit 1
/// and a typed code within a bounded time, never a redb traversal panic (exit 101)
/// and never an unbounded descent loop. The data commands, `backup`, and `recover`
/// report `store.corruption`; `run` anchors the same corruption at its source path as
/// `run.store`, the runtime's code for a store fault. `recover` in particular must not
/// report a false success on a store it cannot make readable. Run across a sweep of
/// byte offsets, including 20484, which a probe that walks only an unbounded range
/// misses while the bounded-prefix reads still panic, and 33216, where a corrupt cell
/// once stalled the record descent in an infinite loop.
#[test]
fn interior_page_corruption_reports_corruption_on_every_traversal_command() {
    let deadline = std::time::Duration::from_secs(30);
    for offset in [8192usize, 12288, 16384, 20484, 24576, 33216] {
        let (project, dir) = bulk_seeded_corrupt_store(
            &format!("cli-store-interior-page-corruption-{offset}"),
            offset,
        );
        let backup_target = project.join("corrupt.mw-backup");
        let backup_target = backup_target
            .to_str()
            .expect("backup path utf8")
            .to_string();

        // The data tools, `backup`, and `recover` report the store code directly. `run`
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
            (&["backup", &dir, &backup_target], &["store.corruption"]),
            (&["data", "recover", &dir], &["store.corruption"]),
        ];

        for (command, expected_codes) in commands {
            let output = marrow_bounded(command, deadline);
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

/// Seed one multi-page store, then sweep a dense range of single-byte flips across
/// its body. Every store-opening command — the read-only inspections, the
/// write-capable `run`, `backup`, and `recover` — must finish bounded and fail
/// closed: a tolerable flip reads through (exit 0), and any other outcome is a typed
/// fault with exit 1, never a redb panic (exit 101) on traversal, on the write
/// transaction, or on the database close that flushes redb's allocator at drop, and
/// never an unbounded loop. The sweep spans the offsets where each of those escapes
/// was first seen: a write-transaction panic, a drop-time allocator panic, and a
/// delete-path descent that once spun forever.
#[test]
fn swept_interior_corruption_keeps_every_command_bounded_and_fail_closed() {
    let (project, dir) = seeded_bulk_store("cli-store-swept-interior-corruption");
    let store = store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");
    let backup_target = project.join("swept.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    let deadline = std::time::Duration::from_secs(30);

    // `backup` and `recover` are pure store operations: an untraversable store must
    // anchor at `store.corruption`, never a panic. The other commands execute over the
    // store and so may instead surface a runtime or data-level fault when a flip lands in
    // a readable-but-altered value; for them a typed non-store fault is still fail-closed.
    let store_fault_commands: &[(&[&str], &[&str])] = &[
        (&["backup", &dir, &backup_target], &["store.corruption"]),
        (&["data", "recover", &dir], &["store.corruption"]),
    ];
    let executing_commands: &[&[&str]] = &[
        &["run", "--entry", "app::seed", &dir],
        &["data", "integrity", &dir],
        &["data", "dump", &dir],
        &["data", "stats", &dir],
    ];

    for offset in (8192..clean.len().min(49152)).step_by(256) {
        for (command, expected_codes) in store_fault_commands {
            let mut corrupt = clean.clone();
            corrupt[offset] ^= 0xff;
            std::fs::write(&store, &corrupt).expect("write corrupted store body");
            let output = marrow_bounded(command, deadline);
            assert_no_panic_and_bounded(&output, command, offset);
            assert_expected_fault_code(&output, command, expected_codes, offset);
        }
        for command in executing_commands {
            let mut corrupt = clean.clone();
            corrupt[offset] ^= 0xff;
            std::fs::write(&store, &corrupt).expect("write corrupted store body");
            let output = marrow_bounded(command, deadline);
            assert_no_panic_and_bounded(&output, command, offset);
            assert_typed_fault_code(&output, command, offset);
        }
    }
}

/// A store-opening command over a corrupted body must finish bounded (enforced by
/// `marrow_bounded`), never abort on a redb panic, and never leak a panic backtrace.
fn assert_no_panic_and_bounded(output: &std::process::Output, command: &[&str], offset: usize) {
    assert_ne!(
        output.status.code(),
        Some(101),
        "offset {offset}: `marrow {}` must not abort on a redb panic: {output:?}",
        command.join(" ")
    );
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("RUST_BACKTRACE"),
        "offset {offset}: `marrow {}` must not leak a redb panic backtrace: {stderr}",
        command.join(" ")
    );
}

/// A command that anchors an untraversable store: it either reads through (exit 0) or
/// reports one of `expected_codes` with exit 1.
fn assert_expected_fault_code(
    output: &std::process::Output,
    command: &[&str],
    expected_codes: &[&str],
    offset: usize,
) {
    if output.status.code() == Some(0) {
        return;
    }
    assert_eq!(
        output.status.code(),
        Some(1),
        "offset {offset}: `marrow {}` must exit 1 on corruption: {output:?}",
        command.join(" ")
    );
    let reported = stderr_code(output);
    assert!(
        expected_codes.contains(&reported.as_str()),
        "offset {offset}: `marrow {}` must report one of {expected_codes:?}, got {reported}: \
         {output:?}",
        command.join(" ")
    );
}

/// A command that executes over a corrupted body: it either succeeds (exit 0) or exits 1
/// carrying a typed dotted code somewhere on stderr — `store.corruption` for an
/// untraversable store, or a runtime/data finding (`run.*`, `write.*`, `data.*`) when the
/// flip lands in a readable-but-altered value (integrity may then print several problems
/// and a trailing help line). The point is that a non-zero exit is always a typed fault,
/// never an untyped crash.
fn assert_typed_fault_code(output: &std::process::Output, command: &[&str], offset: usize) {
    if output.status.code() == Some(0) {
        return;
    }
    assert_eq!(
        output.status.code(),
        Some(1),
        "offset {offset}: `marrow {}` must exit 1 on a fault: {output:?}",
        command.join(" ")
    );
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
    let has_typed_code = stderr
        .lines()
        .flat_map(|line| line.split([' ', ':']))
        .any(is_code);
    assert!(
        has_typed_code,
        "offset {offset}: `marrow {}` must report a typed code, not an untyped crash: {stderr}",
        command.join(" ")
    );
}

/// `data recover` must be convergent: a successful recover means the next read
/// succeeds. For every corrupted store, recover either makes the store cleanly
/// readable — the following `data integrity` exits 0 — or reports `store.corruption`
/// with exit 1; it must never print a false success (exit 0) over a store it cannot
/// make readable, the shape where recover blessed a store whose next read faulted and
/// re-running recover reported success again yet never converged. The sweep includes
/// the slot- and allocator-region offsets where that false success was first seen
/// (8704, 18432, 19712, 28672) alongside a dense range across a multi-thousand-record
/// store. Every command is bounded so a regression cannot hang the suite.
#[test]
fn data_recover_is_convergent_or_reports_corruption() {
    let (project, dir) = seeded_bulk_store("cli-store-recover-convergence");
    let store = store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");
    let deadline = std::time::Duration::from_secs(30);

    let slot_region_offsets = [8704usize, 18432, 19712, 28672];
    let swept = (8192..clean.len().min(49152)).step_by(512);
    for offset in slot_region_offsets.into_iter().chain(swept) {
        let mut corrupt = clean.clone();
        corrupt[offset] ^= 0xff;
        std::fs::write(&store, &corrupt).expect("write corrupted store body");

        let recover = marrow_bounded(&["data", "recover", &dir], deadline);
        assert_no_panic_and_bounded(&recover, &["data", "recover", &dir], offset);

        let recover_code = recover.status.code();
        assert!(
            recover_code == Some(0) || recover_code == Some(1),
            "offset {offset}: recover must exit 0 or 1, got {recover_code:?}: {recover:?}"
        );

        // The convergence oracle: re-open through the read path the way the next
        // command does. A recover that claimed success must leave the store readable;
        // a recover that reported corruption may leave it unreadable.
        let integrity = marrow_bounded(&["data", "integrity", &dir], deadline);
        assert_no_panic_and_bounded(&integrity, &["data", "integrity", &dir], offset);

        if recover_code == Some(0) {
            assert_eq!(
                integrity.status.code(),
                Some(0),
                "offset {offset}: recover reported success but the store is still \
                 unreadable (a false repair): recover={recover:?} integrity={integrity:?}"
            );
        } else {
            assert_eq!(
                stderr_code(&recover),
                "store.corruption",
                "offset {offset}: a recover that cannot make the store readable must \
                 report store.corruption: {recover:?}"
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
