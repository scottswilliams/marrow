use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    support::commit_catalog_if_clean(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

/// A native-store project whose `seed` entry writes one book, plus its committed
/// catalog. Running `seed` populates the store; the data directory can then be
/// removed to model an empty restore target with the same source and catalog.
fn seeded_project(name: &str) -> (PathBuf, PathBuf) {
    let root = temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20required title: string\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20var b: Book\n\
             \x20\x20\x20\x20b.title = \"Mort\"\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1) = b\n",
        );
    });
    let data_dir = root.join(".data");
    let seed = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    (root, data_dir)
}

fn dump(root: &Path) -> String {
    let out = marrow(&["data", "dump", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "data dump: {out:?}");
    String::from_utf8(out.stdout).expect("dump utf8")
}

#[test]
fn backup_then_restore_round_trips_saved_data() {
    let (root, data_dir) = seeded_project("backup-roundtrip");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let before = dump(&root);
    assert!(before.contains("Mort"), "seed wrote a book: {before}");

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");
    assert!(archive.exists(), "backup wrote the archive file");

    // Empty the store: same source and catalog, no saved data.
    fs::remove_dir_all(&data_dir).expect("remove store data");
    let roots = marrow(&["data", "roots", &dir]);
    assert!(
        String::from_utf8_lossy(&roots.stdout).contains("(no saved data)"),
        "store is empty before restore: {roots:?}"
    );

    let restore = marrow(&["restore", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    let after = dump(&root);
    fs::remove_dir_all(&root).ok();
    assert_eq!(after, before, "restored data matches the original");
}

#[test]
fn restore_refuses_a_non_empty_target() {
    let (root, _data_dir) = seeded_project("backup-not-empty");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );
    // The store still holds the seeded data, so restore must refuse it.
    let restore = marrow(&["restore", &dir, &archive_arg]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert!(
        String::from_utf8_lossy(&restore.stderr).contains("restore.not_empty"),
        "non-empty restore reports restore.not_empty: {restore:?}"
    );
}

#[test]
fn restore_rejects_a_corrupt_backup() {
    let (root, data_dir) = seeded_project("backup-corrupt");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );

    // Flip the final byte of the cell stream, leaving the header intact, so the
    // data checksum no longer matches.
    let mut bytes = fs::read(&archive).expect("read archive");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&archive, &bytes).expect("write corrupt archive");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    let restore = marrow(&["restore", &dir, &archive_arg]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert!(
        String::from_utf8_lossy(&restore.stderr).contains("restore.corrupt_chunk"),
        "a corrupt backup reports restore.corrupt_chunk: {restore:?}"
    );
}

#[test]
fn backup_of_an_unseeded_project_restores_empty() {
    let root = temp_project("backup-empty", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\nresource Book at ^books(id: int)\n\x20\x20\x20\x20required title: string\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("empty.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    let restore = marrow(&["restore", &dir, &archive_arg]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");
    assert!(
        String::from_utf8_lossy(&restore.stdout).contains("restored 0 record(s)"),
        "an empty backup restores zero records: {restore:?}"
    );
}
