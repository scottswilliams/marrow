//! Descriptor-rooted custody conformance: admission flags, exclusive
//! creation, destination-refusing link, exchange and no-replace rename
//! semantics, bounded reads, and typed refusals. This suite runs on every
//! qualified platform; the Linux leg additionally pins the backend in the
//! dependency-condition suite.

mod common;

use std::os::unix::fs::MetadataExt;

use common::Scratch;
use marrow_fs_journal::{AdmittedDir, CustodyError, EntryName, NodeKind};

fn name(spelling: &str) -> EntryName {
    EntryName::admit(spelling).expect("test names are admissible")
}

fn root(scratch: &Scratch) -> AdmittedDir {
    AdmittedDir::admit_trusted_root(scratch.path()).expect("admit the scratch root")
}

#[test]
fn a_directory_root_is_admitted_with_its_identity() {
    let scratch = Scratch::new("root");
    let dir = root(&scratch);
    let metadata = std::fs::metadata(scratch.path()).expect("stat the scratch root");
    assert_eq!(dir.identity().dev(), metadata.dev());
    assert_eq!(dir.identity().ino(), metadata.ino());
}

#[test]
fn a_missing_root_is_a_typed_refusal() {
    let scratch = Scratch::new("missing-root");
    let missing = scratch.path().join("absent");
    assert!(matches!(
        AdmittedDir::admit_trusted_root(&missing),
        Err(CustodyError::NotFound { .. })
    ));
}

#[test]
fn a_regular_file_root_is_a_typed_refusal() {
    let scratch = Scratch::new("file-root");
    let file = scratch.path().join("plain");
    std::fs::write(&file, b"x").expect("write plain file");
    assert!(matches!(
        AdmittedDir::admit_trusted_root(&file),
        Err(CustodyError::NotADirectory { .. })
    ));
}

#[test]
fn a_symlink_root_is_refused_not_followed() {
    let scratch = Scratch::new("symlink-root");
    let real = scratch.path().join("real");
    std::fs::create_dir(&real).expect("create real dir");
    let alias = scratch.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).expect("create symlink");
    assert!(matches!(
        AdmittedDir::admit_trusted_root(&alias),
        Err(CustodyError::SymlinkRefused { .. })
    ));
}

#[test]
fn child_admission_is_descriptor_relative_and_nofollow() {
    let scratch = Scratch::new("child");
    let dir = root(&scratch);

    let child = dir.create_child_dir(&name("inner")).expect("create child");
    let metadata = std::fs::metadata(scratch.path().join("inner")).expect("stat child");
    assert_eq!(child.identity().ino(), metadata.ino());

    let again = dir.admit_child(&name("inner")).expect("admit child");
    assert_eq!(again.identity(), child.identity());

    assert!(matches!(
        dir.admit_child(&name("absent")),
        Err(CustodyError::NotFound { .. })
    ));

    std::fs::write(scratch.path().join("plain"), b"x").expect("write plain file");
    assert!(matches!(
        dir.admit_child(&name("plain")),
        Err(CustodyError::NotADirectory { .. })
    ));

    std::os::unix::fs::symlink("inner", scratch.path().join("alias")).expect("create symlink");
    assert!(matches!(
        dir.admit_child(&name("alias")),
        Err(CustodyError::SymlinkRefused { .. })
    ));
}

#[test]
fn exclusive_creation_witnesses_mode_and_refuses_a_second_creation() {
    let scratch = Scratch::new("excl");
    let dir = root(&scratch);

    let file = dir.create_file_excl(&name("fresh")).expect("create fresh");
    let stat = file.stat().expect("stat fresh");
    assert_eq!(stat.kind(), NodeKind::Regular);
    assert_eq!(stat.nlink(), 1);
    assert_eq!(stat.size(), 0);
    assert_eq!(stat.mode(), 0o600, "creation is mode 0600 exactly");
    assert_eq!(stat.identity(), file.identity());

    let entry = dir
        .stat_entry(&name("fresh"))
        .expect("stat entry")
        .expect("fresh exists");
    assert_eq!(entry.identity(), file.identity());

    assert!(matches!(
        dir.create_file_excl(&name("fresh")),
        Err(CustodyError::AlreadyExists { .. })
    ));
}

#[test]
fn open_file_is_nofollow_and_typed() {
    let scratch = Scratch::new("open");
    let dir = root(&scratch);

    drop(dir.create_file_excl(&name("data")).expect("create data"));
    let opened = dir.open_file(&name("data")).expect("open data");
    let entry = dir
        .stat_entry(&name("data"))
        .expect("stat entry")
        .expect("data exists");
    assert_eq!(opened.identity(), entry.identity());

    assert!(matches!(
        dir.open_file(&name("absent")),
        Err(CustodyError::NotFound { .. })
    ));

    std::os::unix::fs::symlink("data", scratch.path().join("alias")).expect("create symlink");
    assert!(matches!(
        dir.open_file(&name("alias")),
        Err(CustodyError::SymlinkRefused { .. })
    ));

    std::fs::create_dir(scratch.path().join("subdir")).expect("create subdir");
    assert!(matches!(
        dir.open_file(&name("subdir")),
        Err(CustodyError::WrongNodeKind {
            found: NodeKind::Directory,
            ..
        })
    ));
}

#[test]
fn appends_and_bounded_reads_round_trip() {
    let scratch = Scratch::new("append");
    let dir = root(&scratch);

    let mut file = dir.create_file_excl(&name("log")).expect("create log");
    file.append(b"hello ").expect("first append");
    file.append(b"journal").expect("second append");
    file.sync().expect("sync file");

    assert_eq!(
        file.read_prefix(64).expect("bounded read"),
        b"hello journal"
    );
    assert_eq!(file.read_prefix(5).expect("bounded read"), b"hello");
    assert_eq!(file.stat().expect("stat").size(), 13);

    assert_eq!(
        std::fs::read(scratch.path().join("log")).expect("independent read"),
        b"hello journal"
    );
}

#[test]
fn links_refuse_existing_destinations() {
    let scratch = Scratch::new("link");
    let dir = root(&scratch);

    let file = dir.create_file_excl(&name("one")).expect("create one");
    dir.link(&name("one"), &name("two")).expect("link one→two");
    assert_eq!(file.stat().expect("stat").nlink(), 2);
    let two = dir
        .stat_entry(&name("two"))
        .expect("stat two")
        .expect("two exists");
    assert_eq!(two.identity(), file.identity());

    drop(dir.create_file_excl(&name("three")).expect("create three"));
    assert!(matches!(
        dir.link(&name("three"), &name("two")),
        Err(CustodyError::AlreadyExists { .. })
    ));
    assert!(matches!(
        dir.link(&name("absent"), &name("four")),
        Err(CustodyError::NotFound { .. })
    ));
}

#[test]
fn unlink_and_stat_agree_on_absence() {
    let scratch = Scratch::new("unlink");
    let dir = root(&scratch);

    drop(dir.create_file_excl(&name("gone")).expect("create gone"));
    assert!(dir.stat_entry(&name("gone")).expect("stat").is_some());
    dir.unlink(&name("gone")).expect("unlink gone");
    dir.sync().expect("sync dir");
    assert!(dir.stat_entry(&name("gone")).expect("stat").is_none());

    assert!(matches!(
        dir.unlink(&name("gone")),
        Err(CustodyError::NotFound { .. })
    ));
}

#[test]
fn stat_entry_reports_symlinks_without_following() {
    let scratch = Scratch::new("stat-symlink");
    let dir = root(&scratch);
    std::os::unix::fs::symlink("anywhere", scratch.path().join("sym")).expect("create symlink");
    let stat = dir
        .stat_entry(&name("sym"))
        .expect("stat symlink")
        .expect("symlink exists");
    assert_eq!(stat.kind(), NodeKind::Symlink);
}

/// The per-OS EXCHANGE conformance fixture. On Darwin this exercises
/// `renameatx_np(RENAME_SWAP)` through the libc backend; on Linux it
/// exercises `renameat2(RENAME_EXCHANGE)` through the `linux_raw` backend.
#[test]
fn exchange_swaps_two_entries_atomically() {
    let scratch = Scratch::new("exchange");
    let dir = root(&scratch);

    let a = dir.create_file_excl(&name("a")).expect("create a");
    let b = dir.create_file_excl(&name("b")).expect("create b");
    std::fs::write(scratch.path().join("a"), b"alpha").expect("fill a");
    std::fs::write(scratch.path().join("b"), b"beta").expect("fill b");

    dir.exchange(&name("a"), &name("b")).expect("exchange");
    dir.sync().expect("sync dir");

    let a_now = dir
        .stat_entry(&name("a"))
        .expect("stat a")
        .expect("a exists");
    let b_now = dir
        .stat_entry(&name("b"))
        .expect("stat b")
        .expect("b exists");
    assert_eq!(a_now.identity(), b.identity(), "a now names b's inode");
    assert_eq!(b_now.identity(), a.identity(), "b now names a's inode");
    assert_eq!(
        std::fs::read(scratch.path().join("a")).expect("read a"),
        b"beta"
    );
    assert_eq!(
        std::fs::read(scratch.path().join("b")).expect("read b"),
        b"alpha"
    );
}

#[test]
fn exchange_with_a_missing_entry_is_typed() {
    let scratch = Scratch::new("exchange-missing");
    let dir = root(&scratch);
    drop(dir.create_file_excl(&name("present")).expect("create"));
    assert!(matches!(
        dir.exchange(&name("present"), &name("absent")),
        Err(CustodyError::NotFound { .. })
    ));
}

/// The per-OS NOREPLACE conformance fixture. On Darwin this exercises
/// `renameatx_np(RENAME_EXCL)`; on Linux, `renameat2(RENAME_NOREPLACE)`.
#[test]
fn rename_noreplace_moves_and_refuses_existing_destinations() {
    let scratch = Scratch::new("noreplace");
    let dir = root(&scratch);

    let file = dir.create_file_excl(&name("from")).expect("create from");
    dir.rename_noreplace(&name("from"), &name("to"))
        .expect("rename noreplace");
    assert!(dir.stat_entry(&name("from")).expect("stat").is_none());
    let to = dir
        .stat_entry(&name("to"))
        .expect("stat to")
        .expect("to exists");
    assert_eq!(to.identity(), file.identity());

    drop(dir.create_file_excl(&name("from")).expect("recreate from"));
    assert!(matches!(
        dir.rename_noreplace(&name("from"), &name("to")),
        Err(CustodyError::AlreadyExists { .. })
    ));
    // The refusal replaced nothing: both entries retain their inodes.
    assert_eq!(
        dir.stat_entry(&name("to"))
            .expect("stat to")
            .expect("to exists")
            .identity(),
        file.identity()
    );
}

#[test]
fn directory_sync_succeeds_on_an_admitted_directory() {
    let scratch = Scratch::new("dir-sync");
    let dir = root(&scratch);
    dir.sync().expect("sync the admitted directory");
}

/// A directory whose owner bits fall short of what admitting one needs is
/// refused with the mode-repair reading, not a bare filesystem error.
///
/// The state needs no hostile actor. `mkdirat` asks for `0700` and the process
/// umask masks it; the exact mode is restored on the admitted descriptor, so a
/// umask withholding owner read makes the admission between those two steps
/// fail, and a crash in the same window leaves the masked directory on disk for
/// the next run to meet. An operator told only that an operation refused cannot
/// act on it; one told the mode found and the mode required can.
#[test]
fn a_mode_masked_directory_is_refused_with_the_mode_repair_reading() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("dir-mode-masked");
    let dir = root(&scratch);
    let child = name("masked");
    dir.create_child_dir(&child).expect("create the child");

    let path = scratch.path().join("masked");
    // Exactly what a umask withholding owner read leaves behind.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o300))
        .expect("mask the child's mode");
    let binds = AdmittedDir::admit_trusted_root(&path).is_err();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .expect("restore for the assertion below");
    assert!(
        binds,
        "mode 0300 on a directory did not refuse admission, so this kat never ran. Run the \
         suite as a process the mode bits bind, on a filesystem that carries them."
    );

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o300))
        .expect("mask the child's mode again");
    let refusal = dir
        .admit_child(&child)
        .expect_err("a directory this process cannot read is not admitted");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .expect("restore so the scratch root can be removed");

    assert!(
        matches!(
            refusal,
            CustodyError::ModeDenied {
                found: 0o300,
                required: 0o500,
                ..
            }
        ),
        "a mode-masked directory must name the mode found and the mode required: {refusal:?}"
    );
}
