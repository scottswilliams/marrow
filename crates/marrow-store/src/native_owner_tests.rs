//! Owner-lock controls: exclusion, the unclean obligation, quarantine, and the
//! multi-process coordination protocol.

use super::*;
// Nothing here names `Database`: every redb open in this crate routes through
// `open_past_lock_release`. That is a property of what is written, not an enforced
// one — a fully qualified `::redb::Database::create(...)` would compile here and
// bypass the retry.
use ::redb::{ReadableDatabase, TableDefinition};

use crate::redb::{create_raw, reopen_raw};
use crate::scratch_tests::Scratch;
use marrow_codes::Code;

/// Acquire, bind, and open in the one order production uses.
fn open_existing(
    dir: &Path,
    instance: [u8; 16],
) -> Result<NativeEngineOwner, NativeOwnerOpenError<std::convert::Infallible>> {
    NativeEngineOwner::acquire_existing(dir)
        .expect("acquire the owner lock")
        .bind_and_open_existing(NativeOpenAccess::ReadWrite, instance, || Ok(()))
}

/// The exported native engine satisfies the same backend conformance suite
/// as the in-memory engine, through the production acquire-bind-open path
/// rather than a raw engine handle.
#[test]
fn the_native_owner_passes_the_conformance_suite() -> Result<(), StoreError> {
    let scratch = Scratch::new("native-owner-conformance");
    let mut counter = 0u8;
    crate::conformance::run_all(|| {
        counter += 1;
        let dir = scratch.path().join(format!("store-{counter}"));
        std::fs::create_dir_all(&dir).expect("store directory");
        NativeEngineOwner::provision(&dir)?;
        open_existing(&dir, [counter; 16]).map_err(|error| match error {
            NativeOwnerOpenError::Store(store) => store,
            other => panic!("the conformance owner could not open: {other:?}"),
        })
    })
}

fn marker_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join(NATIVE_LOCK_FILE)).expect("read the owner marker")
}

fn inspect_existing(dir: &Path) -> NativeEngineOwner {
    NativeEngineOwner::acquire_existing(dir)
        .expect("acquire")
        .bind_and_open_existing(NativeOpenAccess::ReadOnly, [0x71; 16], || Ok::<_, ()>(()))
        .expect("inspect")
}

fn assert_handoff_excludes_contenders(dir: &Path) {
    assert!(matches!(
        NativeEngineOwner::acquire_existing(dir),
        Err(NativeOwnerAcquireError::Lock(
            NativeLockError::StoreInUse { .. }
        ))
    ));
}

/// Promote `owner` to service, asserting while its marker descriptor is handed off that a
/// contender is still refused by the directory lock alone.
fn promote(
    mut owner: NativeEngineOwner,
    instance: [u8; 16],
) -> Result<NativeEngineOwner, NativeOwnerOpenError<NativePromotionRefusal>> {
    owner.seam = OwnerSeam::armed(|dir, step| {
        if step == OwnerStep::MarkerHandoff {
            assert_handoff_excludes_contenders(dir);
        }
    });
    owner.into_service(instance)
}

/// A seam that corrupts the live engine once, right after it opens, and reports whether
/// that point was reached. A marker handoff it sees is held to the same exclusion check
/// as [`promote`].
#[cfg(unix)]
fn corrupt_after_open() -> (OwnerSeam, Rc<std::cell::Cell<bool>>) {
    let fired = Rc::new(std::cell::Cell::new(false));
    let armed = Rc::clone(&fired);
    let seam = OwnerSeam::armed(move |dir, step| match step {
        OwnerStep::MarkerHandoff => assert_handoff_excludes_contenders(dir),
        OwnerStep::EngineOpened => {
            if !armed.replace(true) {
                corrupt_live_engine_for_audit(dir);
            }
        }
    });
    (seam, fired)
}

#[test]
#[cfg(unix)]
fn service_promotion_preserves_the_inherited_physical_audit_obligation() {
    let scratch = Scratch::new("native-owner-promote-physical-audit");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    seed_audit_body(scratch.path());
    std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), b"unclean").expect("prior obligation");
    let mut owner = inspect_existing(scratch.path());
    // Clearing the still-held marker cannot erase the obligation read at admission.
    std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), b"").expect("clear marker bytes");
    let (seam, corrupted) = corrupt_after_open();
    owner.seam = seam;
    let result = owner.into_service([0x71; 16]);
    assert!(corrupted.get(), "mutation followed the service engine open");
    match result {
        Err(NativeOwnerOpenError::Store(StoreError::Corruption { .. })) => {}
        Err(error) => panic!("expected the physical audit refusal: {error:?}"),
        Ok(owner) => {
            std::mem::forget(owner);
            panic!("service preparation skipped its inherited physical audit");
        }
    }
    assert!(!marker_bytes(scratch.path()).is_empty());
}

#[test]
fn service_promotion_refuses_marker_appearance_and_removal() {
    for initially_present in [false, true] {
        let scratch = Scratch::new("native-owner-promote-marker-presence");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let marker = scratch.path().join(NATIVE_LOCK_FILE);
        if initially_present {
            std::fs::write(&marker, b"unclean").expect("marker");
        }
        let owner = inspect_existing(scratch.path());
        if initially_present {
            std::fs::remove_file(&marker).expect("remove marker");
        } else {
            std::fs::write(&marker, b"changed").expect("new marker");
        }
        assert!(matches!(
            promote(owner, [0x71; 16]),
            Err(NativeOwnerOpenError::Lock(_))
        ));
        if initially_present {
            assert!(!marker.exists());
        } else {
            assert_eq!(marker_bytes(scratch.path()), b"changed");
        }
    }
}

#[test]
fn service_promotion_preserves_exclusion_and_returns_write_access() {
    for marker in [None, Some(&b""[..]), Some(&b"unclean"[..])] {
        let scratch = Scratch::new("native-owner-promote");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        if let Some(bytes) = marker {
            std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), bytes).expect("seed marker");
        }
        let owner = inspect_existing(scratch.path());
        assert!(matches!(
            NativeEngineOwner::acquire_existing(scratch.path()),
            Err(NativeOwnerAcquireError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
        assert_eq!(
            std::fs::read(scratch.path().join(NATIVE_LOCK_FILE))
                .ok()
                .as_deref(),
            marker
        );
        let mut owner = promote(owner, [0x71; 16]).expect("promote without self-contention");
        assert!(matches!(
            NativeEngineOwner::acquire_existing(scratch.path()),
            Err(NativeOwnerAcquireError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
        let mut txn = owner.begin().expect("writable service");
        txn.put(b"value", vec![42]).expect("write");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        assert_eq!(
            owner
                .read_view()
                .expect("read")
                .get(b"value")
                .expect("value"),
            Some(vec![42])
        );
        drop(owner);
        assert!(marker_bytes(scratch.path()).is_empty());
    }
}

#[test]
fn service_promotion_refuses_replaced_engine_and_marker() {
    for entry in [NATIVE_ENGINE_FILE, NATIVE_LOCK_FILE] {
        let scratch = Scratch::new("native-owner-promote-replaced");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), b"unclean").expect("marker");
        let owner = inspect_existing(scratch.path());
        let path = scratch.path().join(entry);
        let displaced = scratch.path().join("displaced");
        std::fs::rename(&path, &displaced).expect("displace");
        std::fs::copy(&displaced, &path).expect("replace with exact bytes");
        let before = std::fs::read(&path).expect("replacement bytes");
        assert!(promote(owner, [0x71; 16]).is_err());
        assert_eq!(std::fs::read(&path).expect("retained replacement"), before);
        assert_eq!(marker_bytes(scratch.path()), b"unclean");
    }
}

#[test]
fn service_promotion_refuses_writable_and_quarantined_owners() {
    let scratch = Scratch::new("native-owner-promote-writable");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let owner = open_existing(scratch.path(), [0x71; 16]).expect("service");
    assert!(matches!(
        promote(owner, [0x71; 16]),
        Err(NativeOwnerOpenError::Refused(
            NativePromotionRefusal::NotReadOnly
        ))
    ));
    let mut owner = inspect_existing(scratch.path());
    owner.lock.quarantine();
    assert!(matches!(
        promote(owner, [0x71; 16]),
        Err(NativeOwnerOpenError::Refused(
            NativePromotionRefusal::Quarantined
        ))
    ));
    assert!(matches!(
        NativeEngineOwner::acquire_existing(scratch.path()),
        Err(NativeOwnerAcquireError::Lock(
            NativeLockError::StoreInUse { .. }
        ))
    ));
}

#[test]
#[cfg(unix)]
fn retained_directory_metadata_and_exclusion_survive_rename() {
    use std::os::unix::fs::MetadataExt;

    let scratch = Scratch::new("native-owner-retained-directory");
    let original = scratch.path().join("original");
    let moved = scratch.path().join("moved");
    std::fs::create_dir(&original).expect("create directory");
    let owner = NativeEngineOwner::acquire_existing(&original).expect("acquire owner");
    let identity = |metadata: std::fs::Metadata| (metadata.dev(), metadata.ino());
    let held = identity(owner.directory_metadata().expect("retained metadata"));
    std::fs::rename(&original, &moved).expect("rename owned directory");
    std::fs::create_dir(&original).expect("replace old pathname");
    assert_eq!(
        identity(owner.directory_metadata().expect("retained metadata")),
        held
    );
    assert_ne!(
        identity(std::fs::metadata(&original).expect("replacement metadata")),
        held
    );
    assert!(matches!(
        NativeEngineOwner::acquire_existing(&moved),
        Err(NativeOwnerAcquireError::Lock(
            NativeLockError::StoreInUse { .. }
        ))
    ));
    drop(owner);
    drop(NativeEngineOwner::acquire_existing(&moved).expect("released owner"));
}

/// Duplicates retain the same lock descriptions as handles inherited across fork.
fn retain_lock_handles(lock: &OwnerLock) -> Vec<File> {
    [&lock.directory_node, &lock.file]
        .into_iter()
        .flatten()
        .map(|file| file.try_clone().expect("duplicate held lock"))
        .collect()
}

#[test]
fn releasing_an_owner_does_not_wait_for_duplicate_handles_to_close() {
    #[derive(Clone, Copy, Debug)]
    enum Release {
        Pending,
        Refused,
        Clean,
        ReadOnlyUnclean,
    }

    let mut failures = Vec::new();
    for release in [
        Release::Pending,
        Release::Refused,
        Release::Clean,
        Release::ReadOnlyUnclean,
    ] {
        let scratch = Scratch::new("native-owner-duplicate-release");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), b"unclean")
            .expect("inherited audit obligation");
        let pending = NativeEngineOwner::acquire_existing(scratch.path()).expect("acquire");
        assert!(matches!(
            contend(scratch.path()),
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
        ));
        let retained = match release {
            Release::Pending => {
                let retained = retain_lock_handles(&pending.lock);
                assert_eq!(retained.len(), 1);
                drop(pending);
                retained
            }
            Release::Refused => {
                let PendingNativeEngineOwner {
                    mut lock,
                    directory,
                    ..
                } = pending;
                lock.prepare_existing(&directory, NativeOpenAccess::ReadWrite, [0x51; 16])
                    .expect("prepare marker before admission");
                let retained = retain_lock_handles(&lock);
                assert_eq!(retained.len(), 2);
                drop(lock);
                retained
            }
            Release::Clean | Release::ReadOnlyUnclean => {
                let access = match release {
                    Release::Clean => NativeOpenAccess::ReadWrite,
                    _ => NativeOpenAccess::ReadOnly,
                };
                let owner = pending
                    .bind_and_open_existing(access, [0x51; 16], || Ok::<(), ()>(()))
                    .expect("open");
                let retained = retain_lock_handles(&owner.lock);
                assert_eq!(retained.len(), 2);
                drop(owner);
                retained
            }
        };
        assert_eq!(
            marker_bytes(scratch.path()).is_empty(),
            matches!(release, Release::Clean),
            "only a clean owner discharges the inherited obligation",
        );
        match NativeEngineOwner::acquire_existing(scratch.path()) {
            Ok(successor) => {
                drop(retained);
                assert!(matches!(
                    contend(scratch.path()),
                    NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
                ));
                drop(successor);
            }
            Err(error) => {
                failures.push((release, error.code()));
                drop(retained);
            }
        }
        NativeEngineOwner::acquire_existing(scratch.path())
            .expect("control: no owner or duplicate remains");
    }
    assert!(
        failures.is_empty(),
        "release still held by duplicates: {failures:?}"
    );
}

#[test]
fn read_only_ownership_cannot_write_upgrade_or_clear_an_inherited_obligation() {
    for marker in [None, Some(b"".as_slice()), Some(b"unclean".as_slice())] {
        let scratch = Scratch::new("native-owner-read-only");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let marker_path = scratch.path().join(NATIVE_LOCK_FILE);
        if let Some(bytes) = marker {
            std::fs::write(&marker_path, bytes).expect("initial marker");
        }
        let assert_marker = || {
            let observed = match std::fs::read(&marker_path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("read marker: {error}"),
            };
            assert_eq!(observed.as_deref(), marker);
        };
        let path = scratch.path().join(NATIVE_ENGINE_FILE);
        let before = std::fs::read(&path).expect("engine before");
        let pending = NativeEngineOwner::acquire_existing(scratch.path()).expect("acquire");
        assert_marker();
        let mut owner = pending
            .bind_and_open_existing(NativeOpenAccess::ReadOnly, [0x41; 16], || Ok::<(), ()>(()))
            .expect("inspect");
        assert_marker();
        assert!(
            owner
                .read_view()
                .expect("read view")
                .get(b"missing")
                .expect("read")
                .is_none()
        );
        assert!(matches!(owner.begin(), Err(StoreError::ReadOnly { .. })));
        assert!(matches!(
            owner.audit_integrity(),
            Err(StoreError::ReadOnly { .. })
        ));
        assert!(matches!(
            owner.reopen_existing_and_audit(),
            Err(StoreError::ReadOnly {
                op: StoreOp::Recovery
            })
        ));
        assert!(before == std::fs::read(&path).expect("engine after"));
        assert_marker();
    }
}

#[test]
fn a_read_only_open_refuses_required_repair_without_changing_the_engine() {
    let scratch = Scratch::new("native-owner-read-only-recovery");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let path = scratch.path().join(NATIVE_ENGINE_FILE);
    let mut bytes = std::fs::read(&path).expect("engine");
    // The redb 4 header keeps the recovery-required flag immediately after its
    // nine-byte magic. Leave both commit slots intact and require an opener to
    // repair; this fixture must never silently become a clean-open case.
    assert_eq!(&bytes[..9], b"redb\x1a\x0a\xa9\x0d\x0a");
    bytes[9] |= 2;
    std::fs::write(&path, &bytes).expect("require physical recovery");
    let refused = NativeEngineOwner::acquire_existing(scratch.path())
        .expect("acquire")
        .bind_and_open_existing(NativeOpenAccess::ReadOnly, [0x43; 16], || Ok::<(), ()>(()));
    assert!(matches!(
        refused,
        Err(NativeOwnerOpenError::Store(StoreError::RecoveryRequired))
    ));
    assert!(bytes == std::fs::read(path).expect("engine after"));
    assert!(!scratch.path().join(NATIVE_LOCK_FILE).exists());
}

#[test]
fn a_read_only_open_refusal_preserves_the_engine_and_unclean_obligation() {
    let scratch = Scratch::new("native-owner-read-only-malformed");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let path = scratch.path().join(NATIVE_ENGINE_FILE);
    std::fs::write(&path, b"not an engine").expect("malformed engine");
    std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), b"unclean").expect("prior marker");
    let refused = NativeEngineOwner::acquire_existing(scratch.path())
        .expect("acquire")
        .bind_and_open_existing(NativeOpenAccess::ReadOnly, [0x42; 16], || Ok::<(), ()>(()));
    assert!(matches!(refused, Err(NativeOwnerOpenError::Store(_))));
    assert_eq!(std::fs::read(path).expect("engine after"), b"not an engine");
    assert_eq!(marker_bytes(scratch.path()), b"unclean");
}

fn contend(dir: &Path) -> NativeOwnerAcquireError {
    match NativeEngineOwner::acquire_existing(dir) {
        Err(error) => error,
        Ok(_) => panic!("a contender acquired a held store"),
    }
}

#[test]
fn provision_is_create_only_and_existing_open_holds_the_lock() {
    let scratch = Scratch::new("native-owner-provision");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    assert!(NativeEngineOwner::provision(scratch.path()).is_err());

    let owner = open_existing(scratch.path(), [7; 16]).expect("open owner");
    assert!(matches!(
        contend(scratch.path()),
        NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
    ));
    drop(owner);
    open_existing(scratch.path(), [8; 16]).expect("clean close releases lock");
}

/// Exclusion is decided before the store directory is read, and the marker
/// names the holder only after mutable opening publishes its instance. A
/// contender is told the store is locked in both states.
#[test]
fn a_contender_is_locked_out_before_and_after_the_holder_binds_its_instance() {
    let scratch = Scratch::new("native-owner-pending-and-bound-contention");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let pending =
        NativeEngineOwner::acquire_existing(scratch.path()).expect("acquire without an instance");

    match contend(scratch.path()) {
        NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. }) => {
            assert_eq!(error.code(), Code::StoreLocked);
            assert!(matches!(error, NativeLockError::StoreInUse { owner: None }));
            assert!(!scratch.path().join(NATIVE_LOCK_FILE).exists());
        }
        other => panic!("a pending holder must exclude a contender: {other}"),
    }

    let owner = pending
        .bind_and_open_existing(NativeOpenAccess::ReadWrite, [0x5B; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("bind and open");
    match contend(scratch.path()) {
        NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { owner: Some(named) }) => {
            assert_eq!(named.pid, std::process::id());
            assert_eq!(
                named.instance,
                Some([0x5B; 16]),
                "a bound holder names its store"
            );
        }
        other => panic!("a bound holder must exclude a contender: {other}"),
    }
    drop(owner);
}

/// A marker this build cannot read costs a contender the holder's identity and
/// nothing else: the exclusion verdict never degrades into an I/O or decode
/// error, which is the whole reason exclusion is decided ahead of any read.
#[test]
fn an_unreadable_marker_still_yields_exactly_the_exclusion_verdict() {
    for (tag, body) in [
        ("empty", b"".as_slice()),
        ("garbage", b"not a marker"),
        (
            "wrong-magic",
            b"XXXX\x01\x01\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x01",
        ),
        ("truncated-bound", b"MWSL\x01\x02\x00\x00\x00\x01"),
    ] {
        let scratch = Scratch::new(&format!("unreadable-marker-{tag}"));
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let held = open_existing(scratch.path(), [0x5C; 16]).expect("open owner");
        std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), body).expect("overwrite the marker");

        match contend(scratch.path()) {
            NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. }) => {
                assert_eq!(error.code(), Code::StoreLocked, "marker {tag}");
            }
            other => panic!("an unreadable {tag} marker changed the verdict: {other}"),
        }
        drop(held);
    }
}

/// The decoder admits before it indexes. A contender reads a marker it did not
/// write, so a byte pattern that aborts the decode would replace the exclusion
/// verdict with a process abort. Every prefix length of the layout, at every
/// version and state tag, either decodes to a whole layout or reports no identity.
#[test]
fn no_marker_byte_pattern_can_abort_the_decoder() {
    let widest = BOUND_BYTES;
    for version in [0x00, LOCK_VERSION, 0x02, 0xFF] {
        for tag in [0x00, PENDING_TAG, BOUND_TAG, 0xFF] {
            let mut bytes = Vec::with_capacity(widest);
            bytes.extend_from_slice(LOCK_MAGIC);
            bytes.push(version);
            bytes.push(tag);
            bytes.resize(widest, 0xA5);
            for len in 0..=widest {
                assert!(
                    NativeLockOwner::decode(&bytes[..len]).is_none()
                        || matches!(len, PENDING_BYTES | BOUND_BYTES),
                    "a {len}-byte marker at version {version:#04x} tag {tag:#04x} decoded \
                     outside a whole layout",
                );
            }
        }
    }
}

/// Every marker a contender can meet under a live holder still yields exactly the
/// exclusion verdict. The contender did not write these bytes and is owed no verdict
/// about them: each truncation of both layouts, each state tag, a foreign magic, plain
/// garbage, and a marker carrying a second link all read as `store.locked`. A second
/// link in particular does not divide exclusion — every opener of either name locks the
/// same node — so refusing on it would convert contention into an I/O verdict.
#[cfg(unix)]
#[test]
fn every_marker_a_contender_can_meet_still_yields_the_exclusion_verdict() {
    let scratch = Scratch::new("native-owner-contender-marker-sweep");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let held = open_existing(scratch.path(), [0x5D; 16]).expect("open owner");
    let marker = scratch.path().join(NATIVE_LOCK_FILE);

    let mut bodies: Vec<Vec<u8>> = Vec::new();
    for instance in [None, Some([0x5E; 16])] {
        let encoded = NativeLockOwner {
            pid: 4242,
            instance,
            acquired_unix_secs: 7,
        }
        .encode();
        for len in 0..=encoded.len() {
            bodies.push(encoded[..len].to_vec());
        }
    }
    for tag in 0..=u8::MAX {
        bodies.push(vec![
            LOCK_MAGIC[0],
            LOCK_MAGIC[1],
            LOCK_MAGIC[2],
            LOCK_MAGIC[3],
            LOCK_VERSION,
            tag,
        ]);
    }
    bodies.push(b"XXXX\x01\x02".to_vec());
    bodies.push(b"not a marker at all".to_vec());

    for body in &bodies {
        std::fs::write(&marker, body).expect("rewrite the marker under the holder");
        match contend(scratch.path()) {
            NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. }) => {
                assert_eq!(
                    error.code(),
                    Code::StoreLocked,
                    "a {}-byte marker changed the verdict",
                    body.len(),
                );
            }
            other => panic!("a {}-byte marker changed the verdict: {other}", body.len(),),
        }
    }

    std::fs::hard_link(&marker, scratch.path().join("marker-alias")).expect("add a second link");
    match contend(scratch.path()) {
        NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }) => {}
        other => panic!("a multiply-linked marker preempted the exclusion verdict: {other}"),
    }
    drop(held);
}

/// The marker layout round-trips in both states.
#[test]
fn the_marker_round_trips_in_both_states() {
    for instance in [None, Some([0x6A; 16])] {
        let owner = NativeLockOwner {
            pid: 4321,
            instance,
            acquired_unix_secs: 0x0102_0304_0506_0708,
        };
        let encoded = owner.encode();
        assert_eq!(
            encoded.len(),
            match instance {
                Some(_) => BOUND_BYTES,
                None => PENDING_BYTES,
            },
        );
        assert_eq!(NativeLockOwner::decode(&encoded), Some(owner));
    }
}

/// The unclean obligation a crashed holder leaves is inherited by the next
/// acquisition and survives every outcome short of a completed open: a holder
/// that dies pending, a holder that dies bound, and an open refused at
/// admission all leave the next acquisition owing the same full audit. Only a
/// clean close discharges it.
#[test]
fn an_inherited_unclean_obligation_survives_refusal_and_drop() {
    for (tag, bind_before_death) in [("pending-death", false), ("bound-death", true)] {
        let scratch = Scratch::new(tag);
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        std::fs::write(scratch.path().join(NATIVE_LOCK_FILE), b"unclean").expect("prior marker");

        // A holder that never closes cleanly: the marker keeps its body.
        let pending =
            NativeEngineOwner::acquire_existing(scratch.path()).expect("acquire the owner");
        if bind_before_death {
            let refused = pending
                .bind_and_open_existing(NativeOpenAccess::ReadWrite, [0x6C; 16], || {
                    Err::<(), _>("refused")
                })
                .err()
                .expect("the admission refusal is the death point");
            assert!(matches!(refused, NativeOwnerOpenError::Refused("refused")));
        } else {
            drop(pending);
        }
        assert!(
            !marker_bytes(scratch.path()).is_empty(),
            "{tag} must leave the unclean obligation behind",
        );

        // Inheriting it and refusing again hands the same obligation on.
        let inherited = NativeEngineOwner::acquire_existing(scratch.path())
            .expect("inherit the obligation")
            .bind_and_open_existing(NativeOpenAccess::ReadWrite, [0x6D; 16], || {
                Err::<(), _>("refused again")
            })
            .err()
            .expect("the second admission also refuses");
        assert!(matches!(
            inherited,
            NativeOwnerOpenError::Refused("refused again"),
        ));
        assert!(
            !marker_bytes(scratch.path()).is_empty(),
            "{tag} must not let a refusal discharge an inherited obligation",
        );

        // Only a completed open and clean close discharges it.
        drop(open_existing(scratch.path(), [0x6E; 16]).expect("a full open discharges it"));
        assert!(
            marker_bytes(scratch.path()).is_empty(),
            "{tag} must be discharged by a clean close",
        );
    }
}

/// The owner marker is admitted as the store directory's own regular
/// single-link entry: a link standing in for it is refused rather than read or
/// created through, and a second hard link to it is refused outright.
#[cfg(unix)]
#[test]
fn the_owner_marker_refuses_a_substituted_or_multiply_linked_entry() {
    for access in [NativeOpenAccess::ReadOnly, NativeOpenAccess::ReadWrite] {
        let scratch = Scratch::new("native-owner-marker-substitution");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let elsewhere = scratch.path().join("elsewhere");
        let marker = scratch.path().join(NATIVE_LOCK_FILE);
        let open = || {
            NativeEngineOwner::acquire_existing(scratch.path())
                .expect("directory exclusion")
                .bind_and_open_existing(access, [0x68; 16], || Ok::<(), ()>(()))
        };

        std::os::unix::fs::symlink(&elsewhere, &marker).expect("link the marker name away");
        assert!(matches!(
            open(),
            Err(NativeOwnerOpenError::Lock(NativeLockError::Io(_)))
        ));
        assert!(
            !elsewhere.exists(),
            "refusal must not create the link target"
        );
        assert_eq!(std::fs::read_link(&marker).unwrap(), elsewhere);
        std::fs::remove_file(&marker).expect("remove the link");

        std::fs::write(&marker, b"unclean").expect("create a real marker");
        let alias = scratch.path().join("marker-alias");
        std::fs::hard_link(&marker, &alias).expect("add a second link");
        assert!(matches!(
            open(),
            Err(NativeOwnerOpenError::Lock(NativeLockError::Io(_)))
        ));
        assert_eq!(std::fs::read(&marker).unwrap(), b"unclean");
        assert_eq!(std::fs::read(alias).unwrap(), b"unclean");
    }
}

#[test]
fn admission_runs_under_lock_before_engine_open() {
    for access in [NativeOpenAccess::ReadOnly, NativeOpenAccess::ReadWrite] {
        let scratch = Scratch::new("native-owner-admission");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let engine = scratch.path().join(NATIVE_ENGINE_FILE);
        let before = std::fs::read(&engine).expect("engine before");
        let error = NativeEngineOwner::acquire_existing(scratch.path())
            .expect("acquire the owner")
            .bind_and_open_existing(access, [9; 16], || {
                assert!(matches!(
                    contend(scratch.path()),
                    NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
                ));
                if access == NativeOpenAccess::ReadOnly {
                    assert!(!scratch.path().join(NATIVE_LOCK_FILE).exists());
                }
                Err::<(), _>("refused")
            });
        assert!(matches!(
            error,
            Err(NativeOwnerOpenError::Refused("refused"))
        ));
        assert_eq!(std::fs::read(engine).expect("engine after"), before);
        if access == NativeOpenAccess::ReadOnly {
            assert!(!scratch.path().join(NATIVE_LOCK_FILE).exists());
        }
        open_existing(scratch.path(), [10; 16])
            .expect("a pre-engine refusal releases its non-quarantined lock");
    }
}

#[test]
fn existing_owner_open_refuses_missing_and_invalid_bodies_without_adopting_them() {
    let missing = Scratch::new("native-owner-missing-existing");
    let missing_path = missing.path().join(NATIVE_ENGINE_FILE);
    for _ in 0..2 {
        assert!(matches!(
            open_existing(missing.path(), [0x21; 16]),
            Err(NativeOwnerOpenError::Store(_))
        ));
        assert!(
            !missing_path.exists(),
            "an owner open must leave a missing engine path absent",
        );
    }

    for (tag, bytes) in [
        ("empty-existing", b"".as_slice()),
        ("bad-existing", b"not redb"),
    ] {
        let scratch = Scratch::new(tag);
        let path = scratch.path().join(NATIVE_ENGINE_FILE);
        std::fs::write(&path, bytes).expect("write invalid engine body");
        assert!(matches!(
            open_existing(scratch.path(), [0x22; 16]),
            Err(NativeOwnerOpenError::Store(_))
        ));
        assert_eq!(
            std::fs::read(&path).expect("read refused engine body"),
            bytes,
            "an owner open must not rewrite or stamp an invalid engine body",
        );
    }

    let unstamped = Scratch::new("native-owner-unstamped-existing");
    let path = unstamped.path().join(NATIVE_ENGINE_FILE);
    drop(create_raw(&path, "an unstamped redb database"));
    assert!(matches!(
        open_existing(unstamped.path(), [0x23; 16]),
        Err(NativeOwnerOpenError::Store(_))
    ));
    let db = reopen_raw(&path, "refused unstamped database");
    let read = db.begin_read().expect("read unstamped database");
    const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");
    assert!(
        matches!(
            read.open_table(META),
            Err(::redb::TableError::TableDoesNotExist(_))
        ),
        "an owner open must not stamp an otherwise valid foreign database",
    );
}

#[test]
fn recovery_reopen_is_irreversibly_quarantined_after_success() {
    let scratch = Scratch::new("native-owner-quarantine-success");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let owner = open_existing(scratch.path(), [11; 16]).expect("open owner");
    let mut owner = owner
        .reopen_existing_and_audit()
        .expect("reopen and audit under retained lock");
    let mut txn = owner
        .begin()
        .expect("known recovery owner remains writable");
    txn.put(b"known", b"usable".to_vec())
        .expect("write through recovered owner");
    assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    assert_eq!(
        owner
            .read_view()
            .expect("known recovery read view")
            .get(b"known")
            .expect("read through recovered owner"),
        Some(b"usable".to_vec()),
    );
    drop(owner);

    assert!(matches!(
        contend(scratch.path()),
        NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
    ));
    assert_ne!(
        std::fs::metadata(scratch.path().join(NATIVE_LOCK_FILE))
            .expect("lock metadata")
            .len(),
        0,
        "quarantine retains the nonempty owner marker",
    );
}

/// Quarantine retains every node the exclusion rests on, not only the marker. A
/// quarantine standing on the marker alone would end the moment that name is unlinked
/// and another node created under it — the replacement the directory node exists to
/// survive — so the store would become openable again before this process exits.
#[cfg(unix)]
#[test]
fn quarantine_survives_the_replacement_of_the_marker_it_leaked() {
    let scratch = Scratch::new("native-owner-quarantine-replaced-marker");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let owner = open_existing(scratch.path(), [17; 16]).expect("open owner");
    drop(
        owner
            .reopen_existing_and_audit()
            .expect("reopen and audit under retained lock"),
    );

    std::fs::remove_file(scratch.path().join(NATIVE_LOCK_FILE))
        .expect("remove the quarantined marker");
    assert!(matches!(
        contend(scratch.path()),
        NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
    ));
}

#[test]
fn failed_recovery_reopen_never_recreates_and_remains_quarantined() {
    let scratch = Scratch::new("native-owner-quarantine-missing");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    let owner = open_existing(scratch.path(), [13; 16]).expect("open owner");
    let engine_path = scratch.path().join(NATIVE_ENGINE_FILE);
    std::fs::remove_file(&engine_path).expect("remove engine");
    assert!(owner.reopen_existing_and_audit().is_err());
    assert!(
        !engine_path.exists(),
        "recovery must not recreate the engine"
    );
    assert!(matches!(
        contend(scratch.path()),
        NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
    ));
}

#[test]
fn failed_recovery_reopen_never_adopts_invalid_replacements() {
    for (tag, replacement) in [
        ("quarantine-empty", b"".as_slice()),
        ("quarantine-malformed", b"not redb"),
    ] {
        let scratch = Scratch::new(tag);
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let owner = open_existing(scratch.path(), [0x31; 16]).expect("open owner");
        let engine_path = scratch.path().join(NATIVE_ENGINE_FILE);
        std::fs::remove_file(&engine_path).expect("remove live engine path");
        std::fs::write(&engine_path, replacement).expect("install invalid replacement");

        assert!(owner.reopen_existing_and_audit().is_err());
        assert_eq!(
            std::fs::read(&engine_path).expect("read refused replacement"),
            replacement,
            "recovery must not rewrite or stamp an invalid replacement",
        );
        assert!(matches!(
            contend(scratch.path()),
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
        ));
    }
}

struct VerdictTxn(CommitOutcome);

impl ReadView for VerdictTxn {
    fn get(&self, _key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(None)
    }

    fn scan_after(&self, _prefix: &[u8], _cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        Ok(Vec::new())
    }
}

impl WriteTxn for VerdictTxn {
    fn put(&mut self, _key: &[u8], _value: Vec<u8>) -> Result<(), StoreError> {
        Ok(())
    }

    fn remove(&mut self, _key: &[u8]) -> Result<(), StoreError> {
        Ok(())
    }

    fn commit(self) -> CommitOutcome {
        self.0
    }
}

#[test]
fn transaction_wrapper_latches_only_an_indeterminate_engine_outcome() {
    for (tag, outcome, quarantined) in [
        ("confirmed", CommitOutcome::Confirmed, false),
        ("aborted", CommitOutcome::Aborted, false),
        ("indeterminate", CommitOutcome::Indeterminate, true),
    ] {
        let scratch = Scratch::new(tag);
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let mut owner = open_existing(scratch.path(), [17; 16]).expect("open owner");
        assert_eq!(
            commit_and_latch(VerdictTxn(outcome), &mut owner.lock),
            outcome,
            "the transaction wrapper commits once and preserves the engine verdict",
        );
        drop(owner);
        assert_eq!(
            matches!(
                NativeEngineOwner::acquire_existing(scratch.path()),
                Err(NativeOwnerAcquireError::Lock(
                    NativeLockError::StoreInUse { .. }
                ))
            ),
            quarantined,
            "only Indeterminate may retain exclusion",
        );
    }
}

/// The three coordinated recovery cases the parent and child both drive.
/// The pair runs one exhaustive protocol: the tag crosses the process
/// boundary as text and is decoded back into this enum on arrival.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinatedMode {
    /// Reopen and audit both succeed.
    Success,
    /// The engine artifact is gone, so the recovery reopen refuses.
    ReopenFailure,
    /// The engine reopens and its full audit then fails.
    AuditFailure,
}

#[cfg(unix)]
impl CoordinatedMode {
    const ALL: [Self; 3] = [Self::Success, Self::ReopenFailure, Self::AuditFailure];

    fn decode(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.to_string() == tag)
    }
}

#[cfg(unix)]
impl std::fmt::Display for CoordinatedMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Success => "success",
            Self::ReopenFailure => "reopen-failure",
            Self::AuditFailure => "audit-failure",
        })
    }
}

/// The rendezvous points of one coordinated case. Each is published by the
/// child and released by the parent.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinatedPhase {
    BeforeRecovery,
    RecoveredLive,
    RecoveredDropped,
    ReopenRefused,
    ReopenedBeforeAudit,
    AuditRefused,
}

#[cfg(unix)]
impl std::fmt::Display for CoordinatedPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::BeforeRecovery => "before-recovery",
            Self::RecoveredLive => "recovered-live",
            Self::RecoveredDropped => "recovered-dropped",
            Self::ReopenRefused => "reopen-refused",
            Self::ReopenedBeforeAudit => "reopened-before-audit",
            Self::AuditRefused => "audit-refused",
        })
    }
}

#[cfg(unix)]
struct ChildGuard(Option<std::process::Child>);

#[cfg(unix)]
impl ChildGuard {
    fn spawn(directory: &Path, mode: CoordinatedMode) -> Self {
        let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "native_owner::tests::coordinated_quarantine_child_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("MARROW_NATIVE_OWNER_COORDINATED_DIR", directory)
            .env("MARROW_NATIVE_OWNER_COORDINATED_MODE", mode.to_string())
            .spawn()
            .expect("spawn coordinated quarantine child");
        Self(Some(child))
    }

    fn id(&self) -> u32 {
        self.0.as_ref().expect("live child").id()
    }

    fn wait_success(mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let status = self
                .0
                .as_mut()
                .expect("live child")
                .try_wait()
                .expect("poll coordinated child exit");
            if let Some(status) = status {
                self.0.take();
                assert!(status.success(), "coordinated child failed: {status}");
                return;
            }
            if std::time::Instant::now() >= deadline {
                let mut child = self.0.take().expect("live child");
                let _ = child.kill();
                let status = child.wait().expect("reap timed-out coordinated child");
                panic!("coordinated child did not exit before the deadline: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Run one call whose contained panic is the point of the case, with the
/// panic report suppressed.
///
/// A hostile store mutation makes redb's own traversal panic; the adapter
/// contains it and returns the typed error the case asserts. The default
/// hook would still print that panic, and the coordinated child inherits the
/// parent's stderr, so it lands in the workspace test log reading exactly
/// like a real store panic to anything scanning that log.
///
/// Suppression is unconditional across `body`, so an unexpected panic there
/// loses its report too; the typed code each case asserts afterwards is what
/// carries the contract, not the absence of output. The window is the one
/// call and no wider: the hook is restored through a guard, so a panic that
/// escapes `body` cannot leave the process silent for everything after it.
/// The hook is process-global, which is sound here only because the sole
/// caller is the single-threaded child the parent spawns with `--exact`.
#[cfg(unix)]
fn without_panic_report<T>(body: impl FnOnce() -> T) -> T {
    type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send>;

    struct RestoreHook(Option<PanicHook>);

    impl Drop for RestoreHook {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                std::panic::set_hook(previous);
            }
        }
    }

    let _restore = RestoreHook(Some(std::panic::take_hook()));
    std::panic::set_hook(Box::new(|_| {}));
    body()
}

#[cfg(unix)]
fn phase_path(
    directory: &Path,
    mode: CoordinatedMode,
    phase: CoordinatedPhase,
    kind: &str,
) -> PathBuf {
    directory.join(format!(".quarantine-{mode}-{phase}-{kind}"))
}

#[cfg(unix)]
fn wait_for_phase(
    child: &mut ChildGuard,
    directory: &Path,
    mode: CoordinatedMode,
    phase: CoordinatedPhase,
) {
    let ready = phase_path(directory, mode, phase, "ready");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if ready.exists() {
            return;
        }
        if let Some(status) = child
            .0
            .as_mut()
            .expect("live child")
            .try_wait()
            .expect("poll coordinated child")
        {
            panic!("coordinated child exited before {mode}/{phase}: {status}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for coordinated phase {mode}/{phase}",
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn release_phase(directory: &Path, mode: CoordinatedMode, phase: CoordinatedPhase) {
    std::fs::write(phase_path(directory, mode, phase, "release"), b"release")
        .expect("release coordinated phase");
}

#[cfg(unix)]
fn child_barrier(directory: &Path, mode: CoordinatedMode, phase: CoordinatedPhase) {
    std::fs::write(
        phase_path(directory, mode, phase, "ready"),
        phase.to_string(),
    )
    .expect("publish coordinated phase");
    let release = phase_path(directory, mode, phase, "release");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !release.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out awaiting release for {mode}/{phase}",
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn assert_competing_open_is_exactly_lock_refused(
    directory: &Path,
    child_pid: u32,
    phase: CoordinatedPhase,
) {
    match NativeEngineOwner::acquire_existing(directory) {
        Err(NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. })) => {
            assert_eq!(error.code(), Code::StoreLocked, "phase {phase}");
            match error {
                NativeLockError::StoreInUse { owner: Some(owner) } => {
                    assert_eq!(owner.pid, child_pid, "phase {phase} owner pid");
                    assert_eq!(owner.instance, Some([0x71; 16]), "phase {phase} instance");
                }
                NativeLockError::StoreInUse { owner: None } => {
                    panic!("phase {phase} lost the exact owner detail")
                }
                NativeLockError::AccessDenied(_) | NativeLockError::Io(_) => unreachable!(),
            }
        }
        Err(NativeOwnerAcquireError::Lock(
            NativeLockError::AccessDenied(error) | NativeLockError::Io(error),
        )) => {
            panic!("phase {phase} produced lock I/O instead of contention: {error}")
        }
        Err(NativeOwnerAcquireError::Io(error)) => {
            panic!("phase {phase} failed to canonicalize: {error}")
        }
        Ok(_) => panic!("phase {phase} admitted a competing owner"),
    }
}

#[cfg(unix)]
fn seed_audit_body(directory: &Path) {
    let mut owner = open_existing(directory, [0x70; 16]).expect("open audit seed owner");
    let mut txn = owner.begin().expect("begin audit seed transaction");
    for index in 0..64u32 {
        txn.put(format!("k{index:03}").as_bytes(), vec![index as u8; 32])
            .expect("seed audit cell");
    }
    assert_eq!(txn.commit(), CommitOutcome::Confirmed);
}

#[cfg(unix)]
fn corrupt_live_engine_for_audit(directory: &Path) {
    let path = directory.join(NATIVE_ENGINE_FILE);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open live engine for hostile mutation");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).expect("read live engine");
    for offset in (0..bytes.len()).step_by(97) {
        bytes[offset] ^= 0xff;
    }
    file.seek(SeekFrom::Start(0)).expect("rewind live engine");
    file.write_all(&bytes).expect("write hostile mutation");
    file.sync_all().expect("sync hostile mutation");
}

#[cfg(unix)]
#[test]
fn explicit_recovery_audits_even_after_a_clean_shutdown() {
    let scratch = Scratch::new("native-owner-clean-recovery-audit");
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    seed_audit_body(scratch.path());
    assert!(
        marker_bytes(scratch.path()).is_empty(),
        "seed closes cleanly"
    );
    let mut pending = NativeEngineOwner::acquire_existing(scratch.path()).expect("acquire");
    assert!(
        marker_bytes(scratch.path()).is_empty(),
        "no inherited audit obligation"
    );
    let (seam, corrupted) = corrupt_after_open();
    pending.seam = seam;
    let result =
        pending.bind_and_open_existing(NativeOpenAccess::Recovery, [0x70; 16], || Ok::<(), ()>(()));
    assert!(corrupted.get(), "mutation followed successful engine open");
    if let Ok(owner) = result {
        let path = scratch.path().to_path_buf();
        std::mem::forget(owner);
        std::mem::forget(scratch);
        panic!(
            "clean-marker recovery skipped the physical audit; preserve {}",
            path.display()
        );
    }
    let error = result.err().expect("audit refuses");
    assert!(
        matches!(
            error,
            NativeOwnerOpenError::Store(StoreError::Corruption { .. })
        ),
        "{error:?}"
    );
    assert!(
        !marker_bytes(scratch.path()).is_empty(),
        "failed audit retains the obligation"
    );
}

#[cfg(unix)]
fn run_coordinated_quarantine_case(mode: CoordinatedMode) {
    let scratch = Scratch::new(&mode.to_string());
    NativeEngineOwner::provision(scratch.path()).expect("provision");
    if mode == CoordinatedMode::AuditFailure {
        seed_audit_body(scratch.path());
    }
    let pristine =
        std::fs::read(scratch.path().join(NATIVE_ENGINE_FILE)).expect("read pristine engine");
    let mut child = ChildGuard::spawn(scratch.path(), mode);

    wait_for_phase(
        &mut child,
        scratch.path(),
        mode,
        CoordinatedPhase::BeforeRecovery,
    );
    assert_competing_open_is_exactly_lock_refused(
        scratch.path(),
        child.id(),
        CoordinatedPhase::BeforeRecovery,
    );

    let backup = scratch.path().join("store.redb.before-recovery");
    if mode == CoordinatedMode::ReopenFailure {
        std::fs::rename(scratch.path().join(NATIVE_ENGINE_FILE), &backup)
            .expect("remove engine before recovery reopen");
    }
    release_phase(scratch.path(), mode, CoordinatedPhase::BeforeRecovery);

    match mode {
        CoordinatedMode::Success => {
            wait_for_phase(
                &mut child,
                scratch.path(),
                mode,
                CoordinatedPhase::RecoveredLive,
            );
            assert_competing_open_is_exactly_lock_refused(
                scratch.path(),
                child.id(),
                CoordinatedPhase::RecoveredLive,
            );
            release_phase(scratch.path(), mode, CoordinatedPhase::RecoveredLive);

            wait_for_phase(
                &mut child,
                scratch.path(),
                mode,
                CoordinatedPhase::RecoveredDropped,
            );
            assert_competing_open_is_exactly_lock_refused(
                scratch.path(),
                child.id(),
                CoordinatedPhase::RecoveredDropped,
            );
            release_phase(scratch.path(), mode, CoordinatedPhase::RecoveredDropped);
        }
        CoordinatedMode::ReopenFailure => {
            wait_for_phase(
                &mut child,
                scratch.path(),
                mode,
                CoordinatedPhase::ReopenRefused,
            );
            assert_competing_open_is_exactly_lock_refused(
                scratch.path(),
                child.id(),
                CoordinatedPhase::ReopenRefused,
            );
            std::fs::rename(&backup, scratch.path().join(NATIVE_ENGINE_FILE))
                .expect("restore valid engine before child exit");
            release_phase(scratch.path(), mode, CoordinatedPhase::ReopenRefused);
        }
        CoordinatedMode::AuditFailure => {
            wait_for_phase(
                &mut child,
                scratch.path(),
                mode,
                CoordinatedPhase::ReopenedBeforeAudit,
            );
            assert_competing_open_is_exactly_lock_refused(
                scratch.path(),
                child.id(),
                CoordinatedPhase::ReopenedBeforeAudit,
            );
            corrupt_live_engine_for_audit(scratch.path());
            release_phase(scratch.path(), mode, CoordinatedPhase::ReopenedBeforeAudit);

            wait_for_phase(
                &mut child,
                scratch.path(),
                mode,
                CoordinatedPhase::AuditRefused,
            );
            assert_competing_open_is_exactly_lock_refused(
                scratch.path(),
                child.id(),
                CoordinatedPhase::AuditRefused,
            );
            std::fs::write(scratch.path().join(NATIVE_ENGINE_FILE), &pristine)
                .expect("restore valid engine before child exit");
            release_phase(scratch.path(), mode, CoordinatedPhase::AuditRefused);
        }
    }

    child.wait_success();
    open_existing(scratch.path(), [0x73; 16]).expect("process exit is the sole quarantine release");
}

#[cfg(unix)]
#[test]
fn quarantine_is_observed_across_success_and_failed_recovery_phases() {
    for mode in CoordinatedMode::ALL {
        run_coordinated_quarantine_case(mode);
    }
}

#[cfg(unix)]
#[test]
#[ignore = "child-process helper for coordinated quarantine phases"]
fn coordinated_quarantine_child_helper() {
    let Ok(path) = std::env::var("MARROW_NATIVE_OWNER_COORDINATED_DIR") else {
        return;
    };
    let tag = std::env::var("MARROW_NATIVE_OWNER_COORDINATED_MODE").expect("coordinated mode");
    let mode = CoordinatedMode::decode(&tag).expect("the parent names a coordinated mode");
    let directory = Path::new(&path);
    let owner = open_existing(directory, [0x71; 16]).expect("child opens owner");
    child_barrier(directory, mode, CoordinatedPhase::BeforeRecovery);

    match mode {
        CoordinatedMode::Success => {
            let owner = owner
                .reopen_existing_and_audit()
                .expect("successful reopen and audit");
            child_barrier(directory, mode, CoordinatedPhase::RecoveredLive);
            drop(owner);
            child_barrier(directory, mode, CoordinatedPhase::RecoveredDropped);
        }
        CoordinatedMode::ReopenFailure => {
            let error = match owner.reopen_existing_and_audit() {
                Ok(_) => panic!("a missing recovery engine unexpectedly reopened"),
                Err(error) => error,
            };
            assert_eq!(error.code(), Code::StoreIo);
            assert!(
                matches!(
                    error,
                    StoreError::Io {
                        op: StoreOp::Open,
                        ..
                    }
                ),
                "missing recovery must fail in the existing-open phase: {error}",
            );
            child_barrier(directory, mode, CoordinatedPhase::ReopenRefused);
        }
        CoordinatedMode::AuditFailure => {
            let mut owner = owner;
            owner.lock.quarantine();
            drop(owner.engine.take());
            owner.engine = Some(
                NativeEngine::open_existing(&directory.join(NATIVE_ENGINE_FILE))
                    .expect("fresh existing-only reopen before audit"),
            );
            child_barrier(directory, mode, CoordinatedPhase::ReopenedBeforeAudit);
            let error = without_panic_report(|| owner.engine_mut().audit_integrity())
                .expect_err("hostile live mutation must fail the full audit");
            assert_eq!(error.code(), Code::StoreCorruption);
            without_panic_report(|| drop(owner));
            child_barrier(directory, mode, CoordinatedPhase::AuditRefused);
        }
    }
}
