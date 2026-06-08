//! `marrow check` is read-only: it neither freezes durable identity into the
//! accepted catalog file nor creates or mutates the saved-data store. The durable
//! write paths — `marrow run` over a persistent store and `marrow evolve apply` —
//! are the contrast: each commits, so each leaves the catalog and store changed.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use marrow_store::tree::TreeStore;

mod support;
mod support_evolve;

use support::{marrow, native_config, temp_project_uncommitted, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, accepted_catalog,
    commit_catalog, native_books_project, native_store_path, open_native_store, root_place,
    seed_title_only, store_epoch,
};

/// The canonical native-store seed source: a `Counter` resource whose `seed`
/// transaction writes one record. Declared inline here rather than reused from the
/// runtime corpus because this suite needs a `module`-bearing source file.
const COUNTER_SOURCE: &str = "module app\n\
     \n\
     resource Counter at ^counter(id: int)\n\
     \x20\x20\x20\x20required value: int\n\
     \n\
     pub fn seed()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 42\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n";

fn catalog_path(root: &Path) -> std::path::PathBuf {
    root.join("marrow.catalog.json")
}

fn store_path(root: &Path) -> std::path::PathBuf {
    root.join(".data").join("marrow.redb")
}

#[test]
fn check_on_an_uncommitted_project_writes_no_catalog_and_no_store() {
    // A project whose durable identity is not yet frozen checks cleanly and reports
    // informationally, but `check` must not be the command that establishes durable
    // state: it leaves the catalog file and the store absent.
    let project = temp_project_uncommitted("check-ro-uncommitted", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();

    assert!(!catalog_path(&project).exists(), "no catalog before check");
    assert!(!store_path(&project).exists(), "no store before check");

    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert!(
        !catalog_path(&project).exists(),
        "check must not freeze durable identity into the catalog file"
    );
    assert!(
        !store_path(&project).exists(),
        "check must not create the saved-data store"
    );
}

#[test]
fn run_freezes_the_catalog_and_creates_the_store() {
    // The contrast for the uncommitted case: `run` over a persistent store is a durable
    // write path, so the same project that `check` left untouched gains a frozen catalog
    // and a created store the first time it runs.
    let project = temp_project_uncommitted("check-ro-run-commits", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();

    let run = marrow(&["run", "--entry", "app::seed", dir]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");

    assert!(
        catalog_path(&project).exists(),
        "run freezes the proposed identity into the catalog file"
    );
    assert!(
        store_path(&project).exists(),
        "run creates the saved-data store and commits the seeded record"
    );
}

#[test]
fn check_on_a_committed_project_modifies_neither_catalog_nor_store() {
    // Once durable state exists, `check` still touches nothing: the catalog file's
    // bytes and modification time and the store file's bytes are identical before and
    // after, so a CI `check` cannot drift a committed project.
    let project = temp_project_uncommitted("check-ro-committed", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );

    let catalog = catalog_path(&project);
    let store = store_path(&project);
    let catalog_before = fs::read(&catalog).expect("read catalog");
    let catalog_mtime_before = mtime(&catalog);
    let store_before = fs::read(&store).expect("read store");

    // Sleep past the filesystem mtime resolution so a rewrite would register.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert_eq!(
        fs::read(&catalog).expect("read catalog"),
        catalog_before,
        "check left the catalog file bytes unchanged"
    );
    assert_eq!(
        mtime(&catalog),
        catalog_mtime_before,
        "check did not rewrite the catalog file"
    );
    assert_eq!(
        fs::read(&store).expect("read store"),
        store_before,
        "check left the store file bytes unchanged"
    );
}

#[test]
fn evolve_apply_advances_the_committed_catalog_and_store() {
    // The contrast for the committed case: `evolve apply` is the durable write path that
    // a check must not be. It advances the accepted catalog epoch and stamps the store,
    // so the two surfaces are not interchangeable.
    let root = native_books_project("check-ro-evolve-apply", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    // The seeded store holds records but carries no activation stamp yet. A plain
    // source-only check passes and leaves it unstamped: check is not the surface that
    // stamps a store.
    assert_eq!(
        marrow(&["check", root.to_str().unwrap()]).status.code(),
        Some(0)
    );
    assert_eq!(
        store_epoch(&root),
        None,
        "check did not stamp the store epoch"
    );

    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    assert_eq!(
        accepted_catalog(&root).epoch,
        baseline_epoch + 1,
        "apply advanced the accepted catalog epoch"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        store.read_catalog_epoch().expect("store epoch"),
        Some(baseline_epoch + 1),
        "apply stamped the store with the new epoch"
    );
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .expect("file modification time")
}
