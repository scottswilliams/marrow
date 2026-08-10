//! Cooperative cache-lock custody: exclusive acquisition, typed contention,
//! release on drop, and lock-entry persistence. Affinity (no clone, no copy)
//! is enforced by the `compile_fail` doctest on [`marrow_fs_journal::CacheLock`].

mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use common::Scratch;
use marrow_fs_journal::{AdmittedDir, CacheLock, CustodyError, EntryName, LockError, NodeKind};

fn name(spelling: &str) -> EntryName {
    EntryName::admit(spelling).expect("test names are admissible")
}

fn root(scratch: &Scratch) -> AdmittedDir {
    AdmittedDir::admit_trusted_root(scratch.path()).expect("admit the scratch root")
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set mode");
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat entry").mode() & 0o7777
}

/// Whether permission bits actually deny this process the access a mode
/// withholds. A process holding the override capability, or a filesystem that
/// carries no mode bits, opens a mode-`0000` entry anyway.
fn mode_bits_deny_this_process(scratch: &Scratch) -> bool {
    let probe = scratch.path().join("deny-probe");
    std::fs::write(&probe, b"").expect("plant the probe");
    set_mode(&probe, 0o000);
    let denied = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&probe)
        .is_err();
    set_mode(&probe, 0o600);
    std::fs::remove_file(&probe).expect("remove the probe");
    denied
}

#[test]
fn acquisition_creates_the_entry_and_witnesses_its_identity() {
    let scratch = Scratch::new("acquire");
    let dir = root(&scratch);

    let lock = CacheLock::acquire(&dir, &name("lock")).expect("acquire");
    let entry = dir
        .stat_entry(&name("lock"))
        .expect("stat lock entry")
        .expect("the lock entry exists");
    assert_eq!(entry.identity(), lock.identity());
    assert_eq!(entry.kind(), NodeKind::Regular);
    assert_eq!(entry.mode(), 0o600);
}

#[test]
fn a_held_lock_refuses_a_second_holder() {
    let scratch = Scratch::new("contend");
    let dir = root(&scratch);

    let held = CacheLock::acquire(&dir, &name("lock")).expect("first acquire");
    assert!(matches!(
        CacheLock::acquire(&dir, &name("lock")),
        Err(LockError::Held)
    ));
    drop(held);

    // Dropping the affine holder is the only release.
    let reacquired = CacheLock::acquire(&dir, &name("lock")).expect("reacquire after drop");
    drop(reacquired);
}

#[test]
fn the_lock_entry_persists_across_holders() {
    let scratch = Scratch::new("persist");
    let dir = root(&scratch);

    let first = CacheLock::acquire(&dir, &name("lock")).expect("first acquire");
    let identity = first.identity();
    drop(first);

    assert!(
        dir.stat_entry(&name("lock"))
            .expect("stat lock entry")
            .is_some(),
        "release keeps the lock entry in place",
    );
    let second = CacheLock::acquire(&dir, &name("lock")).expect("second acquire");
    assert_eq!(
        second.identity(),
        identity,
        "the second holder locks the same persistent inode",
    );
}

#[test]
fn distinct_names_lock_independently() {
    let scratch = Scratch::new("independent");
    let dir = root(&scratch);

    let _first = CacheLock::acquire(&dir, &name("one")).expect("lock one");
    let _second = CacheLock::acquire(&dir, &name("two")).expect("lock two");
}

/// A lock entry whose owner bits a crash inside the create-then-restore window
/// left stripped can be opened by nobody, so acquisition names the observed
/// mode and the mode an operator must restore instead of returning an
/// unclassified I/O error. The planted modes are what the two owner-stripping
/// umasks leave: `0277` leaves `0400`, `0477` leaves `0200`, and `0677` leaves
/// `0000`. Restoring the named mode is the whole of the operator action, so
/// the acquisition that follows it succeeds.
#[test]
fn a_mode_stripped_lock_entry_names_the_operator_action() {
    let scratch = Scratch::new("mode-stripped-lock");
    if !mode_bits_deny_this_process(&scratch) {
        // A process holding the override capability, or a filesystem that
        // ignores mode bits, opens the planted entry regardless; the reading
        // itself is pinned by the crate's own classification test.
        return;
    }
    let dir = root(&scratch);
    let path = scratch.path().join("lock");

    for stripped in [0o400, 0o200, 0o000] {
        std::fs::write(&path, b"").expect("plant the lock entry");
        set_mode(&path, stripped);

        match CacheLock::acquire(&dir, &name("lock")) {
            Err(LockError::Custody(CustodyError::ModeDenied {
                op,
                found,
                required,
            })) => assert_eq!(
                (op, found, required),
                ("open lock", stripped, 0o600),
                "the refusal names the observed mode and the mode to restore",
            ),
            other => panic!("mode {stripped:o}: expected the mode refusal, found {other:?}"),
        }
        assert_eq!(
            mode_of(&path),
            stripped,
            "the refusal writes no mode of its own",
        );

        set_mode(&path, 0o600);
        drop(CacheLock::acquire(&dir, &name("lock")).expect("acquire after the operator restore"));
        std::fs::remove_file(&path).expect("remove the planted entry");
    }
}

/// A symbolic link at the lock name is refused by the `NOFOLLOW` open itself,
/// before any node-kind classification, so the exact typed refusal is the
/// symlink one rather than a wrong-node-kind reading of the link's target.
#[test]
fn a_symlink_lock_entry_is_refused_as_a_symlink() {
    let scratch = Scratch::new("symlink-lock");
    let dir = root(&scratch);
    std::os::unix::fs::symlink("elsewhere", scratch.path().join("lock")).expect("create symlink");
    assert!(matches!(
        CacheLock::acquire(&dir, &name("lock")),
        Err(LockError::Custody(CustodyError::SymlinkRefused {
            op: "open lock"
        }))
    ));
}
