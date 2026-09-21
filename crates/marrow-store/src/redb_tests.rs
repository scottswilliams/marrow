//! Native redb adapter controls: open classification, hardening, conformance and the
//! memory/redb differential.

use std::sync::atomic::Ordering;

use marrow_codes::Code;
use redb::{ReadableDatabase, TableDefinition};

use super::{FORMAT_VERSION, META, NativeEngine, TABLE, create_raw, map_open_error, reopen_raw};
use crate::conformance;
use crate::engine::{ByteEngine, CommitOutcome, ReadView, WriteTxn};
use crate::error::StoreError;
use marrow_test_support::Scratch;

#[test]
fn read_only_admission_selects_the_writable_allocator_path_without_full_repair() {
    use std::sync::{Arc, atomic::AtomicBool};

    let dir = Scratch::new("admitted-allocator");
    let path = dir.path().join("store.redb");
    drop(NativeEngine::create_new(&path).expect("provision"));
    drop(NativeEngine::open_read_only(&path).expect("read-only admission"));
    let called = Arc::new(AtomicBool::new(false));
    let callback_called = Arc::clone(&called);
    let mut builder = redb::Database::builder();
    builder.set_repair_callback(move |session| {
        callback_called.store(true, Ordering::SeqCst);
        session.abort();
    });
    let db = super::open_past_lock_release(&path, || builder.open(&path))
        .expect("admitted bytes take the saved allocator path");
    assert!(
        !called.load(Ordering::SeqCst),
        "read-only success must exclude full repair"
    );
    drop(db);
}

#[test]
#[cfg(panic = "unwind")]
fn service_preparation_aborts_full_repair_of_a_panicked_store() {
    struct UncleanClose;

    let dir = Scratch::new("panicked-preparation");
    let path = dir.path().join("store.redb");
    let writer_path = path.clone();
    let failure = std::thread::spawn(move || {
        let _store = NativeEngine::create_new(&writer_path).expect("provision before panic");
        std::panic::panic_any(UncleanClose);
    })
    .join()
    .expect_err("writer must unwind with its database open");
    assert!(
        failure.is::<UncleanClose>(),
        "the writer failed before the intended panic"
    );
    assert!(matches!(
        NativeEngine::open_for_service(&path),
        Err(StoreError::RecoveryRequired)
    ));
    drop(NativeEngine::open_existing(&path).expect("ordinary opening can recover"));
}

#[cfg(unix)]
#[test]
fn missing_symlink_target_detection_stops_relative_cycles() {
    let root = Scratch::new("redb-symlink-cycle");
    let data_dir = root.path().join(".data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let store_path = data_dir.join("marrow.redb");
    std::os::unix::fs::symlink("../.data/marrow.redb", &store_path)
        .expect("create relative symlink cycle");

    assert_eq!(
        super::missing_file_or_symlink_target(&store_path).expect("resolve symlink target"),
        None,
        "relative symlink cycles must not spin while preparing owner-only creation"
    );
}

/// An unreadable store file — a regular store body whose mode denies access while
/// its parent directory stays searchable — is a permission fault, not a transient
/// I/O blip. The engine open on the denied body must carry the typed
/// `store.permission_denied` code and name the path on every open path, never
/// collapse into the `store.io` catch-all with a raw errno.
#[cfg(unix)]
#[test]
fn opening_a_denied_store_file_is_permission_denied_on_every_open_path() {
    use std::os::unix::fs::PermissionsExt;

    let dir = Scratch::new("redb-denied-file");
    let path = dir.path().join("marrow.redb");
    {
        let mut store = NativeEngine::create_new(&path).expect("create fresh store");
        let mut txn = store.begin().expect("begin");
        txn.put(b"k", b"v".to_vec()).expect("write");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
        .expect("deny access to the store file");

    for result in [
        NativeEngine::open_existing(&path).map(|_| ()),
        NativeEngine::open_read_only(&path).map(|_| ()),
    ] {
        match result {
            Err(StoreError::PermissionDenied { path: reported }) => {
                assert_eq!(reported, path);
            }
            other => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
                panic!("a denied store file must be permission_denied, got {other:?}");
            }
        }
    }

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
}

/// A store path that is a symlink loop (`ELOOP`) or a dangling symlink to a
/// missing target (`ENOENT`) fails closed as the transient `store.io` on every
/// open path, and the rendered message never embeds the platform errno the OS
/// error string carries.
#[cfg(unix)]
#[test]
fn opening_a_symlink_loop_or_dangling_target_is_io_without_a_raw_errno() {
    let dir = Scratch::new("redb-symlink");

    let loop_a = dir.path().join("loop-a.redb");
    let loop_b = dir.path().join("loop-b.redb");
    std::os::unix::fs::symlink(&loop_b, &loop_a).expect("link a -> b");
    std::os::unix::fs::symlink(&loop_a, &loop_b).expect("link b -> a");

    let dangling = dir.path().join("dangling.redb");
    std::os::unix::fs::symlink(dir.path().join("absent.redb"), &dangling)
        .expect("link to a missing target");

    let expect_io = |result: Result<(), StoreError>, label: &str| match result {
        Err(error @ StoreError::Io { .. }) => assert!(
            !error.to_string().contains("(os error"),
            "store.io message must not leak the OS errno ({label}): {error}"
        ),
        other => panic!("expected store.io ({label}), got {other:?}"),
    };

    // A symlink loop is rejected before any handle opens, on every open path.
    expect_io(NativeEngine::create_new(&loop_a).map(|_| ()), "loop create");
    expect_io(
        NativeEngine::open_existing(&loop_a).map(|_| ()),
        "loop existing",
    );
    expect_io(
        NativeEngine::open_read_only(&loop_a).map(|_| ()),
        "loop read-only",
    );

    // A dangling target is a missing store to both existing-only opens; `open`
    // creates the target, so only the non-creating paths surface the fault.
    expect_io(
        NativeEngine::open_existing(&dangling).map(|_| ()),
        "dangling existing",
    );
    expect_io(
        NativeEngine::open_read_only(&dangling).map(|_| ()),
        "dangling read-only",
    );
}

/// The redb-error mapping is damage-faithful: a recoverable unclean shutdown, a
/// reported corruption, a torn body, a read/write lock conflict, a denied open, and a
/// transient fault each land on their own typed code instead of collapsing to `store.io`.
#[test]
fn map_open_error_classifies_each_redb_failure() {
    let path = std::path::Path::new("/tmp/marrow-store.redb");

    assert_eq!(
        map_open_error(path, redb::DatabaseError::RepairAborted).code(),
        Code::StoreRecoveryRequired
    );
    assert_eq!(
        map_open_error(
            path,
            redb::DatabaseError::Storage(redb::StorageError::Corrupted("torn page".into()))
        )
        .code(),
        Code::StoreCorruption
    );
    assert_eq!(
        map_open_error(
            path,
            redb::DatabaseError::Storage(redb::StorageError::Io(std::io::Error::from(
                std::io::ErrorKind::UnexpectedEof
            )))
        )
        .code(),
        Code::StoreCorruption
    );
    match map_open_error(path, redb::DatabaseError::DatabaseAlreadyOpen) {
        StoreError::Locked { data_dir } => assert_eq!(data_dir, path),
        other => panic!("expected store.locked, got {other:?}"),
    }
    match map_open_error(
        path,
        redb::DatabaseError::Storage(redb::StorageError::Io(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ))),
    ) {
        StoreError::PermissionDenied { path: reported } => assert_eq!(reported, path),
        other => panic!("expected store.permission_denied, got {other:?}"),
    }
}

/// The native store satisfies the same backend conformance suite as the
/// in-memory store — one contract, two backends.
#[test]
fn redb_store_passes_the_conformance_suite() -> Result<(), StoreError> {
    let dir = Scratch::new("redb-test");
    let mut counter = 0;
    conformance::run_all(|| {
        // Each law gets a fresh redb file in the shared temp dir; the dir (and
        // its files) outlives every store, dropping only when the test ends.
        counter += 1;
        let path = dir.path().join(format!("store-{counter}.redb"));
        NativeEngine::create_new(&path)
    })
}

/// A fresh store passes its integrity audit, and an externally byte-mutated
/// store body is rejected by the audit — as a returned typed error or a
/// contained panic — rather than read back silently altered or crashing.
#[test]
fn audit_detects_external_corruption_without_crashing() {
    use std::io::{Read, Seek, SeekFrom, Write};

    let dir = Scratch::new("redb-audit");
    let path = dir.path().join("audit.redb");
    let mut store = NativeEngine::create_new(&path).expect("open fresh");

    // Flip a spread of live bytes in the store body out from under redb.
    let corrupt = || {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("reopen store file");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read body");
        for offset in (0..bytes.len()).step_by(97) {
            bytes[offset] ^= 0xFF;
        }
        file.seek(SeekFrom::Start(0)).expect("seek");
        file.write_all(&bytes).expect("write mutated body");
        file.sync_all().expect("sync mutated body");
    };

    // The audit must fail closed with a typed error, and the adapter must also
    // contain redb's second traversal from `Database::drop` rather than letting
    // it unwind across the storage boundary.
    let audited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let law = conformance::a_corrupted_store_fails_its_integrity_audit(&mut store, corrupt);
        drop(store);
        law
    }));
    match audited {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("the corruption law reported a store failure: {error:?}"),
        Err(_) => panic!("the adapter must contain redb's panic as a typed error"),
    }
}

/// The memory and redb engines compute the same ordered-byte algebra: an
/// identical put/remove sequence, read back by point `get` and by `scan_after`
/// at the boundary minus/at/plus each key, agrees cell-for-cell across both.
#[test]
fn memory_and_redb_agree_byte_for_byte() {
    use crate::MemoryEngine;

    fn apply<E: ByteEngine>(engine: &mut E) -> Vec<(Vec<u8>, Vec<u8>)> {
        {
            let mut txn = engine.begin().expect("begin");
            for n in 0..40u32 {
                let key = format!("\x30{:02}", (n * 3) % 20).into_bytes();
                if n.is_multiple_of(4) {
                    txn.remove(&key).expect("remove");
                } else {
                    txn.put(&key, format!("v{n}").into_bytes()).expect("put");
                }
            }
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let view = engine.read_view().expect("view");
        // Page the whole \x30 range and probe boundary cursors around each key.
        let mut all = Vec::new();
        let mut cursor = b"\x30".to_vec();
        loop {
            let page = view.scan_after(b"\x30", &cursor).expect("scan");
            let Some((last, _)) = page.last().cloned() else {
                break;
            };
            cursor = last;
            all.extend(page);
        }
        for (key, _) in all.clone() {
            let mut minus = key.clone();
            *minus.last_mut().unwrap() -= 1;
            // A cursor just below a key includes it; at it excludes it.
            assert!(
                view.scan_after(b"\x30", &minus)
                    .expect("minus")
                    .iter()
                    .any(|(k, _)| *k == key)
            );
            assert!(
                view.scan_after(b"\x30", &key)
                    .expect("at")
                    .iter()
                    .all(|(k, _)| *k != key)
            );
        }
        all
    }

    let mem = apply(&mut MemoryEngine::new());

    let dir = Scratch::new("redb-diff");
    let path = dir.path().join("diff.redb");
    let native = apply(&mut NativeEngine::create_new(&path).expect("open native"));

    assert_eq!(mem, native, "memory and redb disagree on the byte algebra");
}

/// A foreign or meta-less redb file — one with tables but no `marrow.meta` —
/// must be rejected as corruption, not silently adopted and stamped as a
/// Marrow store. (`Database::create` opens existing files too, so `open` tells
/// a brand-new database from an existing one by whether it has any tables.)
#[test]
fn open_rejects_an_existing_file_missing_meta() {
    let dir = Scratch::new("redb-test");
    let path = dir.path().join("foreign.redb");

    // Build a non-empty redb file with some other table and no `marrow.meta`.
    {
        let db = create_raw(&path, "foreign db");
        let write = db.begin_write().expect("begin");
        const OTHER: TableDefinition<&str, u32> = TableDefinition::new("not.marrow");
        write.open_table(OTHER).expect("open foreign table");
        write.commit().expect("commit foreign db");
    }

    for result in [
        NativeEngine::open_existing(&path),
        NativeEngine::open_read_only(&path),
    ] {
        match result {
            Err(StoreError::Corruption { .. }) => {}
            Err(other) => panic!("expected corruption for a meta-less file, got {other:?}"),
            Ok(_) => panic!("a meta-less file must not be adopted as a Marrow store"),
        }
    }

    let db = reopen_raw(&path, "foreign database");
    let read = db.begin_read().expect("read foreign database");
    assert!(
        matches!(
            read.open_table(META),
            Err(redb::TableError::TableDoesNotExist(_))
        ),
        "an existing-only open must not stamp the foreign database",
    );
}

#[test]
fn existing_only_open_refuses_empty_malformed_and_unstamped_files_without_adopting_them() {
    let dir = Scratch::new("redb-invalid-existing");

    for (name, bytes) in [
        ("empty.redb", b"".as_slice()),
        ("malformed.redb", b"not redb"),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write invalid body");
        assert!(
            NativeEngine::open_existing(&path).is_err(),
            "{name} must be refused",
        );
        assert_eq!(
            std::fs::read(&path).expect("read invalid body"),
            bytes,
            "{name} must not be rewritten or stamped",
        );
    }

    let unstamped = dir.path().join("unstamped.redb");
    drop(create_raw(&unstamped, "valid unstamped redb database"));
    assert!(
        NativeEngine::open_existing(&unstamped).is_err(),
        "a valid but unstamped redb database is not a Marrow store",
    );
    let db = reopen_raw(&unstamped, "unstamped database");
    let read = db.begin_read().expect("read unstamped database");
    assert!(
        matches!(
            read.open_table(META),
            Err(redb::TableError::TableDoesNotExist(_))
        ),
        "the existing-only open must not adopt or stamp an empty redb database",
    );
}

#[test]
fn open_rejects_unsupported_format_version_with_typed_error() {
    let dir = Scratch::new("redb-test");
    let path = dir.path().join("future-format.redb");
    let unsupported = FORMAT_VERSION + 1;

    {
        let db = create_raw(&path, "redb file");
        let write = db.begin_write().expect("begin");
        {
            let mut meta = write.open_table(META).expect("open meta table");
            meta.insert("format_version", unsupported)
                .expect("write future format version");
        }
        {
            let _table = write.open_table(TABLE).expect("open data table");
        }
        write.commit().expect("commit future-format store");
    }

    for result in [
        NativeEngine::open_existing(&path),
        NativeEngine::open_read_only(&path),
    ] {
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("future format version must be rejected"),
        };
        assert_eq!(error.code(), Code::StoreFormatVersion);
        match error {
            StoreError::FormatVersion { found, supported } => {
                assert_eq!(found, unsupported);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected format version error, got {other:?}"),
        }
    }
}

/// A brand-new file is created and stamped, and reopening the stamped store
/// succeeds — the new-vs-existing distinction does not break the normal path.
#[test]
fn create_new_then_open_existing_round_trips_a_fresh_store() {
    let dir = Scratch::new("redb-test");
    let path = dir.path().join("fresh.redb");
    {
        let mut store = NativeEngine::create_new(&path).expect("create fresh");
        let mut txn = store.begin().expect("begin");
        txn.put(b"k", b"v".to_vec()).expect("write");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let store = NativeEngine::open_existing(&path).expect("reopen stamped store");
    assert_eq!(
        store
            .read_view()
            .expect("read view")
            .get(b"k")
            .expect("read"),
        Some(b"v".to_vec())
    );
}

/// The lifecycle open primitive is existing-only and write-capable: it must
/// leave a missing path absent, yet permit ordinary transactions after a
/// provisioner has created and stamped the store.
#[test]
fn existing_only_open_never_creates_and_remains_write_capable() {
    let dir = Scratch::new("redb-existing-only");
    let path = dir.path().join("store.redb");

    assert!(
        NativeEngine::open_existing(&path).is_err(),
        "a missing store cannot be opened existing-only",
    );
    assert!(
        !path.exists(),
        "an existing-only open must leave a missing path absent",
    );

    drop(NativeEngine::create_new(&path).expect("provisioner creates and stamps store"));
    {
        let mut store = NativeEngine::open_existing(&path).expect("open existing writable");
        let mut txn = store.begin().expect("begin writable transaction");
        txn.put(b"k", b"v".to_vec()).expect("stage value");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let store = NativeEngine::open_existing(&path).expect("reopen existing writable");
    assert_eq!(
        store
            .read_view()
            .expect("read view")
            .get(b"k")
            .expect("read value"),
        Some(b"v".to_vec()),
    );
}

/// A store path that is a FIFO (or any other non-regular file) must fail closed
/// with a typed corruption diagnostic on every open path rather than blocking
/// forever in the `open()` syscall waiting for a writer. The open runs on a worker
/// thread with a deadline so a regression surfaces as a timeout, not a hung suite.
#[cfg(unix)]
#[test]
fn opening_a_fifo_store_fails_closed_without_blocking() {
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = Scratch::new("redb-fifo");
    let path = dir.path().join("marrow.redb");
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("spawn mkfifo");
    assert!(status.success(), "mkfifo failed");

    for label in ["create_new", "open_existing", "open_read_only"] {
        let path = path.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match label {
                "create_new" => NativeEngine::create_new(&path),
                "open_existing" => NativeEngine::open_existing(&path),
                _ => NativeEngine::open_read_only(&path),
            };
            let _ = sender.send(result.map(|_| ()));
        });
        // The regular-file guard fails closed without ever issuing the blocking
        // open, so the result is effectively immediate; the generous deadline only
        // distinguishes a real infinite block from scheduling latency under load.
        match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(Err(StoreError::Corruption { .. })) => {}
            Ok(other) => panic!("{label} on a FIFO should be corruption, got {other:?}"),
            Err(_) => panic!("{label} on a FIFO blocked instead of failing closed"),
        }
    }
}
