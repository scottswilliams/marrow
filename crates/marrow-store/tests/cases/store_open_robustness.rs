//! Opening a damaged or recoverable native store fails closed with a typed
//! `StoreError`, never a process panic. redb panics (`unreachable!()`, layout
//! assertions) during open and repair on a structurally-broken file; a tampered or
//! truncated body must surface `store.corruption`, and a store left needing repair
//! by an unclean shutdown must refuse a read-only open until a write-capable open
//! can handle recovery.

#![cfg(feature = "native")]

use std::io::{Seek, SeekFrom, Write};

use crate::common;
use common::catalog_id;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use redb::{Database, TableDefinition};

/// Seed a native store with enough records to fill several redb pages, so a
/// truncation or mid-file clobber lands in real tree data rather than slack space.
fn seed_store(path: &std::path::Path) {
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let store = TreeStore::open(path).expect("open fresh store");
    for id in 0..200i64 {
        store
            .write_data_value(
                &books,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(title.clone())],
                vec![0xAB; 64],
            )
            .expect("seed record");
    }
}

fn books() -> CatalogId {
    catalog_id("1111111111111111")
}

/// A store truncated below its recorded length has a valid header but a damaged
/// body: redb's open/repair path asserts the file is long enough and panics. The
/// open backstop must convert that into `store.corruption`, on both a write-capable
/// and a read-only open.
#[test]
fn truncated_store_body_opens_as_corruption_not_a_panic() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("truncated.redb");
    seed_store(&path);

    let full_len = std::fs::metadata(&path).expect("metadata").len();
    assert!(full_len > 4096, "seeded store should exceed one page");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for truncation");
    // Keep the header (first page) but drop the rest of the body.
    file.set_len(4096).expect("truncate body");
    drop(file);

    match TreeStore::open(&path) {
        Err(StoreError::Corruption { .. }) => {}
        Err(other) => panic!("write open of a truncated store: expected corruption, got {other:?}"),
        Ok(_) => panic!("a truncated store body must not open cleanly"),
    }
    match TreeStore::open_read_only(&path) {
        Err(StoreError::Corruption { .. }) => {}
        Err(other) => {
            panic!("read-only open of a truncated store: expected corruption, got {other:?}")
        }
        Ok(_) => panic!("a truncated store body must not open read-only cleanly"),
    }
}

/// Clobbering a region of the body with garbage drives redb into its
/// `unreachable!()` btree path during open. The backstop must still report
/// `store.corruption` rather than aborting the process.
#[test]
fn clobbered_store_body_opens_as_corruption_not_a_panic() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("clobbered.redb");
    seed_store(&path);

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for clobber");
    // Overwrite a stretch of the body past the header with garbage.
    file.seek(SeekFrom::Start(8192)).expect("seek into body");
    file.write_all(&[0x5A; 8192]).expect("clobber body");
    drop(file);

    let error = TreeStore::open(&path).err().unwrap_or_else(|| {
        panic!("a clobbered store body must not open cleanly");
    });
    assert_eq!(error.code(), "store.corruption", "{error:?}");
}

/// An unclean shutdown leaves the file flagged as needing repair. A read-only open
/// refuses to repair with `StoreError::RecoveryRequired`. A write-capable open
/// repairs the store transparently and leaves seeded data readable.
#[test]
fn store_needing_repair_reports_recovery_required_read_only() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("unclean.redb");
    seed_store(&path);

    // redb records "recovery required" in the file header; flipping that flag bit
    // reproduces the state an unclean shutdown leaves without touching tree data.
    flip_recovery_flag(&path);

    let error = TreeStore::open_read_only(&path)
        .err()
        .unwrap_or_else(|| panic!("a store needing repair must not open read-only cleanly"));
    assert_eq!(error, StoreError::RecoveryRequired);
    assert_eq!(error.code(), "store.recovery_required", "{error:?}");

    // A write-capable open repairs the store, and every seeded record survives.
    let recovered = TreeStore::open(&path).expect("write open repairs the store");
    let title = catalog_id("2222222222222222");
    assert_eq!(
        recovered
            .read_data_value(
                &books(),
                &[SavedKey::Int(0)],
                &[DataPathSegment::Member(title)]
            )
            .expect("read recovered record"),
        Some(vec![0xAB; 64])
    );
}

/// A store with damaged pages and a recovery-required flag refuses read-only
/// repair. Write-capable recovery reports corruption when destroyed pages cannot
/// be replayed.
#[test]
fn damaged_store_flagged_for_recovery_reports_corruption_after_read_only_refusal() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("damaged-unclean.redb");
    seed_store(&path);

    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open for clobber");
        file.seek(SeekFrom::Start(8192)).expect("seek into body");
        file.write_all(&[0x5A; 8192]).expect("clobber a data page");
    }
    flip_recovery_flag(&path);

    let read_only = TreeStore::open_read_only(&path)
        .err()
        .expect("a damaged, repair-flagged store does not open read-only");
    assert_eq!(read_only, StoreError::RecoveryRequired);
    assert_eq!(read_only.code(), "store.recovery_required", "{read_only:?}");

    // Write-capable recovery reports corruption instead of silently opening the store.
    let write = TreeStore::open(&path)
        .err()
        .expect("a damaged store does not survive the recovery replay");
    assert_eq!(write.code(), "store.corruption", "{write:?}");
}

/// A clean store still opens normally on both paths — the backstop and the new
/// mapping add no regression for the healthy case.
#[test]
fn clean_store_still_opens_on_both_paths() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("clean.redb");
    seed_store(&path);

    TreeStore::open(&path).expect("write open of a clean store");
    TreeStore::open_read_only(&path).expect("read-only open of a clean store");
}

/// A missing store file is absent, not corrupt: read-only open fails (it never
/// creates), and a write open creates a fresh store.
#[test]
fn missing_store_file_is_absent_not_corrupt() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("missing.redb");

    let read_only = TreeStore::open_read_only(&path)
        .err()
        .expect("absent file errors");
    assert_ne!(
        read_only.code(),
        "store.corruption",
        "an absent file is not corruption: {read_only:?}"
    );

    TreeStore::open(&path).expect("write open creates a missing store");
}

/// A write-capable open-existing path is for repair, not first-run creation: an
/// empty file is non-store data and must fail closed without initializing redb.
#[test]
fn open_existing_rejects_an_empty_file_as_corruption() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("empty.redb");
    std::fs::File::create(&path).expect("create empty file");
    assert_eq!(std::fs::metadata(&path).expect("metadata").len(), 0);

    let error = TreeStore::open_existing(&path)
        .err()
        .expect("an empty file must not open as an existing Marrow store");
    assert_eq!(error.code(), "store.corruption", "{error:?}");
    assert_eq!(
        std::fs::metadata(&path).expect("metadata").len(),
        0,
        "open_existing must not initialize an empty file"
    );
}

/// A redb file with Marrow metadata but no Marrow data table is not a complete
/// store. Recovery must reject it instead of reporting a successful open that
/// fails on the next read-only command.
#[test]
fn open_existing_rejects_a_meta_only_file_as_corruption() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("meta-only.redb");

    {
        let db = Database::create(&path).expect("create redb file");
        let write = db.begin_write().expect("begin");
        {
            const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");
            let mut meta = write.open_table(META).expect("open meta table");
            meta.insert("format_version", 1)
                .expect("write accepted format version");
        }
        write.commit().expect("commit meta-only file");
    }

    let error = TreeStore::open_existing(&path)
        .err()
        .expect("a meta-only redb file must not open as a complete Marrow store");
    assert_eq!(error.code(), "store.corruption", "{error:?}");
}

/// Put the file into the typed recovery-required state without changing its data
/// pages. A read-only open must return `StoreError::RecoveryRequired`; only a
/// write-capable open is allowed to attempt recovery.
fn flip_recovery_flag(path: &std::path::Path) {
    use std::io::Read;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("open store header");
    file.seek(SeekFrom::Start(RECOVERY_FLAG_OFFSET))
        .expect("seek to recovery flag");
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).expect("read recovery flag");
    file.seek(SeekFrom::Start(RECOVERY_FLAG_OFFSET))
        .expect("seek back to recovery flag");
    file.write_all(&[byte[0] ^ 0x01])
        .expect("flip recovery flag");
}

/// The byte offset of the recovery-required flag in redb's file header. Toggling it
/// puts the file in the same "needs repair" state an unclean shutdown leaves.
const RECOVERY_FLAG_OFFSET: u64 = 9;
