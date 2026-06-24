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

/// A seed that writes a fixed `records` count under one root, each record carrying two
/// declared field cells. A body flip that silently drops or rewrites a single field cell
/// inside a surviving record leaves the record count unchanged, so the shortfall shows
/// only against the seeded cell total — the completeness the structural digest anchors.
fn fixed_count_source(records: u32) -> String {
    format!(
        "module app\n\
         \n\
         resource Note\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20required body: string\n\
         store ^notes(id: int): Note\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20for i in 1..={records}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a note title\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.body = \"the body text of this note, long enough to span a cell\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^notes(i) = n\n"
    )
}

/// A seed plus single-record mutation entries over a two-field record: `seed` writes
/// `records` records, `delete_one` removes one record through the production resource-delete
/// path, and `overwrite_one` rewrites an existing record without changing the record count.
/// Each mutation commits a fresh transaction, so the durable per-root structural digest must
/// be restamped by a normal delete or overwrite, not just a presence write, or a healthy
/// store false-corrupts.
fn mutable_count_source(records: u32) -> String {
    format!(
        "module app\n\
         \n\
         resource Note\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20required body: string\n\
         store ^notes(id: int): Note\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20for i in 1..={records}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a note title\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.body = \"the body text of this note, long enough to span a cell\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^notes(i) = n\n\
         \n\
         pub fn delete_one()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20delete ^notes(1)\n\
         \n\
         pub fn overwrite_one()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n.title = \"rewritten title\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n.body = \"rewritten body text replacing the original\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^notes(2) = n\n"
    )
}

/// Seed a store with exactly `records` committed records under `^notes`, each with two
/// declared field cells.
fn seeded_fixed_store(name: &str, records: u32) -> (support::TempProject, String) {
    let project = temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", &fixed_count_source(records));
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "seed a fixed-count store",
    );
    (project, dir)
}

/// The record count `data stats` reports, parsed from its `records: N` line, or `None`
/// when the command did not exit 0 with a record line.
fn reported_record_count(output: &std::process::Output) -> Option<u64> {
    reported_count(output, "records:")
}

/// The field-cell count `data stats` reports, parsed from its `cells: N` line, or `None`
/// when the command did not exit 0 with a cell line. A single dropped or torn field cell
/// inside a surviving record changes this without changing the record count.
fn reported_cell_count(output: &std::process::Output) -> Option<u64> {
    reported_count(output, "cells:")
}

fn reported_count(output: &std::process::Output, label: &str) -> Option<u64> {
    if output.status.code() != Some(0) {
        return None;
    }
    let stdout = String::from_utf8(output.stdout.clone()).ok()?;
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .and_then(|rest| rest.trim().parse::<u64>().ok())
}

/// A single-byte data-btree flip can leave a store redb still opens and walks without a
/// panic, yet enumerate fewer cells than were committed, or read a torn-but-decodable
/// value: redb's range scan reads straight past the damaged page. The data family has no
/// structural mismatch on such a flip — the enumeration and any expectation derived from it
/// shift together — and a record count alone cannot see a single field cell dropped or
/// rewritten inside a surviving record. The independent oracle is the durable per-root
/// structural digest, which covers every committed cell's identity and value. The whole
/// contract across a full-file sweep: `data integrity` must never bless a store holding
/// fewer than the committed cells, and whenever it reports `store.corruption`, `backup` and
/// `recover` must agree rather than archive or bless the truncated store.
#[test]
fn silent_cell_loss_is_caught_by_the_structural_digest() {
    const SEEDED: u64 = 200;
    const EXPECTED_CELLS: u64 = SEEDED * 2;
    let (project, dir) = seeded_fixed_store("cli-store-silent-truncation", SEEDED as u32);
    let store = store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");
    let backup_target = project.join("truncation.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    let deadline = std::time::Duration::from_secs(30);

    for offset in (8192..clean.len()).step_by(256) {
        let mut corrupt = clean.clone();
        corrupt[offset] ^= 0xff;
        std::fs::write(&store, &corrupt).expect("write corrupted store body");

        let integrity = marrow_bounded(&["data", "integrity", &dir], deadline);
        assert_no_panic_and_bounded(&integrity, &["data", "integrity", &dir], offset);

        // A store integrity blesses as verified must still hold every committed cell; a
        // flip that silently dropped a record, or a single field cell inside a surviving
        // record, and still verified would be the defect the digest closes.
        if integrity.status.code() == Some(0) {
            let stats = marrow_bounded(&["data", "stats", &dir], deadline);
            assert_no_panic_and_bounded(&stats, &["data", "stats", &dir], offset);
            assert_eq!(
                reported_record_count(&stats),
                Some(SEEDED),
                "offset {offset}: `data integrity` verified the store yet it no longer holds \
                 all {SEEDED} committed records (a silently blessed truncation): {stats:?}"
            );
            assert_eq!(
                reported_cell_count(&stats),
                Some(EXPECTED_CELLS),
                "offset {offset}: `data integrity` verified the store yet it no longer holds \
                 all {EXPECTED_CELLS} committed cells (a silently blessed cell drop): {stats:?}"
            );
            continue;
        }

        // A flip that lands in a readable-but-altered value surfaces a data-level finding
        // rather than a store fault; only a store integrity calls corrupt binds the
        // archiving and repair paths, which must agree rather than bless it.
        if stderr_code(&integrity) != "store.corruption" {
            continue;
        }
        for command in [
            ["backup", &dir, &backup_target].as_slice(),
            ["data", "recover", &dir].as_slice(),
        ] {
            let output = marrow_bounded(command, deadline);
            assert_no_panic_and_bounded(&output, command, offset);
            assert_eq!(
                output.status.code(),
                Some(1),
                "offset {offset}: `marrow {}` blessed a store integrity reports corrupt: \
                 {output:?}",
                command.join(" "),
            );
            assert_eq!(
                stderr_code(&output),
                "store.corruption",
                "offset {offset}: `marrow {}` must report store.corruption on a truncated \
                 store: {output:?}",
                command.join(" "),
            );
        }
    }
}

/// Seed a store of `records` records whose source also carries the single-record delete and
/// overwrite entries, returning the project and its directory.
fn seeded_mutable_store(name: &str, records: u32) -> (support::TempProject, String) {
    let project = temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", &mutable_count_source(records));
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "seed a mutable-count store",
    );
    (project, dir)
}

/// A baseline declaring `^notes` over a one-field `Note` with a `seed` that writes three records.
/// Pairs with [`EVOLVABLE_DEFAULT_SOURCE`], which adds a defaulted `pages` field plus the
/// `default Note.pages = 0` evolution intent so `evolve apply` has a non-empty activation to run.
const EVOLVABLE_BASELINE_SOURCE: &str = "module app\n\
     \n\
     resource Note\n\
     \x20\x20\x20\x20required title: string\n\
     store ^notes(id: int): Note\n\
     \n\
     pub fn seed()\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20for i in 1..=3\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a note title\"\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^notes(i) = n\n";

/// The evolved counterpart of [`EVOLVABLE_BASELINE_SOURCE`]: a defaulted `pages` field and the
/// `default Note.pages = 0` intent the apply activates over the committed `^notes` records.
const EVOLVABLE_DEFAULT_SOURCE: &str = "module app\n\
     \n\
     resource Note\n\
     \x20\x20\x20\x20required title: string\n\
     \x20\x20\x20\x20required pages: int\n\
     store ^notes(id: int): Note\n\
     \n\
     evolve\n\
     \x20\x20\x20\x20default Note.pages = 0\n\
     \n\
     pub fn seed()\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20for i in 1..=3\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a note title\"\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.pages = 1\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^notes(i) = n\n";

/// Seed an evolvable store: commit [`EVOLVABLE_BASELINE_SOURCE`] (three `^notes` records and a lock
/// recording the root), returning the project so the caller can swap in [`EVOLVABLE_DEFAULT_SOURCE`]
/// before driving `evolve apply`.
fn seeded_evolvable_store(name: &str) -> support::TempProject {
    let project = temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", EVOLVABLE_BASELINE_SOURCE);
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "seed an evolvable baseline store",
    );
    project
}

/// A normal single-record delete and a single-record overwrite must keep the durable
/// per-root structural digest exact: the delete drops the whole record through the
/// production resource-delete path (subtracting each dropped cell), and the overwrite
/// rewrites an existing record's cells without changing the record count (subtracting the
/// prior values and adding the new). Both restamp the digest in their own commit, so a
/// healthy store stays healthy: `data integrity`, `data stats`, `backup`, and `data recover`
/// all pass and report the true count, never the false `store.corruption` a stale digest
/// would raise.
#[test]
fn a_normal_delete_and_overwrite_keep_the_structural_digest_exact() {
    const SEEDED: u64 = 50;
    let (project, dir) = seeded_mutable_store("cli-store-delete-keeps-anchor", SEEDED as u32);
    let backup_target = project.join("delete.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();

    // Delete one record. The digest must drop that record's cells in the same commit, so
    // the count falls to N-1 and integrity stays clean.
    assert_eq!(
        marrow(&["run", "--entry", "app::delete_one", &dir])
            .status
            .code(),
        Some(0),
        "a single-record delete must commit cleanly",
    );
    assert_healthy_store(&dir, &backup_target, SEEDED - 1);

    // Overwrite an existing record's values. The count is unchanged, and the digest must
    // track the new bytes so the store stays healthy rather than false-corrupting.
    assert_eq!(
        marrow(&["run", "--entry", "app::overwrite_one", &dir])
            .status
            .code(),
        Some(0),
        "overwriting an existing record must commit cleanly",
    );
    assert_healthy_store(&dir, &backup_target, SEEDED - 1);
}

/// Every read-and-verify command passes on a healthy store and reports `expected_records`,
/// proving the structural digest and the lock-root witness agree with the live data.
fn assert_healthy_store(dir: &str, backup_target: &str, expected_records: u64) {
    let integrity = marrow(&["data", "integrity", dir]);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "`data integrity` must verify a healthy store: {integrity:?}",
    );
    let stats = marrow(&["data", "stats", dir]);
    assert_eq!(
        reported_record_count(&stats),
        Some(expected_records),
        "`data stats` must report the true post-mutation record count: {stats:?}",
    );
    let backup = marrow(&["backup", dir, backup_target]);
    assert_eq!(
        backup.status.code(),
        Some(0),
        "`backup` must archive a healthy store: {backup:?}",
    );
    let recover = marrow(&["data", "recover", dir]);
    assert_eq!(
        recover.status.code(),
        Some(0),
        "`data recover` must bless a healthy store: {recover:?}",
    );
}

/// Replace the seeded store file with a fresh empty store, modelling a commit-metadata flip
/// that rolls redb back to its empty initial commit: the file opens and walks cleanly, yet
/// presents zero records and zero digests. The committed `marrow.lock` still records the
/// roots the store dropped.
fn roll_store_back_to_empty(project: &Path) {
    let store = store_path(project);
    let scratch = project.join(".data").join("empty-scratch.redb");
    {
        let _empty = marrow_store::tree::TreeStore::open(&scratch).expect("create empty store");
    }
    std::fs::copy(&scratch, &store).expect("overwrite store with empty");
    std::fs::remove_file(&scratch).expect("remove scratch store");
}

/// The per-root structural digest cannot witness a corruption that drops the anchor itself:
/// a rollback to the empty initial commit presents zero records and zero digests, so the
/// digest pass visits nothing and would pass vacuously. The independent witness is the
/// committed `marrow.lock`, which records the roots the store committed. A store that
/// presents fewer roots than the lock recorded has lost durable identity, so `data
/// integrity`, `data stats`, `backup`, and `data recover` must all fail closed with
/// `store.corruption` rather than bless or archive the empty store.
#[test]
fn a_total_drop_to_empty_is_caught_by_the_lock_root_witness() {
    let (project, dir) = seeded_mutable_store("cli-store-total-drop", 200);
    let backup_target = project.join("dropped.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    let deadline = std::time::Duration::from_secs(30);

    roll_store_back_to_empty(&project);

    for command in [
        ["data", "integrity", &dir].as_slice(),
        ["data", "stats", &dir].as_slice(),
        ["backup", &dir, &backup_target].as_slice(),
        ["data", "recover", &dir].as_slice(),
    ] {
        let output = marrow_bounded(command, deadline);
        assert_no_panic_and_bounded(&output, command, 0);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`marrow {}` blessed a store rolled back below its committed roots: {output:?}",
            command.join(" "),
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`marrow {}` must report store.corruption on a store that lost committed roots: \
             {output:?}",
            command.join(" "),
        );
    }
}

/// Replace the seeded store file with a freshly minted store that carries only its store uid and
/// no committed catalog baseline, leaving NO writer live. This is the durable shape a crash, kill,
/// or power loss settles between the production write path's two creation transactions — the uid
/// stamped in its own commit, then the process lost before the first root and lock baseline commit.
/// The committed `marrow.lock` still records the root, so the store has genuinely lost a committed
/// root with no writer re-creating it: a real data loss, not a live race.
fn replace_with_baseline_pending_store(project: &Path) {
    let store = store_path(project);
    std::fs::remove_file(&store).expect("remove seeded store");
    let pending = marrow_store::tree::TreeStore::open(&store).expect("create fresh store");
    pending
        .write_store_uid(&marrow_store::tree::StoreUid::from_entropy_bytes([7u8; 16]))
        .expect("stamp store uid");
}

/// A settled uid-only store with no live writer, under a committed lock that records a root, is a
/// crash between the two creation transactions: the store lost a committed root and nothing is
/// re-creating it. The lock-root witness must catch that loss — never bless it. A live writer would
/// hold the redb write lock across the whole creation, excluding any open as `store.locked`, so a
/// store the inspection can open is provably writer-free and fails closed on every driver.
#[test]
fn a_settled_baseline_pending_store_under_a_committed_lock_fails_closed() {
    let (project, dir) = seeded_mutable_store("cli-store-baseline-pending", 64);
    let backup_target = project.join("baseline-pending.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    let deadline = std::time::Duration::from_secs(30);

    replace_with_baseline_pending_store(&project);

    for command in [
        ["data", "integrity", &dir].as_slice(),
        ["data", "stats", &dir].as_slice(),
        ["data", "get", &dir, "^notes"].as_slice(),
        ["backup", &dir, &backup_target].as_slice(),
    ] {
        let output = marrow_bounded(command, deadline);
        assert_no_panic_and_bounded(&output, command, 0);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`marrow {}` blessed a settled crash-mid-creation store instead of failing closed: \
             {output:?}",
            command.join(" "),
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`marrow {}` must report the lost committed root as store.corruption: {output:?}",
            command.join(" "),
        );
    }

    // Doctor opens the same settled store and runs the lock-root witness; its JSON envelope must
    // carry the store.corruption finding for the lost committed root.
    let doctor = marrow_bounded(&["doctor", &dir, "--format", "json"], deadline);
    assert_no_panic_and_bounded(&doctor, &["doctor"], 0);
    let report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).expect("doctor json report");
    assert!(
        findings_carry_lock_root_corruption(&report),
        "doctor blessed a settled crash-mid-creation store instead of flagging its lost root: \
         {report}"
    );
}

/// `data recover` opens the store write-capable and runs the same lock-root cross-check. A settled
/// uid-only store with no live writer under a committed lock has lost a committed root, so recover
/// must fail it closed as `store.corruption`, never bless it as a clean recovery.
#[test]
fn data_recover_fails_a_settled_baseline_pending_store_closed() {
    let (project, dir) = seeded_mutable_store("cli-store-recover-baseline-pending", 64);
    let deadline = std::time::Duration::from_secs(30);

    replace_with_baseline_pending_store(&project);

    let recover = marrow_bounded(&["data", "recover", &dir], deadline);
    assert_no_panic_and_bounded(&recover, &["data", "recover", &dir], 0);
    assert_eq!(
        recover.status.code(),
        Some(1),
        "recover blessed a settled crash-mid-creation store instead of failing closed: {recover:?}"
    );
    assert_eq!(
        stderr_code(&recover),
        "store.corruption",
        "recover must report the lost committed root as store.corruption: {recover:?}"
    );
}

/// Whether a doctor JSON report carries a lock-root-loss `store.corruption` finding — the
/// verdict a settled crash-mid-creation store must trigger and a live race must not.
fn findings_carry_lock_root_corruption(report: &serde_json::Value) -> bool {
    report["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .any(|finding| finding["data"]["underlying_code"] == "store.corruption")
    })
}

/// Hammer `marrow doctor --format json` against a live writer that repeatedly commits write
/// transactions against the healthy store, holding the redb write flock across each commit window.
/// Doctor opens the store and runs the lock-root witness independently of the read-only
/// inspections, so it must share their live-writer tolerance: across the whole race it may report
/// the store locked, a transient open failure carrying a non-corruption code, or a clean healthy
/// report, but never the lock-root `store.corruption` over a store the writer provably leaves
/// healthy. The writer never removes the committed store: deleting a store under a roots-recording
/// lock is durable loss, which the witness must fail closed, so a legitimate race is a live writer
/// mutating the store in place, not one re-creating it from absence.
#[test]
fn doctor_under_a_live_writer_never_false_corrupts_a_healthy_store() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (_project, dir) = seeded_mutable_store("cli-store-doctor-live-writer", 64);
    let deadline = std::time::Duration::from_secs(30);

    // A writer that, in a loop, overwrites an existing record through the production run path. Each
    // commit holds the redb write flock, so a racing doctor either wins the open or is excluded as
    // `store.locked`; the store always presents every committed root, so the witness never fires.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let dir = dir.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let run = marrow(&["run", "--entry", "app::overwrite_one", &dir]);
                assert!(
                    matches!(run.status.code(), Some(0) | Some(1)),
                    "the background writer must commit cleanly or yield to a racing lock: {run:?}",
                );
            }
        })
    };

    for _ in 0..360 {
        let doctor = marrow_bounded(&["doctor", &dir, "--format", "json"], deadline);
        assert_no_panic_and_bounded(&doctor, &["doctor"], 0);
        let report: serde_json::Value = match serde_json::from_slice(&doctor.stdout) {
            Ok(report) => report,
            // A doctor that fell to the writer's lock may have emitted nothing parseable; the
            // contract bites on the JSON envelope it does produce.
            Err(_) => continue,
        };
        assert!(
            !findings_carry_lock_root_corruption(&report),
            "doctor false-corrupted a healthy store the writer is mid-re-creating: {report}"
        );
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().expect("join the background writer");

    // The writer left the store committed and healthy: a settled integrity pass must verify it,
    // proving the live-writer tolerance never blessed a genuine loss.
    let integrity = marrow(&["data", "integrity", &dir]);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "the store the race left behind must verify clean: {integrity:?}",
    );
}

/// Hammer `marrow data recover --format json` against a live writer committing write transactions
/// against the healthy store in place. Recover opens the store write-capable and runs the lock-root
/// witness, so it must share the live-writer tolerance: it can win the open and bless the in-flight
/// store, yield to the writer's lock, or hit a transient non-lock-root open failure, but it must
/// never report `store.corruption` over a store the writer provably leaves healthy.
#[test]
fn data_recover_under_a_live_writer_never_false_corrupts_a_healthy_store() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (_project, dir) = seeded_mutable_store("cli-store-recover-live-writer", 64);
    let deadline = std::time::Duration::from_secs(30);

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let dir = dir.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let run = marrow(&["run", "--entry", "app::overwrite_one", &dir]);
                assert!(
                    matches!(run.status.code(), Some(0) | Some(1)),
                    "the background writer must commit cleanly or yield to a racing lock: {run:?}",
                );
            }
        })
    };

    for _ in 0..200 {
        let recover = marrow_bounded(&["data", "recover", "--format", "json", &dir], deadline);
        assert_no_panic_and_bounded(&recover, &["data", "recover"], 0);
        // `--format json` writes its single result or fault object to stdout; the contract bites
        // on the `code` it carries, which must never be the lock-root `store.corruption` over a
        // store the writer leaves healthy. A recover that fell to the writer's lock may emit
        // nothing parseable.
        let Ok(report) = serde_json::from_slice::<serde_json::Value>(&recover.stdout) else {
            continue;
        };
        assert_ne!(
            report["code"],
            serde_json::json!("store.corruption"),
            "recover false-corrupted a healthy store the writer is mid-re-creating: {recover:?}"
        );
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().expect("join the background writer");

    // The writer left the store committed and healthy: a settled integrity pass must verify it,
    // proving the live-writer tolerance never blessed a genuine loss.
    let integrity = marrow(&["data", "integrity", &dir]);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "the store the race left behind must verify clean: {integrity:?}",
    );
}

/// Delete the seeded store file outright while leaving the committed `marrow.lock`, modelling
/// a store the developer or a stray `rm` removed. The store path resolves absent, so the
/// read-only inspections, doctor, and recover all short-circuit as a first run unless they
/// consult the lock — which still records the dropped roots.
fn delete_store_file(project: &Path) {
    std::fs::remove_file(store_path(project)).expect("delete store file");
}

/// A store deleted from disk while its committed `marrow.lock` still records roots is the same
/// loss as a rollback to empty carried to the limit: the durable file vanished but its
/// committed identity remains. The path resolves absent, so without the lock witness every
/// read-only inspection, `doctor`, and `recover` would bless it as a clean first run while
/// `backup` fails it closed. All of them must agree with `backup` and report
/// `store.corruption` rather than silently mask the loss.
#[test]
fn an_absent_store_with_a_committed_lock_is_caught_by_the_lock_root_witness() {
    let (project, dir) = seeded_mutable_store("cli-store-absent-lock", 64);
    let backup_target = project.join("absent.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    let deadline = std::time::Duration::from_secs(30);

    delete_store_file(&project);

    for command in [
        ["data", "integrity", &dir].as_slice(),
        ["data", "stats", &dir].as_slice(),
        ["data", "roots", &dir].as_slice(),
        ["data", "dump", &dir].as_slice(),
        ["data", "get", &dir, "^notes"].as_slice(),
        ["data", "get", &dir, "^notes(1)"].as_slice(),
        ["backup", &dir, &backup_target].as_slice(),
        ["data", "recover", &dir].as_slice(),
    ] {
        let output = marrow_bounded(command, deadline);
        assert_no_panic_and_bounded(&output, command, 0);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`marrow {}` blessed an absent store while the lock records committed roots: \
             {output:?}",
            command.join(" "),
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`marrow {}` must report store.corruption on a deleted store with a committed lock: \
             {output:?}",
            command.join(" "),
        );
    }

    let doctor = marrow(&["doctor", &dir, "--format", "json"]);
    assert_eq!(
        doctor.status.code(),
        Some(1),
        "doctor blessed an absent store while the lock records committed roots: {doctor:?}",
    );
    let report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).expect("doctor json report");
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|finding| finding["data"]["underlying_code"] == "store.corruption"),
        "doctor must report the deleted store with a committed lock as store.corruption: {report}"
    );
}

/// A present store rolled back to its empty initial commit while its committed `marrow.lock` still
/// records roots has lost durable identity. A write-capable `run` must fail closed with
/// `store.corruption` rather than re-establish a fresh baseline over the rolled-back store and
/// bless the loss — the same verdict the read-only inspection family reaches over this store, now
/// shared by the writer before it would mask the rollback.
#[test]
fn run_over_a_rolled_back_store_with_a_committed_lock_fails_closed() {
    let (project, dir) = seeded_mutable_store("cli-run-rolled-back-lock", 3);
    let deadline = std::time::Duration::from_secs(30);

    roll_store_back_to_empty(&project);
    assert!(
        project.join("marrow.lock").exists(),
        "the committed lock must survive the rollback",
    );

    let run = marrow_bounded(&["run", "--entry", "app::seed", &dir], deadline);
    assert_no_panic_and_bounded(&run, &["run", "--entry", "app::seed", &dir], 0);
    assert_eq!(
        run.status.code(),
        Some(1),
        "`run` re-baselined a rolled-back store under a committed lock instead of failing closed: \
         {run:?}",
    );
    assert_eq!(
        stderr_code(&run),
        "store.corruption",
        "`run` over a store rolled back below its committed roots must report store.corruption: \
         {run:?}",
    );
}

/// A uid-only store crashed between the two creation transactions, under a committed lock that
/// records a root, has lost the committed root with no writer re-creating it. A write-capable
/// `run` must fail it closed rather than re-baseline over the settled loss, the same verdict
/// `data recover` reaches on this store.
#[test]
fn run_over_a_settled_baseline_pending_store_with_a_committed_lock_fails_closed() {
    let (project, dir) = seeded_mutable_store("cli-run-baseline-pending-lock", 3);
    let deadline = std::time::Duration::from_secs(30);

    replace_with_baseline_pending_store(&project);

    let run = marrow_bounded(&["run", "--entry", "app::seed", &dir], deadline);
    assert_no_panic_and_bounded(&run, &["run", "--entry", "app::seed", &dir], 0);
    assert_eq!(
        run.status.code(),
        Some(1),
        "`run` re-baselined a settled crash-mid-creation store instead of failing closed: {run:?}",
    );
    assert_eq!(
        stderr_code(&run),
        "store.corruption",
        "`run` over a settled baseline-pending store must report the lost root as store.corruption: \
         {run:?}",
    );
}

/// A healthy committed store keeps `run` working: the lock-root guard passes a present store that
/// still presents every committed root, so a follow-up entry commits cleanly and the data survives.
#[test]
fn run_over_a_healthy_store_still_writes() {
    let (_project, dir) = seeded_mutable_store("cli-run-healthy", 5);

    assert_eq!(
        marrow(&["run", "--entry", "app::delete_one", &dir])
            .status
            .code(),
        Some(0),
        "a follow-up run over a healthy store must commit cleanly",
    );
    let stats = marrow(&["data", "stats", &dir]);
    assert_eq!(
        reported_record_count(&stats),
        Some(4),
        "the healthy store must reflect the committed mutation: {stats:?}",
    );
}

/// The store deleted from disk while its committed `marrow.lock` still records roots — the plain
/// `rm` of the redb body — is the unambiguous data-loss repro. A write-capable `run` must fail
/// closed with `store.corruption` rather than silently re-create an empty store and re-seed over
/// the loss, matching the read-only inspection family on the same store. Before the fix `run`
/// returned a fresh rc0 here, permanently discarding the committed records.
#[test]
fn run_over_a_deleted_store_with_a_committed_lock_fails_closed() {
    let (project, dir) = seeded_mutable_store("cli-run-deleted-lock", 3);
    let deadline = std::time::Duration::from_secs(30);

    delete_store_file(&project);
    assert!(
        project.join("marrow.lock").exists(),
        "the committed lock must survive the store deletion",
    );

    let run = marrow_bounded(&["run", "--entry", "app::seed", &dir], deadline);
    assert_no_panic_and_bounded(&run, &["run", "--entry", "app::seed", &dir], 0);
    assert_eq!(
        run.status.code(),
        Some(1),
        "`run` silently re-created a deleted store under a committed lock instead of failing \
         closed: {run:?}",
    );
    assert_eq!(
        stderr_code(&run),
        "store.corruption",
        "`run` over a store deleted while its lock records committed roots must report \
         store.corruption: {run:?}",
    );
}

/// `evolve apply` creates or baselines the store before applying, so it carries the same write-path
/// hole as `run`: a store deleted while its committed `marrow.lock` records roots must fail closed
/// with `store.corruption` rather than re-baseline a fresh empty store and re-project the lock over
/// the loss. Before the fix apply reported "Nothing to apply" rc0 and rewrote the lock to the empty
/// store, permanently discarding the committed records.
#[test]
fn evolve_apply_over_a_deleted_store_with_a_committed_lock_fails_closed() {
    let project = seeded_evolvable_store("cli-evolve-apply-deleted-lock");
    let dir = project.to_str().expect("dir utf8").to_string();
    let deadline = std::time::Duration::from_secs(30);

    write(&project, "src/app.mw", EVOLVABLE_DEFAULT_SOURCE);
    delete_store_file(&project);
    assert!(
        project.join("marrow.lock").exists(),
        "the committed lock must survive the store deletion",
    );

    let apply = marrow_bounded(&["evolve", "apply", &dir], deadline);
    assert_no_panic_and_bounded(&apply, &["evolve", "apply", &dir], 0);
    assert_eq!(
        apply.status.code(),
        Some(1),
        "`evolve apply` re-baselined a deleted store under a committed lock instead of failing \
         closed: {apply:?}",
    );
    assert_eq!(
        stderr_code(&apply),
        "store.corruption",
        "`evolve apply` over a store deleted while its lock records committed roots must report \
         store.corruption: {apply:?}",
    );
}

/// A genuine first run with no committed lock and no store creates the store and commits its data:
/// the lock-root witness records no active root to contradict, so the write path proceeds. This is
/// the missing-lock carve-out the corruption verdict must never swallow, proven through the live
/// write path rather than only the read-only inspections.
#[test]
fn a_true_first_run_creates_the_store_and_commits() {
    let (_project, dir) = seeded_mutable_store("cli-run-true-first", 4);
    let stats = marrow(&["data", "stats", &dir]);
    assert_eq!(
        reported_record_count(&stats),
        Some(4),
        "a true first run must create the store and commit its records: {stats:?}",
    );
}

/// A genuine first `evolve apply` with no committed lock and no store baselines the store and
/// applies cleanly: the lock-root witness records no active root to contradict, so the fresh
/// baseline path runs rather than failing closed. A follow-up run then commits against the
/// baselined store, proving the first apply established a working store rather than a phantom.
#[test]
fn a_true_first_evolve_apply_baselines_the_store() {
    let project = temp_project_uncommitted("cli-evolve-apply-true-first", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", EVOLVABLE_DEFAULT_SOURCE);
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert!(
        !project.join("marrow.lock").exists(),
        "an un-applied project has no committed lock",
    );

    let apply = marrow(&["evolve", "apply", &dir]);
    assert_eq!(
        apply.status.code(),
        Some(0),
        "a true first evolve apply must baseline the store and apply cleanly: {apply:?}",
    );
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "a run after the first apply must commit against the baselined store",
    );
}

/// A healthy committed store keeps `evolve apply` working: the lock-root guard passes a present
/// store that still presents every committed root, so an apply that adds a defaulted field
/// activates cleanly rather than false-corrupting.
#[test]
fn evolve_apply_over_a_healthy_store_still_applies() {
    let project = seeded_evolvable_store("cli-evolve-apply-healthy");
    let dir = project.to_str().expect("dir utf8").to_string();

    write(&project, "src/app.mw", EVOLVABLE_DEFAULT_SOURCE);
    let apply = marrow(&["evolve", "apply", &dir]);
    assert_eq!(
        apply.status.code(),
        Some(0),
        "evolve apply over a healthy store must activate the defaulted field cleanly: {apply:?}",
    );
    let stats = marrow(&["data", "stats", &dir]);
    assert_eq!(
        reported_record_count(&stats),
        Some(3),
        "the healthy store must keep its committed records after the apply: {stats:?}",
    );
}

/// A genuine first run with neither a committed lock nor a store has no recorded baseline to
/// contradict, so the lock-root witness must not fire: every read-only inspection, `doctor`,
/// and `recover` stay a clean rc0 first run. This is the separate missing-lock carve-out the
/// corruption verdict must never swallow.
#[test]
fn a_true_first_run_with_no_lock_and_no_store_stays_clean() {
    let project = native_project("cli-store-first-run");
    let dir = project.to_str().expect("dir utf8").to_string();
    assert!(
        !project.join("marrow.lock").exists(),
        "an unrun project has no committed lock",
    );
    assert!(
        !store_path(&project).exists(),
        "an unrun project has no store file",
    );

    for command in [
        ["data", "integrity", &dir].as_slice(),
        ["data", "stats", &dir].as_slice(),
        ["data", "roots", &dir].as_slice(),
        ["data", "dump", &dir].as_slice(),
        ["data", "get", &dir, "^counter"].as_slice(),
        ["data", "recover", &dir].as_slice(),
    ] {
        let output = marrow(command);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`marrow {}` must stay a clean first run with no lock and no store: {output:?}",
            command.join(" "),
        );
    }
    assert_eq!(
        marrow(&["doctor", &dir]).status.code(),
        Some(0),
        "doctor must report a clean first run with no lock and no store",
    );
}

/// Seed sources declaring `^notes` and, optionally, a second `^memos` root. A project built
/// with both roots commits a lock recording both; one built with only `^notes` commits a store
/// presenting only that root. Swapping the single-root store under the two-root lock models a
/// partial rollback that dropped one committed root while leaving the other intact.
fn two_root_source(with_memos: bool) -> String {
    let memos = if with_memos {
        "store ^memos(id: int): Note\n\
         \n\
         pub fn seed_memos()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a memo title\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n.body = \"the body text of this memo, long enough to span a cell\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^memos(1) = n\n"
    } else {
        ""
    };
    format!(
        "module app\n\
         \n\
         resource Note\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20required body: string\n\
         store ^notes(id: int): Note\n\
         {memos}\n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a note title\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n.body = \"the body text of this note, long enough to span a cell\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^notes(1) = n\n"
    )
}

/// A store rolled back below one of the roots its committed `marrow.lock` records — here a store
/// presenting only `^notes` swapped under a lock that committed both `^notes` and `^memos` — is a
/// partial durable loss, not a clean absent read. The loss makes the whole store corrupt, so
/// `data get` fails closed under `store.corruption` whether it reads the dropped `^memos` root or
/// the surviving `^notes`, never reporting a query against the rolled-back store as a benign
/// absence — the same verdict its inspection siblings reach on this store.
#[test]
fn a_partial_root_loss_get_is_caught_by_the_lock_root_witness() {
    let two_root = temp_project_uncommitted("cli-store-partial-loss-both", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", &two_root_source(true));
    });
    let two_root_dir = two_root.to_str().expect("two-root dir utf8").to_string();
    for entry in ["app::seed", "app::seed_memos"] {
        assert_eq!(
            marrow(&["run", "--entry", entry, &two_root_dir])
                .status
                .code(),
            Some(0),
            "seed both roots so the lock commits ^notes and ^memos",
        );
    }

    let single_root = temp_project_uncommitted("cli-store-partial-loss-notes", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", &two_root_source(false));
    });
    let single_root_dir = single_root
        .to_str()
        .expect("single-root dir utf8")
        .to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &single_root_dir])
            .status
            .code(),
        Some(0),
        "seed a store presenting only ^notes",
    );

    // Drop ^memos by swapping the single-root store under the two-root lock. The lock still
    // records both committed roots; the store now presents only ^notes.
    std::fs::copy(store_path(&single_root), store_path(&two_root))
        .expect("swap the single-root store under the two-root lock");

    let deadline = std::time::Duration::from_secs(30);
    for path in ["^memos", "^notes(1)"] {
        let command = ["data", "get", &two_root_dir, path];
        let output = marrow_bounded(&command, deadline);
        assert_no_panic_and_bounded(&output, &command, 0);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`data get {path}` blessed a store rolled back below a committed root: {output:?}",
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`data get {path}` over a partially rolled-back store must report store.corruption: \
             {output:?}",
        );
    }
}

/// A healthy store backs up and restores into a fresh project, and the restored store
/// re-verifies clean reporting the same record count: the structural digest and the
/// lock-root witness both survive the round-trip.
#[test]
fn a_backup_restore_round_trip_re_verifies_the_record_count() {
    const SEEDED: u64 = 64;
    let (project, dir) = seeded_mutable_store("cli-store-roundtrip-source", SEEDED as u32);
    let backup_target = project.join("roundtrip.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    assert_eq!(
        marrow(&["backup", &dir, &backup_target]).status.code(),
        Some(0),
        "back up a healthy store",
    );

    let restored = temp_project_uncommitted("cli-store-roundtrip-restored", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", &mutable_count_source(SEEDED as u32));
    });
    let restored_dir = restored.to_str().expect("restored dir utf8").to_string();
    assert_eq!(
        marrow(&["restore", &restored_dir, &backup_target])
            .status
            .code(),
        Some(0),
        "restore the archive into a fresh project",
    );

    let integrity = marrow(&["data", "integrity", &restored_dir]);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "the restored store must re-verify clean: {integrity:?}",
    );
    let stats = marrow(&["data", "stats", &restored_dir]);
    assert_eq!(
        reported_record_count(&stats),
        Some(SEEDED),
        "the restored store must report the archived record count: {stats:?}",
    );
    assert_eq!(
        reported_cell_count(&stats),
        Some(SEEDED * 2),
        "the restored store must report the archived cell count: {stats:?}",
    );
}

/// A single byte flipped inside a committed value — a torn-but-decodable body that keeps
/// the record and cell counts unchanged — must fail the structural-digest cross-check. The
/// flip targets the stored bytes of one record's `body`, so redb still reads the cell and a
/// record count alone would bless the tampered value; the content-sensitive digest changes
/// with the bytes, so `data integrity` and the archiving and repair paths fail closed.
#[test]
fn a_torn_value_byte_is_caught_by_the_structural_digest() {
    const SEEDED: u64 = 32;
    let unique_body = "tornsentinelbodyuniquetokenABCDEF";
    let project = temp_project_uncommitted("cli-store-torn-value", |root| {
        write(root, "marrow.json", native_config());
        let source = mutable_count_source(SEEDED as u32).replace(
            "the body text of this note, long enough to span a cell",
            unique_body,
        );
        write(root, "src/app.mw", &source);
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "seed a store with a locatable body value",
    );

    let store = store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");
    let token = unique_body.as_bytes();
    let at = clean
        .windows(token.len())
        .position(|window| window == token)
        .expect("the unique body token must appear verbatim in the store body");

    // Flip the first byte of the stored body. The cell still decodes, so the store reads
    // through it, but its bytes no longer match the digest the commit stamped.
    let mut corrupt = clean.clone();
    corrupt[at] ^= 0xff;
    std::fs::write(&store, &corrupt).expect("write corrupted store body");

    let backup_target = project.join("torn.mw-backup");
    let backup_target = backup_target
        .to_str()
        .expect("backup path utf8")
        .to_string();
    let deadline = std::time::Duration::from_secs(30);
    for command in [
        ["data", "integrity", &dir].as_slice(),
        ["data", "stats", &dir].as_slice(),
        ["backup", &dir, &backup_target].as_slice(),
        ["data", "recover", &dir].as_slice(),
    ] {
        let output = marrow_bounded(command, deadline);
        assert_no_panic_and_bounded(&output, command, at);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`marrow {}` blessed a store with a torn committed value: {output:?}",
            command.join(" "),
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`marrow {}` must report store.corruption on a torn value: {output:?}",
            command.join(" "),
        );
    }
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

/// Every read-only inspection proves the store fully traversable before rendering, the
/// same `verify_readable` walk `data integrity` runs, so the lightweight render verbs
/// are never more permissive than the verifying one. A read-only open only checks the
/// table roots; damage below them — a record the descent cannot re-seek, a structurally
/// malformed or out-of-order index cell — opens cleanly and a partial render would read
/// straight through it. For every byte flip where `data integrity` reports
/// `store.corruption`, each render verb (`stats`, `roots`, `dump`, `get`) must also fail
/// closed with `store.corruption`, never bless the store with an exit-0 verdict. The
/// sweep is bounded so a regression cannot hang the suite.
#[test]
fn read_only_renders_are_never_more_permissive_than_integrity() {
    let (project, dir) = seeded_bulk_store("cli-store-render-vs-integrity");
    let store = store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");
    let deadline = std::time::Duration::from_secs(30);

    let renders: &[&[&str]] = &[
        &["data", "stats", &dir],
        &["data", "roots", &dir],
        &["data", "dump", &dir],
        &["data", "get", &dir, "^counter(1)"],
    ];

    for offset in (8192..clean.len().min(49152)).step_by(256) {
        let mut corrupt = clean.clone();
        corrupt[offset] ^= 0xff;
        std::fs::write(&store, &corrupt).expect("write corrupted store body");

        let integrity = marrow_bounded(&["data", "integrity", &dir], deadline);
        assert_no_panic_and_bounded(&integrity, &["data", "integrity", &dir], offset);
        // The contract bites only where the verifying verb anchors store corruption: a
        // value-level finding it surfaces over a readable store is not a traversal fault.
        if integrity.status.code() != Some(1) || stderr_code(&integrity) != "store.corruption" {
            continue;
        }

        for command in renders {
            let output = marrow_bounded(command, deadline);
            assert_no_panic_and_bounded(&output, command, offset);
            assert_ne!(
                output.status.code(),
                Some(0),
                "offset {offset}: `marrow {}` blessed a store `data integrity` reports corrupt \
                 (a render must not be more permissive than the verifying verb): {output:?}",
                command.join(" ")
            );
            assert_eq!(
                stderr_code(&output),
                "store.corruption",
                "offset {offset}: `marrow {}` must report store.corruption: {output:?}",
                command.join(" ")
            );
        }
    }
}

/// A read-only inspection must agree with the write-capable open `run` uses on a store
/// damaged in redb's transaction-slot header — the commit-tracker region a read-only open
/// reads through using the last clean commit's recorded roots. A flip there leaves the
/// committed data btrees intact, so a bare read-only open succeeds and a content walk
/// reads straight past the damage, while the write path consults that header and rejects
/// the store. The inspection family must not be more permissive: every read-only verb
/// (`integrity`, `stats`, `roots`, `dump`, `get`) must report `store.corruption` and exit
/// 1 on the same bytes `run` reports corrupt, never a false `verified` (exit 0).
///
/// Each command runs over the same corrupt bytes written fresh from a clean snapshot, so a
/// command that opens write-capable and rewrites the header — and a toggling re-flip —
/// cannot restore the store between commands. Every command is bounded so a regression
/// cannot hang the suite.
#[test]
fn inspection_agrees_with_write_open_on_commit_tracker_corruption() {
    let (project, dir) = seeded_bulk_store("cli-store-commit-tracker-agreement");
    let store = store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");
    let deadline = std::time::Duration::from_secs(30);

    // The flip lands in redb's first transaction-slot header, leaving the data btrees
    // intact so a bare read-only open reads through the committed roots. A clean snapshot
    // is rewritten before each command so the corruption is identical every time.
    let mut corrupt = clean.clone();
    corrupt[128] ^= 0xff;
    let restore_corrupt = || std::fs::write(&store, &corrupt).expect("write corrupted body");

    // The write-capable open is the oracle: `run` reports the store corrupt without a
    // read-only open's permissive read-through. The inspection family must match it.
    restore_corrupt();
    let run = marrow_bounded(&["run", "--entry", "app::seed", &dir], deadline);
    assert_no_panic_and_bounded(&run, &["run"], 0);
    assert_eq!(
        run.status.code(),
        Some(1),
        "the commit-tracker flip must make the write-capable open reject the store: {run:?}"
    );

    let inspections: &[&[&str]] = &[
        &["data", "integrity", &dir],
        &["data", "stats", &dir],
        &["data", "roots", &dir],
        &["data", "dump", &dir],
        &["data", "get", &dir, "^counter(1)"],
    ];
    for command in inspections {
        restore_corrupt();
        let output = marrow_bounded(command, deadline);
        assert_no_panic_and_bounded(&output, command, 0);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`marrow {}` blessed a commit-tracker-corrupt store the write open rejects \
             (a read-only inspection must not be more permissive than the write path): {output:?}",
            command.join(" ")
        );
        assert_eq!(
            stderr_code(&output),
            "store.corruption",
            "`marrow {}` must report store.corruption: {output:?}",
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

/// `data recover --format json` emits exactly one structured-report object. On a
/// recovery-required store the schema-load probe opens the store read-only and fails;
/// that probe must not write its own error envelope to the recover command's stdout
/// before the single `status: opened` object. A second JSON object would break the
/// one-object contract every JSON consumer relies on.
#[test]
fn data_recover_json_emits_exactly_one_object_after_recovery() {
    let (project, dir) = seeded_project("cli-store-recover-json-single");
    corrupt_primary_slot_selector(&project);

    let output = marrow(&["data", "recover", "--format", "json", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8 stdout");
    let objects = stdout
        .split_inclusive('\n')
        .filter(|line| !line.trim().is_empty())
        .count();
    assert_eq!(
        objects, 1,
        "recover --format json must emit exactly one object, got {objects}: {stdout:?}"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("recover stdout is one JSON object");
    assert_eq!(
        value["status"],
        serde_json::json!("opened"),
        "the single object is the recovery result: {value}"
    );
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

/// A store path occupied by a symlink loop (`ELOOP`) is present, not absent: it is a
/// store file the process cannot open, not a first run with no store. `Path::exists`
/// collapses the `ELOOP` stat error to absent, so every read-only inspection would bless
/// it as an empty healthy first run while `run` correctly reports `store.io`. The
/// presence gate distinguishes a physically-absent file from a stat error, so a
/// present-but-unopenable store routes to the typed open failure on every command.
#[cfg(unix)]
#[test]
fn a_symlink_loop_store_is_not_a_first_run() {
    let project = native_project("cli-store-symlink-loop");
    let dir = project.to_str().expect("dir utf8").to_string();
    let path = store_path(&project);
    std::fs::create_dir_all(path.parent().expect("store parent")).expect("create store dir");
    let sibling = path.with_file_name("marrow.loop.redb");
    std::os::unix::fs::symlink(&sibling, &path).expect("link store to sibling");
    std::os::unix::fs::symlink(&path, &sibling).expect("link sibling back to store");
    assert!(
        !path.exists(),
        "Path::exists collapses the ELOOP stat error to absent"
    );

    let commands: &[&[&str]] = &[
        &["data", "stats", &dir],
        &["data", "dump", &dir],
        &["data", "roots", &dir],
        &["data", "integrity", &dir],
        &["data", "get", &dir, "^counter(1)"],
    ];
    for command in commands {
        let output = marrow(command);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`marrow {}` must fail closed on an unopenable store, not bless a first run: {output:?}",
            command.join(" ")
        );
        assert_eq!(
            stderr_code(&output),
            "store.io",
            "`marrow {}` must report store.io for a symlink-loop store: {output:?}",
            command.join(" ")
        );
    }
}
