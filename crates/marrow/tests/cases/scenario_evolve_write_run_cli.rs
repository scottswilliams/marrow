//! Tier-2 end-to-end scenario over the `marrow` binary: the practical multi-step
//! language-database cycle a developer actually runs — check, run to save data under
//! the original schema, add a new sparse field in source, discharge it through
//! `evolve preview`/`evolve apply`, then re-run so old records read back intact and a
//! fresh write carries the new field.
//!
//! Oracles are typed: process exit codes, parsed `data get --format json` presence and
//! decoded value bytes, the structured `evolve preview` JSON witness, and the accepted
//! catalog epoch the store snapshot publishes as the local crash bridge — never a
//! substring of human-rendered prose.
use crate::support;
use marrow_store::{AccessMode, SealedStore};
use support::{TempProject, marrow, marrow_sub, write};

/// The accepted catalog epoch a project has committed, read from its store snapshot. A
/// run that auto-applies an evolution advances this exactly as an explicit
/// `evolve apply` does, so the epoch is the typed oracle for "the activation advanced".
fn accepted_epoch(root: &TempProject) -> u64 {
    let path = root.join(".data").join("marrow.redb");
    let store = SealedStore::open(&path, AccessMode::Read)
        .expect("open store read-only")
        .into_store();
    store
        .read_catalog_snapshot()
        .expect("read catalog snapshot")
        .expect("project has an accepted catalog")
        .epoch
}

/// A native-store project whose default entry seeds one book under the original schema.
/// Built uncommitted so the first `run` freezes the catalog transparently, exactly as a
/// developer's first run would.
fn books_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "books::seed" } }"#,
        );
        write(root, "src/books.mw", BASELINE_SOURCE);
    })
}

/// The original schema: a `Book` with only `title`. `seed` writes one record.
const BASELINE_SOURCE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n";

/// The evolved schema: a sparse `subtitle` is added. `seed` still writes only `title`
/// (the old records' shape), and `annotate` writes the new field on a fresh record so
/// the test can prove new writes carry it.
const EVOLVED_SOURCE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     store ^books(id: int): Book\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     pub fn annotate()\n\
     \x20   transaction\n\
     \x20       ^books(2).title = \"Reaper Man\"\n\
     \x20       ^books(2).subtitle = \"a Discworld novel\"\n";

/// The baseline evolved to add a sparse `pages` AND an `evolve default` for it in the
/// same edit. The default targets the newly added sparse field, whose cells are absent on
/// the old record; discharging it mutates nothing. `seed` still writes only `title`.
const SPARSE_WITH_DEFAULT_SOURCE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   pages: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   default Book.pages = 0\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n";

fn dir(root: &TempProject) -> &str {
    root.to_str().expect("project path utf8")
}

/// Decode the value bytes a `data get --format json` result carries, or `None` when the
/// path is absent. The presence field is the typed absence oracle; the bytes are the
/// stored value.
fn get_value(root: &TempProject, path: &str) -> Option<Vec<u8>> {
    let output = marrow(&["data", "get", "--format", "json", dir(root), path]);
    assert_eq!(output.status.code(), Some(0), "data get {path}: {output:?}");
    let record = support::json(output.stdout);
    match record["presence"].as_str() {
        Some("absent") => {
            assert_eq!(record["value_b64"], serde_json::Value::Null);
            None
        }
        Some(_) => Some(
            marrow_run::base64::decode(record["value_b64"].as_str().expect("value_b64"))
                .expect("decode value bytes"),
        ),
        None => panic!("data get presence: {record}"),
    }
}

#[test]
fn add_a_sparse_field_through_the_evolve_cycle_keeps_old_records_and_carries_new_writes() {
    let root = books_project("scenario-evolve-cycle");

    // Step 1: the first run checks the project, freezes the catalog, and saves a record
    // under the original single-field schema.
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    assert_eq!(
        get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "the original schema's record is saved",
    );

    // Step 2: add a sparse field in source and discharge it through the explicit
    // preview -> apply cycle. A bare run would auto-apply the same zero-record change;
    // the explicit path stays valid and is what this scenario exercises.
    write(&root, "src/books.mw", EVOLVED_SOURCE);
    let preview = marrow(&["evolve", "preview", "--format", "json", dir(&root)]);
    assert_eq!(preview.status.code(), Some(0), "preview: {preview:?}");
    let witness = support::json(preview.stdout);
    assert_eq!(
        witness["status"], "activatable",
        "a sparse add is activatable with no obligation: {witness}"
    );

    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "apply: {apply:?}");

    // Step 3: the old record survives the evolution untouched, and the new sparse field
    // reads as absent on it (the sparse contract: absent, not an empty default).
    assert_eq!(
        get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "the pre-evolution record keeps its title after the evolve cycle",
    );
    assert_eq!(
        get_value(&root, "^books(1).subtitle"),
        None,
        "the newly added sparse field is absent on the old record",
    );

    // Step 4: re-run past the evolution. The default `seed` runs again with no fence
    // (the store now sits at the evolved shape), and a fresh `annotate` write carries
    // the new field.
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(rerun.status.code(), Some(0), "re-run seed: {rerun:?}");
    let annotate = marrow_sub("run", &["--entry", "books::annotate", dir(&root)]);
    assert_eq!(annotate.status.code(), Some(0), "annotate: {annotate:?}");
    assert_eq!(
        get_value(&root, "^books(2).title"),
        Some(b"Reaper Man".to_vec()),
    );
    assert_eq!(
        get_value(&root, "^books(2).subtitle"),
        Some(b"a Discworld novel".to_vec()),
        "a write past the evolution carries the new field",
    );
}

#[test]
fn a_sparse_add_with_a_same_block_default_applies_and_runs_without_wedging() {
    // Adding a sparse field and an `evolve default` for it in one edit is intrinsically
    // additive: the default targets the new field's absent cells and discharges mutating
    // nothing. `evolve apply` must succeed, `marrow run` must execute the entry rather than
    // loop on schema drift, and the store must stay healthy with the sparse cell absent.
    let root = books_project("scenario-sparse-same-block-default");
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    assert_eq!(get_value(&root, "^books(1).title"), Some(b"Mort".to_vec()));
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/books.mw", SPARSE_WITH_DEFAULT_SOURCE);
    let preview = marrow(&["evolve", "preview", "--format", "json", dir(&root)]);
    assert_eq!(preview.status.code(), Some(0), "preview: {preview:?}");
    let witness = support::json(preview.stdout);
    assert_eq!(
        witness["status"], "activatable",
        "a sparse add with its default is activatable with no obligation: {witness}"
    );

    // Apply must be a clean no-op, never a store-corruption fault over the absent sparse
    // cell, and must advance the accepted epoch.
    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(
        apply.status.code(),
        Some(0),
        "apply must not raise store corruption over an absent sparse cell: {apply:?}"
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "apply advanced the accepted catalog epoch by exactly one",
    );

    // The old record survives untouched and the new sparse field reads as absent, not the
    // default value: an unpopulated sparse cell is absent, not zero.
    assert_eq!(get_value(&root, "^books(1).title"), Some(b"Mort".to_vec()));
    assert_eq!(
        get_value(&root, "^books(1).pages"),
        None,
        "the sparse field stays absent on the old record; the default backfilled nothing",
    );

    // The store is healthy: integrity reports no problems.
    let integrity = marrow(&["data", "integrity", "--format", "json", dir(&root)]);
    assert_eq!(integrity.status.code(), Some(0), "integrity: {integrity:?}");
    assert_eq!(
        support::json(integrity.stdout)["problems"],
        serde_json::json!([]),
        "the store stays healthy across the sparse-default apply",
    );

    // A plain `marrow run` against the evolved source executes the entry rather than
    // looping on `run.schema_drift`, and the write lands.
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "run must execute the entry, not wedge on schema drift: {rerun:?}",
    );
    assert_eq!(get_value(&root, "^books(1).title"), Some(b"Mort".to_vec()));
    assert_eq!(
        get_value(&root, "^books(1).pages"),
        None,
        "the sparse field is still absent after the re-run",
    );
}

/// An identity-reference project: a `Book` and a separate `Author` store. The baseline
/// seeds one of each with no reference; the evolved schema adds a sparse `Id(^authors)`
/// reference to `Book` and links the existing book to the author, printing whether the
/// stored reference reads back as the same identity.
fn reference_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "lib::seed" } }"#,
        );
        write(root, "src/lib.mw", REFERENCE_BASELINE);
    })
}

const REFERENCE_BASELINE: &str = "module lib\n\
     resource Author\n\
     \x20   name: string\n\
     store ^authors(id: int): Author\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^authors(1).name = \"Ada\"\n";

const REFERENCE_EVOLVED: &str = "module lib\n\
     resource Author\n\
     \x20   name: string\n\
     store ^authors(id: int): Author\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   authorId: Id(^authors)\n\
     store ^books(id: int): Book\n\
     pub fn seed()\n\
     \x20   print(\"noop\")\n\
     pub fn link()\n\
     \x20   for author in keys(^authors)\n\
     \x20       transaction\n\
     \x20           ^books(1).authorId = author\n\
     \x20       return\n\
     pub fn linkedToFirstAuthor()\n\
     \x20   for author in keys(^authors)\n\
     \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
     \x20       print($\"linked={stored == author}\")\n\
     \x20       return\n";

#[test]
fn an_identity_reference_added_by_evolution_links_an_existing_record_and_round_trips() {
    // Scenario 2 through the full binary: a `Book` with saved records gains a sparse
    // `Id(^authors)` reference under an evolve cycle, then a fresh write links the
    // pre-existing book to an existing author and the stored reference reads back as the
    // same identity. The CLI path discharges the apply through the binary that publishes
    // the accepted catalog, so the runtime identity write binds the new member's id.
    let root = reference_project("scenario-evolve-identity");

    // Seed a book and an author under the original (reference-free) schema.
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    assert_eq!(get_value(&root, "^books(1).title"), Some(b"Mort".to_vec()),);

    // Add the sparse reference field and discharge it. A sparse add is activatable with
    // no obligation, so apply advances the epoch without touching the old record's data.
    write(&root, "src/lib.mw", REFERENCE_EVOLVED);
    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "apply: {apply:?}");
    assert_eq!(
        get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "the old record keeps its title across the reference-field evolution",
    );
    assert_eq!(
        get_value(&root, "^books(1).authorId"),
        None,
        "the reference is absent on the old record until it is linked",
    );

    // Link the existing book to the existing author, then prove the stored reference
    // reads back as that author's identity.
    let link = marrow_sub("run", &["--entry", "lib::link", dir(&root)]);
    assert_eq!(link.status.code(), Some(0), "link: {link:?}");
    let check = marrow_sub("run", &["--entry", "lib::linkedToFirstAuthor", dir(&root)]);
    assert_eq!(check.status.code(), Some(0), "round-trip read: {check:?}");
    let stdout = String::from_utf8(check.stdout).expect("stdout utf8");
    assert_eq!(
        stdout, "linked=true\n",
        "the stored reference equals the author identity it was written from",
    );
    // The reference is now present on the previously reference-free record.
    assert!(
        get_value(&root, "^books(1).authorId").is_some(),
        "the linked reference is stored on the old record",
    );
}

#[test]
fn a_bare_run_after_a_sparse_add_auto_applies() {
    // Seed a record under the original schema, then add a sparse field in source and run
    // again WITHOUT an explicit evolve cycle. Adding a sparse field mutates zero stored
    // records, so the run auto-applies the evolution under the apply lock: it advances the
    // accepted epoch by one, stamps the new shape, and proceeds. The old record reads back
    // intact and the new sparse field is absent on it.
    let root = books_project("scenario-sparse-bare-run");
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/books.mw", EVOLVED_SOURCE);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "a sparse add auto-applies on run: {rerun:?}",
    );

    assert_eq!(
        get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "the pre-evolution record keeps its title across the auto-apply",
    );
    assert_eq!(
        get_value(&root, "^books(1).subtitle"),
        None,
        "the newly added sparse field is absent on the old record",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "the auto-apply advanced the accepted catalog epoch by exactly one",
    );
}
