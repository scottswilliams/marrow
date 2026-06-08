//! Tier-2 end-to-end scenario over the `marrow` binary: the practical multi-step
//! language-database cycle a developer actually runs — check, run to save data under
//! the original schema, add a new sparse field in source, discharge it through
//! `evolve preview`/`evolve apply`, then re-run so old records read back intact and a
//! fresh write carries the new field.
//!
//! Oracles are typed: process exit codes, parsed `data get --format json` presence and
//! decoded value bytes, and the structured `evolve preview` JSON witness — never a
//! substring of human-rendered prose. A separate `#[ignore]`d test records a durability
//! divergence the scenario surfaced: a bare `marrow run` after a source-only sparse-field
//! add fails closed on the activation fence rather than running as the docs' "source
//! change only" table row implies, so the evolve cycle above is the real repair path.

use std::fs;

mod support;

use support::{TempProject, marrow, marrow_sub, write};

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
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n";

/// The evolved schema: a sparse `subtitle` is added. `seed` still writes only `title`
/// (the old records' shape), and `annotate` writes the new field on a fresh record so
/// the test can prove new writes carry it.
const EVOLVED_SOURCE: &str = "module books\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     pub fn annotate()\n\
     \x20   transaction\n\
     \x20       ^books(2).title = \"Reaper Man\"\n\
     \x20       ^books(2).subtitle = \"a Discworld novel\"\n";

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

    // Step 2: add a sparse field in source. A populated store still has to re-stamp the
    // changed durable shape, so the cycle is preview -> apply, not a bare run.
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
     resource Author at ^authors(id: int)\n\
     \x20   name: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^authors(1).name = \"Ada\"\n";

const REFERENCE_EVOLVED: &str = "module lib\n\
     resource Author at ^authors(id: int)\n\
     \x20   name: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     \x20   authorId: Id(^authors)\n\
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
#[ignore = "DIVERGENCE: docs/data-evolution.md says a sparse-field add is a 'source change only' \
            with existing records staying valid, but a bare `marrow run` over a populated store \
            after the add fails closed with run.schema_drift -- the changed durable shape must be \
            re-stamped through `evolve apply` (or the store must be empty) before the program can run."]
fn a_bare_run_after_a_source_only_sparse_add_is_fenced_by_schema_drift() {
    // Repro of the divergence the working scenario routes around. Run under the original
    // schema to stamp the store, add a sparse field in source, then run again WITHOUT an
    // evolve cycle. The docs' change table calls a sparse add "source change only", which
    // implies the re-run should proceed and read the field as absent; instead the
    // activation fence rejects it because the source digest binds the resource's member
    // shape and now differs from the one the store recorded at the same epoch.
    let root = books_project("scenario-sparse-bare-run");
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");

    write(&root, "src/books.mw", EVOLVED_SOURCE);
    let rerun = marrow_sub("run", &[dir(&root)]);

    // The documented contract for a sparse add is "existing records stay valid", so this
    // bare re-run should succeed (exit 0) and leave the old record readable. It does not:
    // the run is fenced before execution.
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "a source-only sparse add should not fence a re-run",
    );
    let stderr = String::from_utf8(rerun.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.schema_drift"),
        "a sparse add must not read as schema drift: {stderr}",
    );

    fs::remove_dir_all(root.path()).ok();
}
