//! Where dropped-member orphan data surfaces. A populated member removed from
//! source with no `evolve retire` intent and no dependent index is a legal
//! sparse-field drop: its stored cells linger, and the read-only `marrow data
//! integrity` surface is what reports them as `data.orphan`. The source-attached
//! activation surfaces (`check --data`, `evolve preview`) treat the bare drop as a
//! no-op, because the lingering data depends on nothing the new schema requires.
//! This pins which surface owns orphan detection, so a future change cannot silently
//! move the boundary.

use marrow_check::CheckedSavedPlace;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::marrow;
use support_evolve::{
    RETIRE_BASELINE_SOURCE, commit_catalog, member_catalog_id, native_books_project,
    open_native_store, read_scalar_by_catalog_id, root_place, seed_member, seed_title_only,
};

/// The baseline `RETIRE_BASELINE_SOURCE` resource with its `subtitle` member dropped
/// from source — no `evolve retire` intent, no index reads it. The accepted catalog
/// still records `subtitle`, but current source no longer declares it.
const SUBTITLE_DROPPED_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
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
fn check_data_and_evolve_preview_treat_a_bare_member_drop_as_a_no_op() {
    // A bare member drop with no retire intent and no dependent index is a legal
    // sparse-field drop: the source-attached activation surfaces have no obligation to
    // discharge, so `check --data` passes and `evolve preview` is activatable. Orphan
    // detection belongs to `data integrity`, not these surfaces.
    let (root, _place, _subtitle_id) = project_with_orphaned_subtitle("orphan-activation-surfaces");

    let check = marrow(&[
        "check",
        "--data",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
    assert_eq!(
        support::json(check.stdout)["status"],
        serde_json::json!("ok"),
        "check --data treats the bare drop as a no-op"
    );

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
        "evolve preview treats the bare drop as activatable: {preview_value:#?}"
    );
    assert_eq!(
        preview_value["blocking"],
        serde_json::json!([]),
        "no repair-required obligation blocks a dependency-free drop"
    );
}

#[test]
fn the_dropped_member_cell_survives_activation() {
    // The lingering orphan is durable, not transient: applying the dependency-free drop
    // commits successfully and leaves the dropped member's cell exactly as it was, so it
    // remains visible to a later `data integrity` repair pass.
    let (root, place, subtitle_id) = project_with_orphaned_subtitle("orphan-survives-apply");

    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");
    assert_eq!(
        support::json(apply.stdout)["status"],
        serde_json::json!("applied"),
        "a dependency-free drop applies cleanly"
    );

    {
        // Drop the store handle before the integrity subprocess: the native engine
        // takes a single-writer lock, so a held handle would fail the CLI open.
        let store = open_native_store(&root);
        assert_eq!(
            read_scalar_by_catalog_id(&store, &place, 1, &subtitle_id, ScalarType::Str),
            Some(Scalar::Str("Appendix".into())),
            "the dropped member's cell lingers after activation"
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
        "the lingering cell is still an orphan after activation"
    );
}
