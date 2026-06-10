use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::{marrow, write};
use support_evolve::{
    BLOCK_BASELINE_SOURCE, RENAME_BLOCK_DELETED_SOURCE, RENAME_BLOCK_SOURCE, RENAME_SOURCE,
    RETIRE_BASELINE_SOURCE, RETIRE_BLOCK_DELETED_SOURCE, RETIRE_BLOCK_SOURCE, RETIRE_SOURCE,
    accepted_catalog, commit_catalog, member_catalog_id, native_books_project, native_store_path,
    open_native_store, read_scalar, root_place, seed_member, seed_title_only, store_epoch,
};

#[test]
fn evolve_apply_accepts_two_repeated_approve_retire_flags() {
    let root = native_books_project(
        "evolve-apply-multi-retire",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   notes: string\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    let notes_id = member_catalog_id(&accepted_place, "notes");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
        seed_member(
            &store,
            &accepted_place,
            1,
            "notes",
            Scalar::Str("note".into()),
        );
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         \x20   retire Book.notes\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--approve-retire",
        &format!("{notes_id}:1"),
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let subtitle_present = read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str);
    let notes_present = read_scalar(&store, &accepted_place, 1, "notes", ScalarType::Str);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Both approved retires apply: the retire witness counts the two cells removed,
    // asserted as the typed envelope field rather than the rendered count line.
    let record = support::json(output.stdout);
    assert_eq!(record["kind"], serde_json::json!("evolve_apply"));
    assert_eq!(record["status"], serde_json::json!("applied"));
    assert_eq!(record["records_retired"], serde_json::json!(2));
    assert_eq!(subtitle_present, None, "subtitle was retired");
    assert_eq!(notes_present, None, "notes was retired");
}

/// A bare source rename of a populated member — `subtitle` renamed to `blurb` in source
/// with no `evolve rename` intent — must not silently auto-apply on a plain `marrow run`.
/// A bare diff is ambiguous between rename and delete-and-add; reading it as delete-and-add
/// would orphan the populated `subtitle` and silently advance the epoch. The populated-drop
/// fence catches it: the run fails closed naming the required repair rather than dropping
/// the data, and the epoch does not advance.
#[test]
fn a_bare_rename_of_a_populated_member_does_not_silently_auto_apply() {
    let root = native_books_project("bare-rename-fences", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
    }

    // Rename `subtitle` to `blurb` in source only, with no `evolve rename` block and a
    // runnable entry that reads the renamed member.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   blurb: string\n\
         pub fn show(): string\n\
         \x20   return (^books(1).blurb ?? \"absent\")\n",
    );

    let run = marrow(&["run", "--entry", "books::show", root.to_str().unwrap()]);
    assert_eq!(
        run.status.code(),
        Some(1),
        "a bare rename over populated data must fence, not silently auto-apply: {run:?}"
    );
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift rather than dropping the data: {stderr}"
    );

    // The epoch did not advance and the old `subtitle` cell still carries its data: nothing
    // was silently dropped.
    assert_eq!(
        store_epoch(&root),
        Some(baseline_epoch),
        "a fenced bare rename does not advance the epoch"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("Appendix".into())),
        "the populated member's cell survives the fenced run"
    );
}

#[test]
fn evolve_apply_advances_accepted_catalog_in_lockstep_for_retire() {
    let root = native_books_project("evolve-apply-retire-lockstep", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let file_epoch = accepted_catalog(&root).epoch;
    let store_epoch = store_epoch(&root);
    assert_eq!(
        store_epoch,
        Some(baseline_epoch + 1),
        "store advanced one epoch"
    );
    assert_eq!(
        file_epoch,
        baseline_epoch + 1,
        "accepted catalog file advanced in lockstep with the store"
    );

    // With the accepted file left behind the store epoch, the open fence rejects every
    // later run as `run.store_evolved` with no recovery; the lockstep advance keeps the
    // file and store at one epoch, so the fence never reports the store as evolved.
    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after lockstep advance: {stderr}"
    );
}

#[test]
fn evolve_apply_advances_accepted_catalog_in_lockstep_for_rename() {
    let root = native_books_project("evolve-apply-rename-lockstep", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", RENAME_SOURCE);

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let catalog = accepted_catalog(&root);
    assert_eq!(
        catalog.epoch,
        baseline_epoch + 1,
        "file advanced in lockstep"
    );
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // The renamed member keeps its stable id, records the new path, and leaves
    // the old spelling as an alias rather than a live path.
    let blurb = catalog
        .entries
        .iter()
        .find(|entry| entry.path == "books::Book::blurb")
        .expect("renamed member recorded at its new path");
    assert_eq!(
        blurb.stable_id, subtitle_id,
        "rename preserves the stable id"
    );
    assert!(
        catalog
            .entries
            .iter()
            .all(|entry| entry.path != "books::Book::subtitle"),
        "old path is not left as a live spelling"
    );
    assert!(
        blurb
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::subtitle"),
        "old path survives as an alias"
    );

    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after lockstep advance: {stderr}"
    );
}

// After a rename apply, the rename is recorded in the accepted catalog. The evolve
// block is a transient transition the author may keep or delete; neither choice may
// break `marrow run`. The store fences on the durable shape, which a consumed rename
// block does not change, and the consumed rename is treated as satisfied at check.
#[test]
fn run_succeeds_after_rename_apply_with_block_present_or_deleted() {
    let root = native_books_project("run-after-rename-block", BLOCK_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");

    // The baseline run stamps the store and writes record 2; a subtitle cell on that
    // stamped record gives the later rename real data to carry forward.
    let baseline = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "baseline run: {baseline:?}"
    );
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &accepted_place,
            2,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }

    write(&root, "src/books.mw", RENAME_BLOCK_SOURCE);
    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "rename apply: {apply:?}");

    let kept = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "run with the consumed rename block still present: {kept:?}"
    );

    write(&root, "src/books.mw", RENAME_BLOCK_DELETED_SOURCE);
    let deleted = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "run after deleting the consumed rename block: {deleted:?}"
    );
}

// After a retire apply, the retire is recorded in the accepted catalog. The evolve
// block is transient; keeping or deleting it must not break `marrow run`.
#[test]
fn run_succeeds_after_retire_apply_with_block_present_or_deleted() {
    let root = native_books_project("run-after-retire-block", BLOCK_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");

    // The baseline run stamps the store and writes record 2; a subtitle cell on that
    // stamped record gives the later retire one populated cell to approve.
    let baseline = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "baseline run: {baseline:?}"
    );
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &accepted_place,
            2,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }

    write(&root, "src/books.mw", RETIRE_BLOCK_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(0), "retire apply: {apply:?}");

    let kept = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "run with the consumed retire block still present: {kept:?}"
    );

    write(&root, "src/books.mw", RETIRE_BLOCK_DELETED_SOURCE);
    let deleted = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "run after deleting the consumed retire block: {deleted:?}"
    );
}
