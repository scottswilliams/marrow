//! Node-kind admission over a planted FIFO, in its own test binary.
//!
//! Planting the one non-regular node an `RDWR | CREATE | NOFOLLOW` open
//! accepts — a FIFO — requires a subprocess, and a `flock` survives in a
//! concurrently spawned child until that child's close-on-exec descriptor
//! closes at `exec`. A sibling test releasing a lock in the same process
//! during that window would observe a spurious `Held`, so this leg holds no
//! company. Every test in this crate that plants a FIFO under a custody open
//! belongs here for that reason. The window is per-process, so a test binary in
//! another crate that plants its own FIFO shares none of it.

mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::mpsc;
use std::time::Duration;

use common::Scratch;
use marrow_fs_journal::{AdmittedDir, CacheLock, CustodyError, EntryName, LockError, NodeKind};

/// A planted non-regular lock entry is refused as the wrong node kind, with
/// its mode untouched. The classification is asserted as its exact typed
/// variant, because `flock` classifies no node kind: on Darwin it refuses a
/// FIFO with the unsupported-semantics errno this crate reads as
/// [`CustodyError::Unsupported`], so an acquisition that locked before it
/// classified would name the platform's lock semantics rather than the planted
/// node. The mode restore runs only after the node is admitted as a regular
/// file, so a node refused for its kind keeps the mode it carried.
#[test]
fn a_non_regular_lock_entry_is_refused_as_the_wrong_node_kind_with_its_mode_untouched() {
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
    assert!(
        matches!(
            CacheLock::acquire(&dir, &name),
            Err(LockError::Custody(CustodyError::WrongNodeKind {
                op: "lock",
                found: NodeKind::Other,
            }))
        ),
        "a planted FIFO is refused as the wrong node kind, not as unsupported \
         platform semantics",
    );
    assert_eq!(
        std::fs::metadata(&path)
            .expect("stat the planted node")
            .mode()
            & 0o7777,
        0o640,
        "the refused node's mode is untouched",
    );
}

/// How long an entry open of a planted FIFO is given before the open is read as
/// blocking. The refusal is a handful of syscalls; a second is a margin no
/// scheduler needs.
const ENTRY_OPEN_BUDGET: Duration = Duration::from_secs(1);

/// One custody open of an existing entry, reduced to its refusal so both opens
/// share this file's budgeted driver.
type EntryOpen = fn(&AdmittedDir, &EntryName) -> Result<(), CustodyError>;

/// Every custody open of an existing entry refuses a planted FIFO for its node
/// kind instead of blocking on it.
///
/// `O_RDONLY` on a FIFO with no writer blocks until a writer arrives, so the
/// read-only classification open would, without `O_NONBLOCK`, never reach the
/// node-kind refusal at all: the caller would hang, indefinitely and under the
/// write lock, on a node it was about to refuse. The read-write open reaches the
/// refusal on its own flags, because an `O_RDWR` open of a FIFO waits for
/// nobody — but that is a property of the platform's FIFO semantics rather than
/// of this crate's code, so it is asserted here under the same budget rather
/// than assumed. Both legs together are what make this file the evidence for
/// every open a planted FIFO can reach; `open_lock_file` is covered by the
/// sibling test above.
///
/// Each open runs on its own thread over its own planted node and is awaited
/// with a budget, so an open that does block fails this test loudly rather than
/// hanging the binary. A blocked thread is unblockable by construction, so it is
/// left parked and the process exit collects it; a failing run is already
/// reporting the defect. The nodes are separate because a later leg opening a
/// shared FIFO for writing would release an earlier leg blocked on it and hide
/// exactly the defect this budget exists to catch.
#[test]
fn every_entry_open_refuses_a_fifo_rather_than_blocking_on_it() {
    for (tag, open) in [
        ("read-only", open_readonly as EntryOpen),
        ("read-write", open_read_write as EntryOpen),
    ] {
        let scratch = Scratch::new(&format!("fifo-{tag}"));
        let path = scratch.path().join("entry");
        let planted = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("run mkfifo");
        assert!(planted.success(), "mkfifo planted the non-regular node");

        let root = scratch.path().to_path_buf();
        let (report, opened) = mpsc::channel();
        std::thread::spawn(move || {
            let dir = AdmittedDir::admit_trusted_root(&root).expect("admit the scratch root");
            let name = EntryName::admit("entry").expect("test names are admissible");
            report.send(open(&dir, &name)).ok();
        });

        let outcome = opened.recv_timeout(ENTRY_OPEN_BUDGET).unwrap_or_else(|_| {
            panic!(
                "the {tag} open of the FIFO at {} did not return within {ENTRY_OPEN_BUDGET:?}, \
                 so it is blocking on a node it must refuse",
                path.display()
            )
        });
        assert!(
            matches!(
                outcome,
                Err(CustodyError::WrongNodeKind {
                    op: "open file",
                    found: NodeKind::Other,
                })
            ),
            "the {tag} open of a FIFO reported {outcome:?} rather than the wrong-node-kind refusal",
        );
    }
}

fn open_readonly(dir: &AdmittedDir, name: &EntryName) -> Result<(), CustodyError> {
    dir.open_file_readonly(name).map(|_| ())
}

fn open_read_write(dir: &AdmittedDir, name: &EntryName) -> Result<(), CustodyError> {
    dir.open_file(name).map(|_| ())
}
