//! Cooperative cache-lock custody: exclusive acquisition, typed contention,
//! release on drop, and lock-entry persistence. Affinity (no clone, no copy)
//! is enforced by the `compile_fail` doctest on [`marrow_fs_journal::CacheLock`].

mod common;

use common::Scratch;
use marrow_fs_journal::{AdmittedDir, CacheLock, EntryName, LockError, NodeKind};

fn name(spelling: &str) -> EntryName {
    EntryName::admit(spelling).expect("test names are admissible")
}

fn root(scratch: &Scratch) -> AdmittedDir {
    AdmittedDir::admit_trusted_root(scratch.path()).expect("admit the scratch root")
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

#[test]
fn a_symlink_lock_entry_is_refused() {
    let scratch = Scratch::new("symlink-lock");
    let dir = root(&scratch);
    std::os::unix::fs::symlink("elsewhere", scratch.path().join("lock")).expect("create symlink");
    assert!(matches!(
        CacheLock::acquire(&dir, &name("lock")),
        Err(LockError::Custody(_))
    ));
}
