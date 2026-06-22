use std::fs;
use std::path::Path;

use crate::support;
use crate::support_evolve;
use marrow_store::cell::{CatalogId, DataCellKey};
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    DataPathSegment, TreeEnumMember, TreeStore, decode_tree_enum_member, encode_tree_enum_member,
};
use marrow_store::value::{Scalar, encode_value};
use support::{marrow, write};
use support_evolve::{
    BRANCH_WORKFLOW_BASELINE_SOURCE, BRANCH_WORKFLOW_EVOLVED_SOURCE, LEAF_RETYPE_BASELINE_SOURCE,
    LEAF_RETYPE_RETIRE_OLD_SOURCE, LEAF_RETYPE_TRANSFORM_SOURCE, ORPHAN_REPAIR_SOURCE,
    ORPHAN_REPAIRED_TARGET_SOURCE, STORE_REKEY_BASELINE_SOURCE, STORE_REKEY_STRING_TARGET_SOURCE,
    TRANSFORM_FAULT_BASELINE_SOURCE, TRANSFORM_FAULT_OVERFLOW_SOURCE, accepted_catalog,
    accepted_catalog_entry_id, native_books_project, native_store_path, store_epoch,
};

#[test]
fn branch_workflow_conflict_resolution_keeps_losing_store_fenced() {
    let root = native_books_project(
        "evolve-unhappy-branch-workflow",
        BRANCH_WORKFLOW_BASELINE_SOURCE,
    );
    let dir = root.to_str().unwrap();

    let baseline = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(baseline.status.code(), Some(0), "{baseline:?}");
    assert_eq!(store_epoch(&root), Some(1));
    let losing_branch_store = fs::read(native_store_path(&root)).expect("read losing store");

    write(&root, "src/books.mw", BRANCH_WORKFLOW_EVOLVED_SOURCE);
    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");
    assert_eq!(store_epoch(&root), Some(2));
    let resolved_lock = fs::read_to_string(root.join("marrow.lock")).expect("read resolved lock");

    write(
        &root,
        "marrow.lock",
        &format!("<<<<<<< HEAD\n{resolved_lock}\n=======\n{{}}\n>>>>>>> branch\n"),
    );
    let conflicted = marrow(&["check", dir]);
    assert_eq!(
        conflicted.status.code(),
        Some(1),
        "conflicted lock must fail check: {conflicted:?}"
    );
    let stderr = String::from_utf8(conflicted.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("catalog.lock_corrupt"),
        "the conflict marker surfaces the typed lock-corrupt code: {stderr}"
    );

    write(&root, "marrow.lock", &resolved_lock);
    let resolved = marrow(&["check", dir]);
    assert_eq!(
        resolved.status.code(),
        Some(0),
        "resolving the lock conflict makes source check green: {resolved:?}"
    );

    fs::write(native_store_path(&root), losing_branch_store).expect("restore losing store");
    assert_eq!(store_epoch(&root), Some(1));
    let fenced = marrow(&["run", "--entry", "books::noop", dir]);
    assert_eq!(
        fenced.status.code(),
        Some(1),
        "the losing branch store must fence before execution: {fenced:?}"
    );
    let stderr = String::from_utf8(fenced.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.store_behind"),
        "the losing branch store hits the activation fence: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(root.join("marrow.lock")).expect("read lock after fence"),
        resolved_lock,
        "the fence does not rewrite the resolved lock"
    );
}

#[test]
fn worked_leaf_retype_migrates_then_retires_old_leaf_bytes() {
    let root = native_books_project("evolve-unhappy-leaf-retype", LEAF_RETYPE_BASELINE_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_pages_id),
        Some(encode_value(&Scalar::Int(3)).expect("encode pages")),
        "baseline stores the populated int leaf under the old catalog id"
    );

    write(&root, "src/books.mw", LEAF_RETYPE_TRANSFORM_SOURCE);
    let transform = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(transform.status.code(), Some(0), "{transform:?}");
    let transform_record = support::json(transform.stdout);
    assert_eq!(
        transform_record["records_transformed"],
        serde_json::json!(1),
        "the retype migration is a checked per-record transform"
    );

    let page_label_id = accepted_catalog_entry_id(&root, "books::Book::pageLabel");
    let expected_label_bytes =
        encode_value(&Scalar::Str("pages:3".to_string())).expect("encode page label");
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &page_label_id),
        Some(expected_label_bytes.clone()),
        "the new string leaf stores the evolved bytes"
    );

    write(&root, "src/books.mw", LEAF_RETYPE_RETIRE_OLD_SOURCE);
    let retire = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{old_pages_id}:1"),
        "--no-backup",
        "--format",
        "json",
        dir,
    ]);
    assert_eq!(retire.status.code(), Some(0), "{retire:?}");
    let retire_record = support::json(retire.stdout);
    assert_eq!(retire_record["records_retired"], serde_json::json!(1));
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_pages_id),
        None,
        "the populated old leaf storage is deleted under the old catalog id"
    );
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &page_label_id),
        Some(expected_label_bytes),
        "retiring the old leaf does not rewrite the evolved bytes"
    );
}

/// A transform body that faults over a real record (here an integer overflow) blocks the
/// migration. The `evolve.transform_faulted` JSON diagnostic must name the offending
/// record identity and the underlying runtime fault code, not an opaque empty payload, so
/// an operator knows which record and which fault to fix.
#[test]
fn a_faulting_transform_reports_the_record_and_underlying_fault_code() {
    let root = native_books_project(
        "evolve-unhappy-transform-fault",
        TRANSFORM_FAULT_BASELINE_SOURCE,
    );
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");

    // The transform multiplies a nine-billion price by 1e12, overflowing `int` for the
    // seeded record. Apply must fail closed with the enriched diagnostic.
    write(&root, "src/books.mw", TRANSFORM_FAULT_OVERFLOW_SOURCE);
    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");

    let diagnostic = support::json(apply.stdout);
    assert_eq!(
        diagnostic["code"], "evolve.transform_faulted",
        "{diagnostic}"
    );
    assert_eq!(diagnostic["data"]["record"], "^books(2)", "{diagnostic}");
    assert_eq!(
        diagnostic["data"]["inner_code"], "run.overflow",
        "{diagnostic}"
    );
}

#[test]
fn worked_store_rekey_copies_through_non_int_identity_constructor() {
    let root = native_books_project("evolve-unhappy-store-rekey", STORE_REKEY_BASELINE_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_title_id = accepted_catalog_entry_id(&root, "books::Book::title");
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_title_id),
        Some(encode_value(&Scalar::Str("Dune".into())).expect("encode title")),
        "baseline stores the source record under the int-keyed root"
    );

    write(&root, "src/books.mw", STORE_REKEY_STRING_TARGET_SOURCE);
    let migrate = marrow(&["run", "--maintenance", "--entry", "books::migrate", dir]);
    assert_eq!(migrate.status.code(), Some(0), "{migrate:?}");
    let show = marrow(&["run", "--entry", "books::showNew", dir]);
    assert_eq!(show.status.code(), Some(0), "{show:?}");
    assert_eq!(
        String::from_utf8(show.stdout).expect("stdout utf8"),
        "Dune\n"
    );

    assert!(
        !record_exists(&root, "books::^books", &[SavedKey::Int(1)]),
        "the source int-keyed record is deleted rather than reinterpreted"
    );
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_title_id),
        None,
        "source store data is not silently reused under the old ids"
    );

    let new_title_id = accepted_catalog_entry_id(&root, "books::Book::title");
    let expected_title = encode_value(&Scalar::Str("Dune".into())).expect("encode title");
    assert_eq!(
        read_member_bytes(
            &root,
            "books::^booksBySlug",
            &[SavedKey::Str("book-1".into())],
            &new_title_id,
        ),
        Some(expected_title),
        "the string-keyed target is addressed through Id(^booksBySlug, \"book-1\")"
    );
}

#[test]
fn populated_store_key_shape_change_fences_preview_apply_and_run() {
    const REKEYED_SAME_STORE_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(slug: string): Book\n\
         pub fn noop()\n\
         \x20   print(\"entry executed\")\n";

    let root = native_books_project(
        "evolve-unhappy-store-key-shape",
        STORE_REKEY_BASELINE_SOURCE,
    );
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_store_id = accepted_catalog_entry_id(&root, "books::^books");
    let old_title_id = accepted_catalog_entry_id(&root, "books::Book::title");
    let old_title_bytes = encode_value(&Scalar::Str("Dune".into())).expect("encode title");
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_title_id),
        Some(old_title_bytes.clone()),
        "baseline stores the populated title under the int-keyed identity"
    );
    let old_epoch = store_epoch(&root);
    assert_eq!(old_epoch, Some(1));

    write(&root, "src/books.mw", REKEYED_SAME_STORE_SOURCE);
    let preview = marrow(&["evolve", "preview", "--format", "json", dir]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_json = support::json(preview.stdout);
    assert_eq!(preview_json["status"], serde_json::json!("blocked"));
    let blocking = preview_json["blocking"]
        .as_array()
        .expect("blocking reports");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(old_store_id)
        }),
        "preview should report repair required for the re-keyed populated store: {preview_json:#?}"
    );

    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    let apply_json = support::json(apply.stdout);
    assert_eq!(
        apply_json["code"],
        serde_json::json!("evolve.repair_required")
    );
    assert_eq!(
        apply_json["data"]["catalog_id"],
        serde_json::json!(old_store_id)
    );
    assert_eq!(
        store_epoch(&root),
        old_epoch,
        "refused apply does not advance the durable store epoch"
    );
    assert!(record_exists(&root, "books::^books", &[SavedKey::Int(1)]));
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_title_id),
        Some(old_title_bytes.clone()),
        "refused apply leaves the old int-keyed member bytes in place"
    );

    let run = marrow(&["run", "--entry", "books::noop", dir]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift before executing the entry: {stderr}"
    );
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert!(
        !stdout.contains("entry executed"),
        "the schema drift fence must stop before entry output: {stdout}"
    );
    assert_eq!(
        store_epoch(&root),
        old_epoch,
        "fenced run does not advance the durable store epoch"
    );
    assert!(record_exists(&root, "books::^books", &[SavedKey::Int(1)]));
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_title_id),
        Some(old_title_bytes),
        "fenced run leaves the old int-keyed member bytes in place"
    );
}

#[test]
fn populated_keyed_layer_key_shape_change_fences_preview_apply_and_run() {
    const BASELINE_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   versions(version: int)\n\
         \x20       required body: string\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   ^books(1).title = \"Dune\"\n\
         \x20   ^books(1).versions(7).body = \"draft\"\n";
    const KEY_RESHAPED_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   versions(version: string)\n\
         \x20       required body: string\n\
         store ^books(id: int): Book\n\
         pub fn noop()\n\
         \x20   print(\"keyed layer entry executed\")\n";

    let root = native_books_project("evolve-unhappy-keyed-layer-key-shape", BASELINE_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_versions_id = accepted_catalog_entry_id(&root, "books::Book::versions");
    let old_body_id = accepted_catalog_entry_id(&root, "books::Book::versions::body");
    let old_body_path = [
        member_segment(&old_versions_id),
        DataPathSegment::Key(SavedKey::Int(7)),
        member_segment(&old_body_id),
    ];
    let old_body_bytes = encode_value(&Scalar::Str("draft".into())).expect("encode body");
    assert_eq!(
        read_path_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_body_path),
        Some(old_body_bytes.clone()),
        "baseline stores the keyed-layer leaf under the int-keyed layer path"
    );
    let old_epoch = store_epoch(&root);
    assert_eq!(old_epoch, Some(1));

    write(&root, "src/books.mw", KEY_RESHAPED_SOURCE);
    let preview = marrow(&["evolve", "preview", "--format", "json", dir]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_json = support::json(preview.stdout);
    assert_eq!(preview_json["status"], serde_json::json!("blocked"));
    let blocking = preview_json["blocking"]
        .as_array()
        .expect("blocking reports");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(old_versions_id)
        }),
        "preview should report repair required for the populated keyed layer: {preview_json:#?}"
    );

    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    let apply_json = support::json(apply.stdout);
    assert_eq!(
        apply_json["code"],
        serde_json::json!("evolve.repair_required")
    );
    assert_eq!(
        apply_json["data"]["catalog_id"],
        serde_json::json!(old_versions_id)
    );
    assert_eq!(
        store_epoch(&root),
        old_epoch,
        "refused apply does not advance the durable store epoch"
    );
    assert!(record_exists(&root, "books::^books", &[SavedKey::Int(1)]));
    assert_eq!(
        read_path_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_body_path),
        Some(old_body_bytes.clone()),
        "refused apply leaves the old keyed-layer bytes in place"
    );

    let run = marrow(&["run", "--entry", "books::noop", dir]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift before executing the entry: {stderr}"
    );
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert!(
        !stdout.contains("keyed layer entry executed"),
        "the schema drift fence must stop before entry output: {stdout}"
    );
    assert_eq!(
        store_epoch(&root),
        old_epoch,
        "fenced run does not advance the durable store epoch"
    );
    assert!(record_exists(&root, "books::^books", &[SavedKey::Int(1)]));
    assert_eq!(
        read_path_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_body_path),
        Some(old_body_bytes),
        "fenced run leaves the old keyed-layer bytes in place"
    );
}

#[test]
fn populated_unkeyed_group_to_keyed_layer_fences_preview_apply_and_run() {
    const BASELINE_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   notes\n\
         \x20       required body: string\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^books(1).title = \"Dune\"\n\
         \x20       ^books(1).notes.body = \"draft\"\n";
    const KEYED_RESHAPED_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   notes(version: int)\n\
         \x20       required body: string\n\
         store ^books(id: int): Book\n\
         pub fn noop()\n\
         \x20   print(\"unkeyed group to keyed layer entry executed\")\n";

    let root = native_books_project("evolve-unhappy-group-to-keyed-layer", BASELINE_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_notes_id = accepted_catalog_entry_id(&root, "books::Book::notes");
    let old_body_id = accepted_catalog_entry_id(&root, "books::Book::notes::body");
    let old_body_path = [member_segment(&old_notes_id), member_segment(&old_body_id)];
    let old_body_bytes = encode_value(&Scalar::Str("draft".into())).expect("encode body");
    assert_eq!(
        read_path_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_body_path),
        Some(old_body_bytes.clone()),
        "baseline stores the unkeyed-group leaf under the old group path"
    );

    write(&root, "src/books.mw", KEYED_RESHAPED_SOURCE);
    assert_group_keyed_reshape_fences_preview_apply_and_run(
        &root,
        dir,
        &old_notes_id,
        &old_body_path,
        &old_body_bytes,
        "unkeyed group to keyed layer entry executed",
    );
}

#[test]
fn populated_keyed_layer_to_unkeyed_group_fences_preview_apply_and_run() {
    const BASELINE_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   notes(version: int)\n\
         \x20       required body: string\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^books(1).title = \"Dune\"\n\
         \x20       ^books(1).notes(7).body = \"draft\"\n";
    const GROUP_RESHAPED_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   notes\n\
         \x20       required body: string\n\
         store ^books(id: int): Book\n\
         pub fn noop()\n\
         \x20   print(\"keyed layer to unkeyed group entry executed\")\n";

    let root = native_books_project("evolve-unhappy-keyed-layer-to-group", BASELINE_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_notes_id = accepted_catalog_entry_id(&root, "books::Book::notes");
    let old_body_id = accepted_catalog_entry_id(&root, "books::Book::notes::body");
    let old_body_path = [
        member_segment(&old_notes_id),
        DataPathSegment::Key(SavedKey::Int(7)),
        member_segment(&old_body_id),
    ];
    let old_body_bytes = encode_value(&Scalar::Str("draft".into())).expect("encode body");
    assert_eq!(
        read_path_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_body_path),
        Some(old_body_bytes.clone()),
        "baseline stores the keyed-layer leaf under the old keyed path"
    );

    write(&root, "src/books.mw", GROUP_RESHAPED_SOURCE);
    assert_group_keyed_reshape_fences_preview_apply_and_run(
        &root,
        dir,
        &old_notes_id,
        &old_body_path,
        &old_body_bytes,
        "keyed layer to unkeyed group entry executed",
    );
}

const ENUM_UNSELECT_BASELINE_SOURCE: &str = "module books\n\
     enum Status\n\
     \x20   draft\n\
     \x20   archived\n\
     \x20   review\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required status: Status\n\
     store ^books(id: int): Book\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Dune\"\n\
     \x20       ^books(1).status = Status::archived\n";

#[test]
fn stored_enum_value_naming_removed_member_fences_preview_apply_and_run() {
    const REMOVED_SOURCE: &str = "module books\n\
         enum Status\n\
         \x20   draft\n\
         \x20   review\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required status: Status\n\
         store ^books(id: int): Book\n\
         pub fn noop()\n\
         \x20   print(\"enum removed entry executed\")\n";

    assert_seeded_enum_status_fences_after_source_change(
        "evolve-unhappy-enum-member-removed",
        REMOVED_SOURCE,
        "enum removed entry executed",
    );
}

#[test]
fn stored_enum_value_naming_member_later_made_category_fences_preview_apply_and_run() {
    const CATEGORY_SOURCE: &str = "module books\n\
         enum Status\n\
         \x20   draft\n\
         \x20   category archived\n\
         \x20       old\n\
         \x20   review\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required status: Status\n\
         store ^books(id: int): Book\n\
         pub fn noop()\n\
         \x20   print(\"enum category entry executed\")\n";

    assert_seeded_enum_status_fences_after_source_change(
        "evolve-unhappy-enum-member-category",
        CATEGORY_SOURCE,
        "enum category entry executed",
    );
}

#[test]
fn stored_enum_value_naming_bare_renamed_member_fences_preview_apply_and_run() {
    const BARE_RENAME_SOURCE: &str = "module books\n\
         enum Status\n\
         \x20   draft\n\
         \x20   stored\n\
         \x20   review\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required status: Status\n\
         store ^books(id: int): Book\n\
         pub fn noop()\n\
         \x20   print(\"enum rename entry executed\")\n";

    assert_seeded_enum_status_fences_after_source_change(
        "evolve-unhappy-enum-member-bare-rename",
        BARE_RENAME_SOURCE,
        "enum rename entry executed",
    );
}

#[test]
fn explicit_enum_member_rename_preserves_seeded_value() {
    const EXPLICIT_RENAME_SOURCE: &str = "module books\n\
         evolve\n\
         \x20   rename Status::archived -> Status::stored\n\
         enum Status\n\
         \x20   draft\n\
         \x20   stored\n\
         \x20   review\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required status: Status\n\
         store ^books(id: int): Book\n\
         pub fn show()\n\
         \x20   print(^books(1).status ?? Status::draft)\n";

    let root = native_books_project(
        "evolve-unhappy-enum-member-explicit-rename",
        ENUM_UNSELECT_BASELINE_SOURCE,
    );
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_status_id = accepted_catalog_entry_id(&root, "books::Book::status");
    let old_archived_id = accepted_catalog_entry_id(&root, "books::Status::archived");
    let old_status_bytes =
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_status_id)
            .expect("seeded status bytes");
    assert_seeded_archived_enum_bytes(&root, &old_status_bytes);
    let old_epoch = store_epoch(&root).expect("baseline epoch");

    write(&root, "src/books.mw", EXPLICIT_RENAME_SOURCE);
    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");
    let apply_json = support::json(apply.stdout);
    assert_eq!(apply_json["kind"], serde_json::json!("evolve_apply"));
    assert_eq!(apply_json["status"], serde_json::json!("applied"));
    assert_eq!(
        store_epoch(&root),
        Some(old_epoch + 1),
        "explicit enum-member rename advances the accepted catalog epoch"
    );

    let new_status_id = accepted_catalog_entry_id(&root, "books::Book::status");
    let new_stored_id = accepted_catalog_entry_id(&root, "books::Status::stored");
    assert_eq!(
        new_status_id, old_status_id,
        "the enum-typed field keeps its stable identity"
    );
    assert_eq!(
        new_stored_id, old_archived_id,
        "evolve rename moves the enum member's stable identity to the new spelling"
    );
    assert_eq!(
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &new_status_id),
        Some(old_status_bytes),
        "explicit rename leaves the stored enum-member bytes attached to the same field"
    );

    let show = marrow(&["run", "--entry", "books::show", dir]);
    assert_eq!(show.status.code(), Some(0), "{show:?}");
    assert_eq!(
        String::from_utf8(show.stdout).expect("stdout utf8"),
        "Status::stored\n"
    );
}

#[test]
fn worked_orphan_repair_is_bracketed_by_integrity() {
    let root = native_books_project("evolve-unhappy-orphan-repair", ORPHAN_REPAIR_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_subtitle_id = accepted_catalog_entry_id(&root, "books::Book::subtitle");
    let expected_subtitle = encode_value(&Scalar::Str("Appendix".into())).expect("encode subtitle");
    assert_eq!(
        read_member_bytes(
            &root,
            "books::^books",
            &[SavedKey::Int(1)],
            &old_subtitle_id,
        ),
        Some(expected_subtitle),
        "the repair fixture starts with the old subtitle cell populated"
    );

    write(&root, "src/books.mw", ORPHAN_REPAIRED_TARGET_SOURCE);
    let before = marrow(&["data", "integrity", "--format", "json", dir]);
    assert_eq!(before.status.code(), Some(1), "{before:?}");
    let before_problems = support::json(before.stdout);
    assert!(
        before_problems["problems"]
            .as_array()
            .expect("problems array")
            .iter()
            .any(|problem| problem["code"] == serde_json::json!("data.orphan")),
        "the dropped member is visible as an orphan before repair"
    );

    write(&root, "src/books.mw", ORPHAN_REPAIR_SOURCE);
    let repair = marrow(&["run", "--maintenance", "--entry", "books::repair", dir]);
    assert_eq!(repair.status.code(), Some(0), "{repair:?}");
    assert_eq!(
        read_member_bytes(
            &root,
            "books::^books",
            &[SavedKey::Int(1)],
            &old_subtitle_id,
        ),
        None,
        "maintenance repair deletes the old subtitle cell bytes before integrity is rerun"
    );

    write(&root, "src/books.mw", ORPHAN_REPAIRED_TARGET_SOURCE);
    let after = marrow(&["data", "integrity", "--format", "json", dir]);
    assert_eq!(after.status.code(), Some(0), "{after:?}");
    assert_eq!(
        support::json(after.stdout)["problems"],
        serde_json::json!([])
    );
}

fn catalog_id(root: impl AsRef<Path>, path: &str) -> CatalogId {
    CatalogId::new(accepted_catalog_entry_id(root, path)).expect("catalog id")
}

fn read_member_bytes(
    root: impl AsRef<Path>,
    store_path: &str,
    identity: &[SavedKey],
    member_id: &str,
) -> Option<Vec<u8>> {
    let store_id = catalog_id(root.as_ref(), store_path);
    let store =
        TreeStore::open_read_only(&native_store_path(root.as_ref())).expect("open native store");
    store
        .read_data_value(
            &store_id,
            identity,
            &[DataPathSegment::Member(
                CatalogId::new(member_id.to_string()).expect("member catalog id"),
            )],
        )
        .expect("read member bytes")
}

fn member_segment(member_id: &str) -> DataPathSegment {
    DataPathSegment::Member(CatalogId::new(member_id.to_string()).expect("member catalog id"))
}

fn read_path_bytes(
    root: impl AsRef<Path>,
    store_path: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
) -> Option<Vec<u8>> {
    let store_id = catalog_id(root.as_ref(), store_path);
    let store =
        TreeStore::open_read_only(&native_store_path(root.as_ref())).expect("open native store");
    store
        .read_data_value(&store_id, identity, path)
        .expect("read path bytes")
}

fn data_cells_snapshot(root: impl AsRef<Path>) -> Vec<(DataCellKey, Vec<u8>)> {
    let store =
        TreeStore::open_read_only(&native_store_path(root.as_ref())).expect("open native store");
    let mut cells = Vec::new();
    store
        .visit_backup_cells(|cell| {
            cells.push((cell.data_key().clone(), cell.value().to_vec()));
            Ok(())
        })
        .expect("visit data cells");
    cells
}

fn assert_seeded_enum_status_fences_after_source_change(
    case_name: &str,
    changed_source: &str,
    sentinel: &str,
) {
    let root = native_books_project(case_name, ENUM_UNSELECT_BASELINE_SOURCE);
    let dir = root.to_str().unwrap();

    let seed = marrow(&["run", "--entry", "books::seed", dir]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let old_status_id = accepted_catalog_entry_id(&root, "books::Book::status");
    let old_status_bytes =
        read_member_bytes(&root, "books::^books", &[SavedKey::Int(1)], &old_status_id)
            .expect("seeded status bytes");
    assert_seeded_archived_enum_bytes(&root, &old_status_bytes);
    let old_epoch = store_epoch(&root);
    let old_lock = fs::read(root.join("marrow.lock")).expect("read lock before enum source change");
    let old_data_cells = data_cells_snapshot(&root);
    let old_catalog = accepted_catalog(&root)
        .to_json_pretty()
        .expect("render accepted catalog snapshot");

    write(&root, "src/books.mw", changed_source);
    assert_enum_unselect_fences_preview_apply_and_run(
        &root,
        dir,
        &old_status_id,
        &old_status_bytes,
        old_epoch,
        &old_lock,
        &old_data_cells,
        &old_catalog,
        sentinel,
    );
}

fn assert_seeded_archived_enum_bytes(root: impl AsRef<Path>, status_bytes: &[u8]) {
    let status_enum_id = catalog_id(root.as_ref(), "books::Status");
    let archived_member_id = catalog_id(root.as_ref(), "books::Status::archived");
    let expected = TreeEnumMember::new(status_enum_id, archived_member_id);
    assert_eq!(
        decode_tree_enum_member(status_bytes).expect("decode seeded enum status bytes"),
        expected,
        "seeded status bytes name books::Status::archived"
    );
    assert_eq!(
        encode_tree_enum_member(&expected).expect("encode expected enum status bytes"),
        status_bytes,
        "seeded status bytes are the canonical native enum-member payload"
    );
}

fn assert_enum_unselect_fences_preview_apply_and_run(
    root: impl AsRef<Path>,
    dir: &str,
    old_status_id: &str,
    old_status_bytes: &[u8],
    old_epoch: Option<u64>,
    old_lock: &[u8],
    old_data_cells: &[(DataCellKey, Vec<u8>)],
    old_catalog: &str,
    sentinel: &str,
) {
    let root = root.as_ref();
    assert_eq!(old_epoch, Some(1));
    let assert_unchanged = |action: &str| {
        assert_eq!(
            store_epoch(root),
            old_epoch,
            "{action} does not advance the durable store epoch"
        );
        assert_eq!(
            fs::read(root.join("marrow.lock")).expect("read lock after enum unselect fence"),
            old_lock,
            "{action} does not rewrite the committed lock"
        );
        assert_eq!(
            accepted_catalog(root)
                .to_json_pretty()
                .expect("render accepted catalog after enum unselect fence"),
            old_catalog,
            "{action} does not rewrite the store's accepted catalog snapshot"
        );
        let data_cells = data_cells_snapshot(root);
        assert_eq!(
            data_cells.as_slice(),
            old_data_cells,
            "{action} leaves the complete data-family snapshot unchanged"
        );
        assert!(record_exists(root, "books::^books", &[SavedKey::Int(1)]));
        assert_eq!(
            read_member_bytes(root, "books::^books", &[SavedKey::Int(1)], old_status_id),
            Some(old_status_bytes.to_vec()),
            "{action} leaves the old enum-field bytes in place"
        );
    };

    let preview = marrow(&["evolve", "preview", "--format", "json", dir]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_json = support::json(preview.stdout);
    assert_eq!(preview_json["status"], serde_json::json!("blocked"));
    let blocking = preview_json["blocking"]
        .as_array()
        .expect("blocking reports");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(old_status_id)
        }),
        "preview should report repair required for the populated enum field: {preview_json:#?}"
    );
    assert_unchanged("preview");

    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    let apply_json = support::json(apply.stdout);
    assert_eq!(
        apply_json["code"],
        serde_json::json!("evolve.repair_required")
    );
    assert_eq!(
        apply_json["data"]["catalog_id"],
        serde_json::json!(old_status_id)
    );
    assert_unchanged("refused apply");

    let run = marrow(&["run", "--entry", "books::noop", dir]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift before executing the entry: {stderr}"
    );
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert!(
        !stdout.contains(sentinel),
        "the schema drift fence must stop before entry output: {stdout}"
    );
    assert_unchanged("fenced run");
}

fn assert_group_keyed_reshape_fences_preview_apply_and_run(
    root: impl AsRef<Path>,
    dir: &str,
    old_group_id: &str,
    old_body_path: &[DataPathSegment],
    old_body_bytes: &[u8],
    sentinel: &str,
) {
    let root = root.as_ref();
    let old_epoch = store_epoch(root);
    assert_eq!(old_epoch, Some(1));
    let old_data_cells = data_cells_snapshot(root);
    let old_lock = fs::read(root.join("marrow.lock")).expect("read lock before reshape");

    let preview = marrow(&["evolve", "preview", "--format", "json", dir]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_json = support::json(preview.stdout);
    assert_eq!(preview_json["status"], serde_json::json!("blocked"));
    let blocking = preview_json["blocking"]
        .as_array()
        .expect("blocking reports");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(old_group_id)
        }),
        "preview should report repair required for the populated old group shape: {preview_json:#?}"
    );

    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(apply.status.code(), Some(1), "{apply:?}");
    let apply_json = support::json(apply.stdout);
    assert_eq!(
        apply_json["code"],
        serde_json::json!("evolve.repair_required")
    );
    assert_eq!(
        apply_json["data"]["catalog_id"],
        serde_json::json!(old_group_id)
    );
    assert_eq!(
        store_epoch(root),
        old_epoch,
        "refused apply does not advance the durable store epoch"
    );
    assert_eq!(
        fs::read(root.join("marrow.lock")).expect("read lock after refused apply"),
        old_lock,
        "refused apply does not rewrite the committed lock"
    );
    assert_eq!(
        data_cells_snapshot(root),
        old_data_cells,
        "refused apply leaves the complete data-family snapshot unchanged"
    );
    assert!(record_exists(root, "books::^books", &[SavedKey::Int(1)]));
    assert_eq!(
        read_path_bytes(root, "books::^books", &[SavedKey::Int(1)], old_body_path),
        Some(old_body_bytes.to_vec()),
        "refused apply leaves the old group/keyed bytes in place"
    );

    let run = marrow(&["run", "--entry", "books::noop", dir]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift before executing the entry: {stderr}"
    );
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert!(
        !stdout.contains(sentinel),
        "the schema drift fence must stop before entry output: {stdout}"
    );
    assert_eq!(
        store_epoch(root),
        old_epoch,
        "fenced run does not advance the durable store epoch"
    );
    assert_eq!(
        fs::read(root.join("marrow.lock")).expect("read lock after fenced run"),
        old_lock,
        "fenced run does not rewrite the committed lock"
    );
    assert_eq!(
        data_cells_snapshot(root),
        old_data_cells,
        "fenced run leaves the complete data-family snapshot unchanged"
    );
    assert!(record_exists(root, "books::^books", &[SavedKey::Int(1)]));
    assert_eq!(
        read_path_bytes(root, "books::^books", &[SavedKey::Int(1)], old_body_path),
        Some(old_body_bytes.to_vec()),
        "fenced run leaves the old group/keyed bytes in place"
    );
}

fn record_exists(root: impl AsRef<Path>, store_path: &str, identity: &[SavedKey]) -> bool {
    let store_id = catalog_id(root.as_ref(), store_path);
    let store =
        TreeStore::open_read_only(&native_store_path(root.as_ref())).expect("open native store");
    store
        .record_identity_exists_under(&store_id, identity, identity.len())
        .expect("read record existence")
}
