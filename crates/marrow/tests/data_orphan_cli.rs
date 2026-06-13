//! Where a dropped-member or dropped-root drop surfaces. A populated member removed from
//! source with no `evolve retire` intent would orphan its stored cells, and dropping a whole
//! resource would orphan every record under its store; either way the source-attached
//! activation surfaces (`evolve preview`, `evolve apply`, and a plain `marrow run`'s
//! auto-apply) fence it closed and name `evolve retire`, rather than silently dropping the
//! data. An empty member or empty store has nothing to orphan, so the same bare drop is a
//! free no-op there. The read-only `marrow data integrity` surface reports any lingering cell
//! as `data.orphan` regardless. This pins which surface owns the fence and which owns orphan
//! detection, so a future change cannot silently move the boundary or reintroduce the silent
//! drop.

use marrow_check::CheckedSavedPlace;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::marrow;
use support_evolve::{
    RETIRE_BASELINE_SOURCE, commit_catalog, member_catalog_id, native_books_project,
    native_store_path, open_native_store, read_scalar, read_scalar_by_catalog_id, root_place,
    seed_member, seed_title_only, store_epoch,
};

/// The baseline `RETIRE_BASELINE_SOURCE` resource with its `subtitle` member dropped
/// from source — no `evolve retire` intent, no index reads it. The accepted catalog
/// still records `subtitle`, but current source no longer declares it.
const SUBTITLE_DROPPED_SOURCE: &str = "module books\n\
resource Book\n\
\x20   required title: string\n\
store ^books(id: int): Book\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

/// Commit the `title`+`subtitle` baseline, seed both members for one record, then
/// drop `subtitle` from source. Returns the project root, the committed baseline
/// place (still resolves the store id for reads by catalog id after the drop), and
/// the catalog id the dropped member's cells were written under.
fn project_with_orphaned_subtitle(name: &str) -> (support::TempProject, CheckedSavedPlace, String) {
    let root = native_books_project(name, RETIRE_BASELINE_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    let subtitle_id = member_catalog_id(&place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
        seed_member(
            &store,
            &place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
    }
    support::write(&root, "src/books.mw", SUBTITLE_DROPPED_SOURCE);
    (root, place, subtitle_id)
}

/// Commit the `title`+`subtitle` baseline and drop `subtitle` from source without ever
/// seeding any record, so the dropped member holds no stored data to orphan. Returns the
/// project root.
fn project_with_empty_subtitle_drop(name: &str) -> support::TempProject {
    let root = native_books_project(name, RETIRE_BASELINE_SOURCE);
    commit_catalog(&root);
    support::write(&root, "src/books.mw", SUBTITLE_DROPPED_SOURCE);
    root
}

/// A two-resource baseline: `Author` is kept across the drop, `Book` is the root a later
/// source revision deletes whole. Both have a `title` member the seed helpers write through.
const TWO_RESOURCE_BASELINE_SOURCE: &str = "module books\n\
resource Author\n\
\x20   required title: string\n\
store ^authors(id: int): Author\n\
resource Book\n\
\x20   required title: string\n\
store ^bookstore(id: int): Book\n\
pub fn add(title: string): Id(^authors)\n\
\x20   return nextId(^authors)\n";

/// The baseline with the entire `Book` resource — its `resource` block, its `store
/// ^bookstore(...)`, and its members — deleted from source. Only `Author` remains. A zero-arg
/// `show` entry lets a plain `marrow run` trigger the auto-apply window so the fence is
/// exercised on the run surface.
const BOOK_RESOURCE_DROPPED_SOURCE: &str = "module books\n\
resource Author\n\
\x20   required title: string\n\
store ^authors(id: int): Author\n\
pub fn add(title: string): Id(^authors)\n\
\x20   return nextId(^authors)\n\
pub fn show(): string\n\
\x20   return (^authors(1).title ?? \"absent\")\n";

/// Commit the two-resource baseline, seed one populated `Book` record, then drop the whole
/// `Book` resource from source. Returns the project root and the committed `Book` place
/// (still resolves the store id for reads by catalog id after the drop).
fn project_with_orphaned_book_resource(name: &str) -> (support::TempProject, CheckedSavedPlace) {
    let root = native_books_project(name, TWO_RESOURCE_BASELINE_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "bookstore");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    support::write(&root, "src/books.mw", BOOK_RESOURCE_DROPPED_SOURCE);
    (root, place)
}

/// Commit the two-resource baseline and drop the whole `Book` resource without ever seeding a
/// `Book` record, so the dropped store holds no records to orphan. Returns the project root.
fn project_with_empty_book_resource_drop(name: &str) -> support::TempProject {
    let root = native_books_project(name, TWO_RESOURCE_BASELINE_SOURCE);
    commit_catalog(&root);
    support::write(&root, "src/books.mw", BOOK_RESOURCE_DROPPED_SOURCE);
    root
}

fn integrity_codes(value: &serde_json::Value) -> Vec<&str> {
    value["problems"]
        .as_array()
        .expect("problems array")
        .iter()
        .filter_map(|problem| problem["code"].as_str())
        .collect()
}

#[test]
fn data_integrity_reports_a_dropped_member_orphan() {
    // The read-only integrity surface is what catches a lingering dropped-member cell:
    // it walks the actual stored cells, so a cell under a member current source no
    // longer declares is reported as `data.orphan` with exit 1.
    let (root, _place, _subtitle_id) = project_with_orphaned_subtitle("orphan-integrity");

    let output = marrow(&[
        "data",
        "integrity",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = support::json(output.stdout);
    assert!(
        integrity_codes(&value).contains(&"data.orphan"),
        "the dropped member's lingering cell is an orphan: {value:#?}"
    );
}

#[test]
fn evolve_preview_fences_a_populated_bare_drop() {
    // A bare member drop with no retire intent whose cells are populated would orphan that
    // data on a bare activation, so the source-attached preview fails closed and
    // name `evolve retire`. The fence is the guard; the developer must state the destructive
    // intent before the data can be dropped.
    let (root, _place, _subtitle_id) = project_with_orphaned_subtitle("orphan-activation-surfaces");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_value = support::json(preview.stdout);
    assert_eq!(
        preview_value["status"],
        serde_json::json!("blocked"),
        "evolve preview fences the populated bare drop: {preview_value:#?}"
    );
    let blocking = preview_value["blocking"]
        .as_array()
        .expect("blocking array");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("evolve retire"))
        }),
        "the fence names evolve retire: {preview_value:#?}"
    );
}

#[test]
fn an_empty_member_drop_is_a_free_no_op() {
    // The carve-out: when the dropped member holds no stored cells, there is nothing to
    // orphan, so the same bare drop is activatable with no fence. An empty store reshapes
    // freely — the fence guards data loss, not schema shape.
    let root = project_with_empty_subtitle_drop("orphan-empty-noop");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(preview.status.code(), Some(0), "{preview:?}");
    let preview_value = support::json(preview.stdout);
    assert_eq!(
        preview_value["status"],
        serde_json::json!("activatable"),
        "evolve preview treats the empty-member drop as activatable: {preview_value:#?}"
    );
    assert_eq!(
        preview_value["blocking"],
        serde_json::json!([]),
        "no obligation blocks a drop with no data to lose"
    );
}

#[test]
fn a_populated_bare_drop_does_not_apply_without_a_retire_intent() {
    // The lingering cell is durable, not silently dropped: `evolve apply` over the populated
    // bare drop fails closed too, leaving the dropped member's cell exactly as it was. The
    // developer must add `evolve retire` (and approve the drop) before the data is removed,
    // so a `data integrity` repair pass still finds it.
    let (root, place, subtitle_id) = project_with_orphaned_subtitle("orphan-survives-apply");

    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    assert_eq!(
        support::json(apply.stdout)["code"],
        serde_json::json!("evolve.repair_required"),
        "a populated bare drop fails apply closed"
    );

    {
        // Drop the store handle before the integrity subprocess: the native engine
        // takes a single-writer lock, so a held handle would fail the CLI open.
        let store = open_native_store(&root);
        assert_eq!(
            read_scalar_by_catalog_id(&store, &place, 1, &subtitle_id, ScalarType::Str),
            Some(Scalar::Str("Appendix".into())),
            "the dropped member's cell survives the fenced apply"
        );
    }

    let integrity = marrow(&[
        "data",
        "integrity",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    assert!(
        integrity_codes(&support::json(integrity.stdout)).contains(&"data.orphan"),
        "the lingering cell is still an orphan"
    );
}

#[test]
fn evolve_preview_fences_a_populated_whole_resource_drop() {
    // Dropping the whole `Book` resource takes its store with it. Its records would be
    // orphaned under the gone root, so the source-attached preview fails closed
    // and name `evolve retire`, exactly as a populated member drop does.
    let (root, _place) = project_with_orphaned_book_resource("orphan-resource-surfaces");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_value = support::json(preview.stdout);
    assert_eq!(
        preview_value["status"],
        serde_json::json!("blocked"),
        "evolve preview fences the populated whole-resource drop: {preview_value:#?}"
    );
    let blocking = preview_value["blocking"]
        .as_array()
        .expect("blocking array");
    // One fence per dropped root, naming it and pointing at retire.
    let drop_fences: Vec<_> = blocking
        .iter()
        .filter(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["message"].as_str().is_some_and(|message| {
                    message.contains("evolve retire") && message.contains("^bookstore")
                })
        })
        .collect();
    assert_eq!(
        drop_fences.len(),
        1,
        "exactly one fence names the dropped root: {preview_value:#?}"
    );
}

#[test]
fn an_empty_whole_resource_drop_is_a_free_no_op() {
    // The carve-out: when the dropped resource's store holds no records, there is nothing to
    // orphan, so dropping the whole root is activatable with no fence.
    let root = project_with_empty_book_resource_drop("orphan-resource-empty-noop");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(preview.status.code(), Some(0), "{preview:?}");
    let preview_value = support::json(preview.stdout);
    assert_eq!(
        preview_value["status"],
        serde_json::json!("activatable"),
        "evolve preview treats the empty whole-resource drop as activatable: {preview_value:#?}"
    );
    assert_eq!(
        preview_value["blocking"],
        serde_json::json!([]),
        "no obligation blocks a whole-root drop with no data to lose"
    );
}

#[test]
fn a_populated_whole_resource_drop_does_not_apply_or_run_without_a_retire_intent() {
    // The dropped store's records are durable, not silently dropped: `evolve apply` fails
    // closed, a plain `marrow run`'s auto-apply fences on schema drift rather than orphaning
    // the records, the epoch never advances, and the records survive for a repair pass.
    let (root, place) = project_with_orphaned_book_resource("orphan-resource-survives");
    let baseline_epoch = store_epoch(&root).expect("baseline epoch");

    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    assert_eq!(
        support::json(apply.stdout)["code"],
        serde_json::json!("evolve.repair_required"),
        "a populated whole-resource drop fails apply closed"
    );

    let run = marrow(&["run", "--entry", "books::show", root.to_str().unwrap()]);
    assert_eq!(
        run.status.code(),
        Some(1),
        "a populated whole-resource drop must fence the run, not silently auto-apply: {run:?}"
    );
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift rather than dropping the records: {stderr}"
    );

    assert_eq!(
        store_epoch(&root),
        Some(baseline_epoch),
        "a fenced whole-resource drop does not advance the epoch"
    );
    {
        // Drop the store handle before the integrity subprocess: the native engine takes a
        // single-writer lock, so a held handle would fail the CLI open.
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar(&store, &place, 1, "title", ScalarType::Str),
            Some(Scalar::Str("Dune".into())),
            "the dropped store's record survives the fenced apply and run"
        );
    }

    let integrity = marrow(&[
        "data",
        "integrity",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    assert!(
        integrity_codes(&support::json(integrity.stdout)).contains(&"data.orphan"),
        "the dropped store's lingering records are orphans"
    );
}
