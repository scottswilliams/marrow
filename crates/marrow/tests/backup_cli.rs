use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use marrow_check::checked_saved_root_place;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

mod support;

use support::{TempProject, marrow, member_catalog_id, temp_project, write};

/// The `code` field of a single JSON error record printed to stdout.
fn json_code(output: &Output) -> String {
    support::json(output.stdout.clone())["code"]
        .as_str()
        .expect("json error code")
        .to_string()
}

/// A native-store project whose `seed` entry writes one book, plus its committed
/// catalog. Running `seed` populates the store; the data directory can then be
/// removed to model an empty restore target with the same source and catalog.
fn seeded_project(name: &str) -> (TempProject, PathBuf) {
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

fn evolution_default_project(name: &str) -> (TempProject, PathBuf) {
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
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n",
        );
    });
    let data_dir = root.join(".data");
    let seed = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    (root, data_dir)
}

fn add_pages_default_evolution(root: impl AsRef<Path>) {
    write(
        root.as_ref(),
        "src/shelf.mw",
        "module shelf\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20required pages: int\n\
         \n\
         evolve\n\
         \x20\x20\x20\x20default Book.pages = 0\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).pages = 0\n",
    );
}

fn dump(root: impl AsRef<Path>) -> String {
    let out = marrow(&["data", "dump", root.as_ref().to_str().unwrap()]);
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
    assert_eq!(after, before, "restored data matches the original");
}

#[test]
fn restore_crash_window_backup_then_evolve_resume_completes() {
    let (root, data_dir) = evolution_default_project("backup-crash-window");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("crash-window.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");

    add_pages_default_evolution(&root);
    let first = marrow(&["evolve", "apply", &dir]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "first evolve apply: {first:?}"
    );

    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind catalog");
    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    let restore = marrow(&["restore", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    let resume = marrow(&["evolve", "apply", &dir]);
    assert_eq!(resume.status.code(), Some(0), "resume: {resume:?}");
    let stdout = String::from_utf8(resume.stdout).expect("resume stdout utf8");
    assert!(
        stdout.contains("completed evolution"),
        "resume publishes the restored proposal: {stdout}"
    );

    let after = dump(&root);
    assert!(
        after.contains("Mort") && after.contains('0'),
        "restored data survives resume: {after}"
    );
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
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.not_empty");
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
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");
}

/// A native-store project with both a non-unique and a unique index over a keyed
/// root, plus a `seed` entry that writes several books and lookups that read through
/// each index. Running `seed` populates the data and the maintained indexes.
fn indexed_project(name: &str) -> (TempProject, PathBuf) {
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
             \x20\x20\x20\x20shelf: string\n\
             \x20\x20\x20\x20isbn: string\n\
             \n\
             \x20\x20\x20\x20index byShelf(shelf, id)\n\
             \x20\x20\x20\x20index byIsbn(isbn) unique\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).shelf = \"fiction\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).isbn = \"978-1\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).title = \"Reaper\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).shelf = \"fiction\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).isbn = \"978-2\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).title = \"Sourcery\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).shelf = \"history\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).isbn = \"978-3\"\n\
             \n\
             pub fn find_isbn()\n\
             \x20\x20\x20\x20for id in ^books.byIsbn(\"978-2\")\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print(^books(id).title)\n\
             \n\
             pub fn count_shelf()\n\
             \x20\x20\x20\x20var c = 0\n\
             \x20\x20\x20\x20for id in keys(^books.byShelf(\"fiction\"))\n\
             \x20\x20\x20\x20\x20\x20\x20\x20c = c + 1\n\
             \x20\x20\x20\x20print($\"{c}\")\n",
        );
    });
    let data_dir = root.join(".data");
    let seed = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    (root, data_dir)
}

/// A backup carries data only; restore rebuilds the generated indexes from the
/// restored records. After a backup, an emptied store, and a restore, both a unique
/// lookup and a non-unique `keys` traversal resolve the rebuilt entries.
#[test]
fn restore_rebuilds_indexes_usable_through_lookups() {
    let (root, data_dir) = indexed_project("backup-index-rebuild");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    // Remove the entire store data directory, then restore from the archive.
    fs::remove_dir_all(&data_dir).expect("remove store data");
    let restore = marrow(&["restore", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    // The unique index resolves the looked-up record by isbn.
    let unique = marrow(&["run", "--entry", "shelf::find_isbn", &dir]);
    let unique_out = String::from_utf8_lossy(&unique.stdout).to_string();
    // The non-unique index resolves both fiction books.
    let count = marrow(&["run", "--entry", "shelf::count_shelf", &dir]);
    let count_out = String::from_utf8_lossy(&count.stdout).to_string();

    assert_eq!(unique.status.code(), Some(0), "find_isbn run: {unique:?}");
    assert!(
        unique_out.contains("Reaper"),
        "rebuilt unique index resolves the book: {unique_out}"
    );
    assert_eq!(count.status.code(), Some(0), "count_shelf run: {count:?}");
    assert!(
        count_out.contains('2'),
        "rebuilt non-unique index resolves both fiction books: {count_out}"
    );
}

fn checked_books_place(root: impl AsRef<Path>) -> marrow_check::CheckedSavedPlace {
    let root = root.as_ref();
    support::commit_catalog_if_clean(root);
    let config_text = fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let (report, program) = marrow_check::check_project(root, &config).expect("check project");
    assert!(!report.has_errors(), "fixture checks cleanly: {report:#?}");
    checked_saved_root_place(&program, "books", marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

/// The real store catalog id of `^books`, read through the production check path so
/// an orphan cell can be written under the live store but an undeclared member.
fn store_catalog_id(root: impl AsRef<Path>) -> CatalogId {
    let place = checked_books_place(root);
    CatalogId::new(place.store_catalog_id.expect("accepted store catalog id"))
        .expect("store catalog id")
}

/// A backup that carries an orphan data cell is not valid under the target
/// source/catalog, so restore rejects it before committing anything to the empty
/// target. Orphans are compiler/data-integrity facts, not faithful debris restore
/// may activate.
#[test]
fn restore_rejects_a_backup_carrying_an_orphan_cell() {
    let (root, data_dir) = seeded_project("backup-orphan");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    // Write a data cell under the live store but an undeclared member catalog id: a
    // dropped field left this behind. It is a data-family cell, so the backup copies it.
    let store_catalog = store_catalog_id(&root);
    {
        let store =
            TreeStore::open(&data_dir.join("marrow.redb")).expect("open native store for orphan");
        store
            .write_data_value(
                &store_catalog,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new("cat_000000000000000000000000cafef00d".to_string())
                        .expect("orphan member id"),
                )],
                b"left-behind".to_vec(),
            )
            .expect("write orphan cell");
    }

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    let roots_after_failed_restore = marrow(&["data", "roots", &dir]);
    assert_eq!(
        restore.status.code(),
        Some(1),
        "restore rejects a backup with orphan debris: {restore:?}"
    );
    assert_eq!(json_code(&restore), "restore.data_invalid");
    assert!(
        String::from_utf8_lossy(&roots_after_failed_restore.stdout).contains("(no saved data)"),
        "failed restore leaves the target empty: {roots_after_failed_restore:?}"
    );
}

#[test]
fn restore_rejects_a_backup_carrying_an_impossible_data_cell_shape() {
    let (root, data_dir) = seeded_project("backup-impossible-cell-shape");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let place = checked_books_place(&root);
    let store_catalog = CatalogId::new(
        place
            .store_catalog_id
            .clone()
            .expect("accepted store catalog id"),
    )
    .expect("store catalog id");
    let title_catalog = member_catalog_id(&place.root_members, "title");
    {
        let store = TreeStore::open(&data_dir.join("marrow.redb"))
            .expect("open native store for impossible cell");
        store
            .write_data_value(
                &store_catalog,
                &[SavedKey::Int(1)],
                &[
                    DataPathSegment::Member(title_catalog),
                    DataPathSegment::Key(SavedKey::Int(99)),
                ],
                b"impossible".to_vec(),
            )
            .expect("write impossible cell shape");
    }

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    let roots_after_failed_restore = marrow(&["data", "roots", &dir]);
    assert_eq!(
        restore.status.code(),
        Some(1),
        "restore rejects a backup with an impossible data cell shape: {restore:?}"
    );
    assert_eq!(json_code(&restore), "restore.data_invalid");
    assert!(
        String::from_utf8_lossy(&roots_after_failed_restore.stdout).contains("(no saved data)"),
        "failed restore leaves the target empty: {roots_after_failed_restore:?}"
    );
}

/// A faithful backup ends exactly at its last cell. Appending bytes makes the file
/// no longer the backup the manifest describes, so restore rejects it.
#[test]
fn restore_rejects_trailing_bytes() {
    let (root, data_dir) = seeded_project("backup-trailing");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );

    // Append stray bytes after the cell stream, leaving the header and checksum intact.
    let mut bytes = fs::read(&archive).expect("read archive");
    bytes.extend_from_slice(b"trailing");
    fs::write(&archive, &bytes).expect("write archive with trailing bytes");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");
}

/// `nextId(^books)` allocates from the highest stored record id, which lives in the
/// data the backup carries. After a round-trip into an emptied store, the next id
/// continues from the same value the original store would have allocated.
#[test]
fn restore_continues_next_id_from_the_restored_data() {
    let root = temp_project("backup-next-id", |root| {
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
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"a\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).title = \"b\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).title = \"c\"\n\
             \n\
             pub fn peek_next()\n\
             \x20\x20\x20\x20print($\"{nextId(^books)}\")\n",
        );
    });
    let data_dir = root.join(".data");
    let dir = root.to_str().unwrap().to_string();
    assert_eq!(marrow(&["run", &dir]).status.code(), Some(0), "seed");

    // The highest stored id is 3, so nextId is 4 before any round-trip.
    let before = marrow(&["run", "--entry", "shelf::peek_next", &dir]);
    assert!(
        String::from_utf8_lossy(&before.stdout).contains('4'),
        "nextId before restore is 4: {before:?}"
    );

    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0),
        "backup"
    );
    fs::remove_dir_all(&data_dir).expect("remove store data");
    assert_eq!(
        marrow(&["restore", &dir, &archive_arg]).status.code(),
        Some(0),
        "restore"
    );

    // After restore, nextId continues from the restored data: still 4.
    let after = marrow(&["run", "--entry", "shelf::peek_next", &dir]);
    assert!(
        String::from_utf8_lossy(&after.stdout).contains('4'),
        "nextId after restore continues from the restored data: {after:?}"
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
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");
    assert!(
        String::from_utf8_lossy(&restore.stdout).contains("restored 0 record(s)"),
        "an empty backup restores zero records: {restore:?}"
    );
}
