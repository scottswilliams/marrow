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

use crate::support;
use crate::support_evolve;
use marrow_check::CheckedSavedPlace;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};
use support::marrow;
use support_evolve::{
    RETIRE_BASELINE_SOURCE, commit_catalog, member_catalog_id, native_books_project,
    native_store_path, open_native_store, read_scalar, read_scalar_by_catalog_id, root_place,
    seed_member, seed_title_only, store_catalog_id, store_epoch,
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

/// The same bare `subtitle` drop with a zero-arg default entry. The entry writes a
/// visible marker if execution gets past activation, so a fenced run can prove it did
/// not execute user code.
const SUBTITLE_DROPPED_RUN_SOURCE: &str = "module books\n\
resource Book\n\
\x20   required title: string\n\
store ^books(id: int): Book\n\
pub fn main()\n\
\x20   transaction\n\
\x20       ^books(2).title = \"entry-ran\"\n";

/// Commit the `title`+`subtitle` baseline, seed both members for one record, then
/// drop `subtitle` from source. Returns the project root, the committed baseline
/// place (still resolves the store id for reads by catalog id after the drop), and
/// the catalog id the dropped member's cells were written under.
fn project_with_orphaned_subtitle(name: &str) -> (support::TempProject, CheckedSavedPlace, String) {
    let root = native_books_project(name, RETIRE_BASELINE_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books").expect("books root place");
    let subtitle_id = member_catalog_id(&place, "subtitle").expect("subtitle catalog id");
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

/// Commit and seed the `title`+`subtitle` baseline, then drop `subtitle` from source
/// with a default entry configured for plain `marrow run`.
fn project_with_orphaned_subtitle_run(
    name: &str,
) -> (support::TempProject, CheckedSavedPlace, String) {
    let (root, place, subtitle_id) = project_with_orphaned_subtitle(name);
    support::write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "books::main" } }"#,
    );
    support::write(&root, "src/books.mw", SUBTITLE_DROPPED_RUN_SOURCE);
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
    let place = root_place(&program, "bookstore").expect("bookstore root place");
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

/// The baseline with its whole store renamed from `^books` to `^archive`. The `Book` resource and
/// its members stay, but every cell written under `^books` is now under a store the current source
/// no longer declares, so the source-driven inspection sees no saved roots at all.
const STORE_RENAMED_SOURCE: &str = "module books\n\
resource Book\n\
\x20   required title: string\n\
store ^archive(id: int): Book\n\
pub fn add(title: string): Id(^archive)\n\
\x20   return nextId(^archive)\n";

/// A store-root-only removal: `Book` and its member model remain declared, while the
/// `^books` root is gone. `^log` keeps the project durable and gives a default entry a
/// visible side effect if activation lets it execute.
const STORE_REMOVED_SOURCE: &str = "module books\n\
resource Log\n\
\x20   note: string\n\
store ^log(id: int): Log\n\
resource Book\n\
\x20   required title: string\n\
pub fn main()\n\
\x20   transaction\n\
\x20       ^log(1).note = \"entry-ran\"\n";

const STORE_RENAMED_RUN_SOURCE: &str = "module books\n\
resource Log\n\
\x20   note: string\n\
store ^log(id: int): Log\n\
resource Book\n\
\x20   required title: string\n\
store ^archive(id: int): Book\n\
pub fn main()\n\
\x20   transaction\n\
\x20       ^log(1).note = \"entry-ran\"\n";

const STORE_ONLY_BASELINE_SOURCE: &str = "module books\n\
resource Log\n\
\x20   note: string\n\
store ^log(id: int): Log\n\
resource Book\n\
\x20   required title: string\n\
store ^books(id: int): Book\n\
pub fn main()\n\
\x20   transaction\n\
\x20       ^log(1).note = \"entry-ran\"\n";

/// Commit the baseline, seed one populated record under `^books`, then rename the whole store to
/// `^archive` in source. Every seeded cell is now orphaned under the gone store id, and the current
/// source declares only the empty `^archive`. Returns the project root.
fn project_with_renamed_store(name: &str) -> support::TempProject {
    let root = native_books_project(name, RETIRE_BASELINE_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books").expect("books root place");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    support::write(&root, "src/books.mw", STORE_RENAMED_SOURCE);
    root
}

fn project_with_store_only_change(
    name: &str,
    target_source: &str,
    populate_books: bool,
) -> (support::TempProject, CheckedSavedPlace, CheckedSavedPlace) {
    let root = native_books_project(name, STORE_ONLY_BASELINE_SOURCE);
    support::write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "books::main" } }"#,
    );
    let program = commit_catalog(&root);
    let books = root_place(&program, "books").expect("books root place");
    let log = root_place(&program, "log").expect("log root place");
    if populate_books {
        let store = open_native_store(&root);
        seed_title_only(&store, &books, 1, "Dune");
    }
    support::write(&root, "src/books.mw", target_source);
    (root, books, log)
}

fn integrity_codes(value: &serde_json::Value) -> Vec<&str> {
    value["problems"]
        .as_array()
        .expect("problems array")
        .iter()
        .filter_map(|problem| problem["code"].as_str())
        .collect()
}

// The repair fence's remediation prose — the in-source evolve block framing, the
// scaffold pointer, the `marrow evolve apply` command, and the absence of a bogus
// `evolve retire` subcommand — is one shared rendering, pinned once per subject
// shape by the `evolve_repair_required_*` goldens. Every other site asserts the
// typed `evolve.repair_required` code and, where the fence keys on a root, the
// stable catalog id.
fn assert_repair_required(value: &serde_json::Value, context: &str) {
    assert_eq!(
        value["code"],
        serde_json::json!("evolve.repair_required"),
        "{context} fails closed with the repair fence: {value:#?}"
    );
}

fn assert_run_fences_on_schema_drift(stderr: &[u8], context: &str) {
    let fault = support::parse_result_line(&support::last_fault(stderr));
    assert_eq!(
        fault.code, "run.schema_drift",
        "{context} fences the run on schema drift"
    );
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
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = support::json(output.stdout);
    assert!(
        integrity_codes(&value).contains(&"data.orphan"),
        "the dropped member's lingering cell is an orphan: {value:#?}"
    );
}

#[test]
fn data_stats_warns_when_a_drifted_source_hides_orphan_cells() {
    // `marrow data stats` renders the store through the current source-derived schema view, so a
    // cell under a member current source no longer declares is not counted. Counting it silently
    // would under-report intact data with no signal, so stats emits the same orphan advisory
    // `data integrity` reports — a count of cells under undeclared members — on stderr while still
    // exiting 0, because the data is physically intact and stats is a read-only inspection.
    let (root, _place, _subtitle_id) = project_with_orphaned_subtitle("orphan-stats-advisory");

    let output = marrow(&["data", "stats", root.to_str().expect("project path utf-8")]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("data.orphan"),
        "stats surfaces the hidden orphan cell as an advisory: {stderr}"
    );
    assert!(
        stderr.contains('1'),
        "the advisory names the count of hidden cells: {stderr}"
    );
}

#[test]
fn data_dump_warns_when_a_drifted_source_hides_orphan_cells() {
    // `marrow data dump` walks only the source-declared places, so a dropped member's cell is
    // omitted from the dump. The same orphan advisory keeps the reduced output from being silent.
    let (root, _place, _subtitle_id) = project_with_orphaned_subtitle("orphan-dump-advisory");

    let output = marrow(&["data", "dump", root.to_str().expect("project path utf-8")]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("data.orphan"),
        "dump surfaces the hidden orphan cell as an advisory: {stderr}"
    );
}

#[test]
fn data_roots_warns_when_a_whole_store_rename_hides_orphan_cells() {
    // After a whole-store rename, `marrow data roots` walks only the source-declared stores, so the
    // renamed-away store's records vanish and roots prints `(no saved data)`. Without the advisory a
    // developer would read that as an empty store and not as drifted source hiding intact records.
    // The same orphan advisory `data integrity` reports keeps the empty listing from being silent.
    let root = project_with_renamed_store("orphan-roots-rename-text");

    let output = marrow(&["data", "roots", root.to_str().expect("project path utf-8")]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // The empty-roots view and the advisory line — its hidden-cell count and its
    // `marrow data integrity` pointer — are render contracts, pinned here once.
    support::assert_matches_golden(
        &String::from_utf8(output.stdout).expect("stdout utf8"),
        "data_roots_no_saved_data.txt",
    );
    let advisory = support::last_fault(&output.stderr);
    assert_eq!(
        support::parse_result_line(&advisory).code,
        "data.orphan",
        "roots surfaces the hidden orphan cells as an advisory"
    );
    support::assert_matches_golden(&advisory, "data_orphan_advisory_hidden_cells.txt");
}

#[test]
fn data_roots_json_warns_when_a_whole_store_rename_hides_orphan_cells() {
    // The advisory is a stderr note, so the stdout JSON stays the clean roots envelope while the
    // orphan signal still reaches a machine reader on stderr.
    let root = project_with_renamed_store("orphan-roots-rename-json");

    let output = marrow(&[
        "data",
        "roots",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        support::json(output.stdout)["roots"],
        serde_json::json!([]),
        "the renamed-away store leaves no source-visible roots"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("data.orphan"),
        "roots --format json surfaces the hidden orphan cells as an advisory: {stderr}"
    );
}

#[test]
fn data_get_warns_when_a_whole_store_rename_hides_orphan_cells() {
    // `marrow data get` over a path whose store the source still declares is unaffected, but after a
    // whole-store rename a read against the empty renamed store is silently absent while populated
    // cells linger under the gone store id. The orphan advisory keeps that absence from misleading.
    let root = project_with_renamed_store("orphan-get-rename-text");

    let output = marrow(&[
        "data",
        "get",
        root.to_str().expect("project path utf-8"),
        "^archive(1).title",
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        support::parse_result_line(&support::last_fault(&output.stderr)).code,
        "data.orphan",
        "get surfaces the hidden orphan cells as an advisory: {output:?}"
    );
}

#[test]
fn data_get_json_warns_when_a_whole_store_rename_hides_orphan_cells() {
    // The advisory reaches a machine reader on stderr while stdout stays the clean get envelope.
    let root = project_with_renamed_store("orphan-get-rename-json");

    let output = marrow(&[
        "data",
        "get",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
        "^archive(1).title",
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("data.orphan"),
        "get --format json surfaces the hidden orphan cells as an advisory: {stderr}"
    );
}

#[test]
fn data_stats_is_silent_when_no_orphan_cells_exist() {
    // The advisory fires only when source has drifted over intact data. A project whose source
    // still declares every stored member has nothing hidden, so stats stays clean on stderr.
    let root = native_books_project("orphan-stats-clean", RETIRE_BASELINE_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books").expect("books root place");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["data", "stats", root.to_str().expect("project path utf-8")]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("data.orphan"),
        "no orphan advisory when source declares every stored member: {stderr}"
    );
}

#[test]
fn evolve_preview_fences_a_populated_bare_drop() {
    // A bare member drop with no retire intent whose cells are populated would orphan that
    // data on a bare activation, so the source-attached preview fails closed and
    // names `evolve retire`. The fence is the guard; the developer must state the destructive
    // intent before the data can be dropped.
    let (root, _place, subtitle_id) = project_with_orphaned_subtitle("orphan-activation-surfaces");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
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
    let fence = blocking
        .iter()
        .find(|report| report["code"] == serde_json::json!("evolve.repair_required"))
        .unwrap_or_else(|| panic!("a repair_required fence: {preview_value:#?}"));
    assert_eq!(
        fence["data"]["catalog_id"],
        serde_json::json!(subtitle_id),
        "the fence keys on the dropped member's stable catalog id: {preview_value:#?}"
    );
    support::assert_matches_golden(
        fence["message"].as_str().expect("fence message"),
        "evolve_repair_required_member_drop.txt",
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
        root.to_str().expect("project path utf-8"),
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
        root.to_str().expect("project path utf-8"),
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
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    assert!(
        integrity_codes(&support::json(integrity.stdout)).contains(&"data.orphan"),
        "the lingering cell is still an orphan"
    );
}

#[test]
fn a_populated_bare_member_drop_does_not_run_without_a_retire_intent() {
    let (root, place, subtitle_id) =
        project_with_orphaned_subtitle_run("orphan-member-survives-run");
    let baseline_epoch = store_epoch(&root).expect("baseline epoch");

    let run = marrow(&["run", root.to_str().unwrap()]);

    assert_eq!(
        run.status.code(),
        Some(1),
        "a populated bare member drop must fence the default run: {run:?}"
    );
    assert_run_fences_on_schema_drift(&run.stderr, "a populated bare member drop");
    // The run surface embeds the same repair guidance inside the schema-drift
    // fault; this composition is a render contract, pinned once here.
    support::assert_matches_golden(
        &support::last_fault(&run.stderr),
        "run_fault_schema_drift_member_drop.txt",
    );
    assert_eq!(
        store_epoch(&root),
        Some(baseline_epoch),
        "a fenced bare member drop does not advance the epoch"
    );
    {
        let store = open_native_store(&root);
        assert_eq!(
            read_scalar_by_catalog_id(&store, &place, 1, &subtitle_id, ScalarType::Str),
            Some(Scalar::Str("Appendix".into())),
            "the dropped member's cell survives the fenced run"
        );
        assert_eq!(
            read_scalar(&store, &place, 2, "title", ScalarType::Str),
            None,
            "the default entry did not execute after the activation fence"
        );
    }

    let integrity = marrow(&[
        "data",
        "integrity",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    assert!(
        integrity_codes(&support::json(integrity.stdout)).contains(&"data.orphan"),
        "the dropped member's lingering cell is still reported by integrity"
    );
}

#[test]
fn evolve_preview_fences_a_populated_whole_resource_drop() {
    // Dropping the whole `Book` resource takes its store with it. Its records would be
    // orphaned under the gone root, so the source-attached preview fails closed
    // and names `evolve retire`, exactly as a populated member drop does.
    let (root, place) = project_with_orphaned_book_resource("orphan-resource-surfaces");
    let store_id = store_catalog_id(&place).expect("dropped root store catalog id");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
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
    // One fence per dropped root, keyed on the store's stable catalog id, not its rendered
    // source spelling.
    let drop_fences: Vec<_> = blocking
        .iter()
        .filter(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(store_id.as_str())
        })
        .collect();
    assert_eq!(
        drop_fences.len(),
        1,
        "exactly one fence repairs the dropped root by catalog id: {preview_value:#?}"
    );
    support::assert_matches_golden(
        drop_fences[0]["message"].as_str().expect("fence message"),
        "evolve_repair_required_root_drop.txt",
    );
}

fn assert_store_only_preview_fences(name: &str, source: &str, context: &str) {
    let (root, place, _log) = project_with_store_only_change(name, source, true);
    let store_id = store_catalog_id(&place).expect("dropped root store catalog id");

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let value = support::json(preview.stdout);
    assert_eq!(value["status"], serde_json::json!("blocked"));
    let blocking = value["blocking"].as_array().expect("blocking array");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(store_id.as_str())
        }),
        "{context} preview should fence the store root by catalog id: {value:#?}"
    );
}

fn assert_populated_store_only_change_does_not_apply_or_run(
    name: &str,
    source: &str,
    context: &str,
) {
    let (root, place, log) = project_with_store_only_change(name, source, true);
    let baseline_epoch = store_epoch(&root).expect("baseline epoch");

    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    assert_repair_required(&support::json(apply.stdout), &format!("{context} apply"));

    let run = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(
        run.status.code(),
        Some(1),
        "{context} must fence the default run: {run:?}"
    );
    assert_run_fences_on_schema_drift(&run.stderr, context);
    assert_eq!(
        store_epoch(&root),
        Some(baseline_epoch),
        "{context} does not advance the epoch"
    );
    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar(&store, &place, 1, "title", ScalarType::Str),
            Some(Scalar::Str("Dune".into())),
            "{context}'s record survives the fenced apply and run"
        );
        assert_eq!(
            read_scalar(&store, &log, 1, "note", ScalarType::Str),
            None,
            "the default entry did not execute after the activation fence"
        );
    }

    let integrity = marrow(&[
        "data",
        "integrity",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    assert!(
        integrity_codes(&support::json(integrity.stdout)).contains(&"data.orphan"),
        "{context}'s lingering records are orphans"
    );
}

fn assert_empty_store_only_change_is_a_free_no_op(name: &str, source: &str, context: &str) {
    let (root, _place, log) = project_with_store_only_change(name, source, false);

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(preview.status.code(), Some(0), "{preview:?}");
    let preview_value = support::json(preview.stdout);
    assert_eq!(preview_value["status"], serde_json::json!("activatable"));
    assert_eq!(preview_value["blocking"], serde_json::json!([]));

    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    let run = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(
        run.status.code(),
        Some(0),
        "the default entry runs after {context} is activated: {run:?}"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &log, 1, "note", ScalarType::Str),
        Some(Scalar::Str("entry-ran".into())),
        "{context} activated before the default entry executed"
    );
}

#[test]
fn evolve_preview_fences_a_populated_store_only_removal() {
    assert_store_only_preview_fences(
        "orphan-store-removal-preview",
        STORE_REMOVED_SOURCE,
        "store-only removal",
    );
}

#[test]
fn a_populated_store_only_removal_does_not_apply_or_run() {
    assert_populated_store_only_change_does_not_apply_or_run(
        "orphan-store-removal-survives",
        STORE_REMOVED_SOURCE,
        "a populated store-only removal",
    );
}

#[test]
fn an_empty_store_only_removal_is_a_free_no_op() {
    assert_empty_store_only_change_is_a_free_no_op(
        "orphan-store-removal-empty",
        STORE_REMOVED_SOURCE,
        "an empty store-only removal",
    );
}

#[test]
fn evolve_preview_fences_a_populated_store_only_rename() {
    assert_store_only_preview_fences(
        "orphan-store-rename-preview",
        STORE_RENAMED_RUN_SOURCE,
        "store-only rename",
    );
}

#[test]
fn a_populated_store_only_rename_does_not_apply_or_run() {
    assert_populated_store_only_change_does_not_apply_or_run(
        "orphan-store-rename-survives",
        STORE_RENAMED_RUN_SOURCE,
        "a populated store-only rename",
    );
}

#[test]
fn an_empty_store_only_rename_is_a_free_no_op() {
    assert_empty_store_only_change_is_a_free_no_op(
        "orphan-store-rename-empty",
        STORE_RENAMED_RUN_SOURCE,
        "an empty store-only rename",
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
        root.to_str().expect("project path utf-8"),
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
        root.to_str().expect("project path utf-8"),
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
    assert_run_fences_on_schema_drift(&run.stderr, "a populated whole-resource drop");

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
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    assert!(
        integrity_codes(&support::json(integrity.stdout)).contains(&"data.orphan"),
        "the dropped store's lingering records are orphans"
    );
}
