//! Lock-entry node-kind admission, in its own test binary.
//!
//! Planting the one non-regular node an `RDWR | CREATE | NOFOLLOW` open
//! accepts — a FIFO — requires a subprocess, and a `flock` survives in a
//! concurrently spawned child until that child's close-on-exec descriptor
//! closes at `exec`. A sibling test releasing a lock in the same process
//! during that window would observe a spurious `Held`, so this leg holds no
//! company.

mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};

use common::Scratch;
use marrow_fs_journal::{AdmittedDir, CacheLock, EntryName, LockError};

/// A planted non-regular lock entry is refused with its mode untouched: the
/// mode restore runs only after the node is admitted as a regular file, so
/// acquisition never writes a mode onto a node it goes on to reject.
#[test]
fn a_non_regular_lock_entry_is_refused_with_its_mode_untouched() {
    let scratch = Scratch::new("fifo-lock");
    let path = scratch.path().join("lock");
    let planted = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("run mkfifo");
    assert!(planted.success(), "mkfifo planted the non-regular node");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
        .expect("set the planted mode");

    let name = EntryName::admit("lock").expect("test names are admissible");
    let dir = AdmittedDir::admit_trusted_root(scratch.path()).expect("admit the scratch root");
    assert!(matches!(
        CacheLock::acquire(&dir, &name),
        Err(LockError::Custody(_))
    ));
    assert_eq!(
        std::fs::metadata(&path)
            .expect("stat the planted node")
            .mode()
            & 0o7777,
        0o640,
        "the refused node's mode is untouched",
    );
}
