//! `marrow data integrity`: the saved-data integrity verdicts. Problems are asserted
//! by typed diagnostic code, tooling kind, and structured payloads. Display paths
//! are checked only where the rendered operator path is the contract. The shared
//! child-page limit guard is asserted on its typed path error.

use std::fs;
use std::path::Path;

use crate::support;
use crate::support_data;
use crate::support_evolve;
use marrow_check::tooling::{
    DataChild, DataPathError, DataPathSegment, DataPresence, ToolingError,
    count_activation_integrity_problems, count_integrity_problems, data_children, read_data_path,
    resolve_data_path, walk_data,
};
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment as StoreDataPathSegment, TreeStore};
use marrow_store::value::SUPPORTED_DATE_MAX_DAYS;
use support::write;
use support_data::{
    assert_stable_store_snapshot_eq, assert_store_snapshot, checked_place, checked_program,
    delete_tree_path, encode_identity_keys, field_path, integrity_problem, json, keyed_field_path,
    marrow, member_path_catalog_id, native_project, seeded_project, write_orphan_cell,
    write_record_presence, write_tree_node, write_tree_value, write_tree_value_without_node,
    write_tree_values,
};
use support_evolve::{
    REQUIRED_BASELINE_SOURCE, REQUIRED_DEFAULT_SOURCE, commit_catalog, native_books_project,
    open_native_store, root_place, seed_title_only,
};

const NATIVE_STORE_CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;

fn int_key_json(value: i64) -> serde_json::Value {
    serde_json::json!({ "type": "int", "value": value })
}

fn check_pending_source_against_accepted_store(project: &Path) -> (usize, usize) {
    let config_text = fs::read_to_string(project.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let store_path = support::native_store_path(project, &config).expect("native store path");
    let accepted = TreeStore::open_read_only(&store_path)
        .expect("open store read-only")
        .read_catalog_snapshot()
        .expect("read catalog snapshot");
    let (_report, program) =
        marrow_check::check_project_with_catalog(project, &config, accepted.as_ref())
            .expect("check pending source against accepted catalog");
    let store = TreeStore::open_read_only(&store_path).expect("open store read-only");
    count_integrity_problems(&store, &program).expect("count integrity problems")
}

#[test]
fn shared_data_children_rejects_zero_limit() {
    let (project, _dir) = seeded_project("data-children-zero-limit");
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");
    let error = data_children(
        &program,
        &store,
        &[DataPathSegment::Root("counter".into())],
        0,
        None,
    )
    .expect_err("shared child pages reject a zero limit");

    assert!(
        matches!(error, ToolingError::Path(DataPathError::ZeroLimit)),
        "expected a typed zero-limit path error, got {error:?}"
    );
}

#[test]
fn shared_data_children_returns_typed_member_segments() {
    let (project, _dir) = seeded_project("data-children-typed-members");
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");
    let record = [
        DataPathSegment::Root("counter".into()),
        DataPathSegment::Key(SavedKey::Int(1)),
    ];

    let page = data_children(&program, &store, &record, 10, None)
        .expect("record children resolve through checked member facts");

    assert!(
        page.children
            .iter()
            .any(|child| child == &DataChild::Field("value".into())),
        "plain saved field should return a field segment, got {:?}",
        page.children
    );
}

/// `read_data_path` must tell the four presence states apart at a record identity node:
/// a node carrying a real field child is `ChildrenOnly`, a structurally-existing node
/// with zero cells and zero children is `Exists`, a never-written identity is `Absent`,
/// and a populated leaf is `ValueOnly`. The zero-cell case is the regression: a deleted
/// field can leave an identity node behind, and it must not masquerade as has-children.
#[test]
fn read_data_path_distinguishes_an_empty_identity_node_from_has_children() {
    let project = native_project("data-read-empty-identity");
    let place = checked_place(&project, "counter");
    // `^counter(1)` carries a real `.value` child; `^counter(2)` is a bare node marker
    // with no cells, exactly the state a field delete leaves behind.
    write_tree_value(
        &project,
        "counter",
        &[SavedKey::Int(1)],
        &field_path(&place, "value"),
        b"42".to_vec(),
    );
    write_record_presence(&project, "counter", &[SavedKey::Int(2)]);
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");

    let presence = |path_text: &[DataPathSegment]| {
        let path = resolve_data_path(&program, path_text)
            .expect("resolve path")
            .expect("path");
        read_data_path(&store, &path)
            .expect("read presence")
            .presence
    };

    assert_eq!(
        presence(&[DataPathSegment::Root("counter".into())]),
        DataPresence::ChildrenOnly,
        "a root with record children is children-only"
    );
    assert_eq!(
        presence(&[
            DataPathSegment::Root("counter".into()),
            DataPathSegment::Key(SavedKey::Int(1)),
            DataPathSegment::Field("value".into()),
        ]),
        DataPresence::ValueOnly,
        "a populated leaf is value-only"
    );
    assert_eq!(
        presence(&[
            DataPathSegment::Root("counter".into()),
            DataPathSegment::Key(SavedKey::Int(1)),
        ]),
        DataPresence::ChildrenOnly,
        "an identity node with a real field child is children-only"
    );
    assert_eq!(
        presence(&[
            DataPathSegment::Root("counter".into()),
            DataPathSegment::Key(SavedKey::Int(2)),
        ]),
        DataPresence::Exists,
        "a zero-cell structurally-existing identity node exists without value or children"
    );
    assert_eq!(
        presence(&[
            DataPathSegment::Root("counter".into()),
            DataPathSegment::Key(SavedKey::Int(3)),
        ]),
        DataPresence::Absent,
        "a never-written identity is absent"
    );
}

/// The shared saved-data walk must page across record identities and resume from its
/// own cursor without dropping or repeating an entry. A small limit forces several
/// pages over a multi-record root; the union of the pages must be every record's
/// value leaf exactly once, in identity order.
#[test]
fn shared_data_walk_resumes_across_record_pages_exactly_once() {
    let project = native_project("data-walk-resume-pages");
    let place = checked_place(&project, "counter");
    let identities = (1..=5).map(|id| vec![SavedKey::Int(id)]);
    write_tree_values(
        &project,
        &place,
        identities,
        &field_path(&place, "value"),
        b"42",
    );
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");
    let root = [DataPathSegment::Root("counter".into())];
    let path = resolve_data_path(&program, &root)
        .expect("resolve root path")
        .expect("root path");

    let expected: Vec<String> = (1..=5).map(|id| format!("^counter({id}).value")).collect();
    let mut collected = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let resume = cursor.as_ref().map(|segments: &Vec<DataPathSegment>| {
            resolve_data_path(&program, segments)
                .expect("resolve cursor path")
                .expect("cursor path")
        });
        let page =
            walk_data(&program, &store, &path, resume.as_ref(), 2).expect("walk a saved-data page");
        pages += 1;
        collected.extend(page.entries.iter().map(|entry| entry.path.clone()));
        match page.next_cursor_path {
            Some(next) => {
                assert!(
                    page.truncated,
                    "a page that yields a cursor must report truncation"
                );
                cursor = Some(next.segments().to_vec());
            }
            None => {
                assert!(!page.truncated, "the final page must not report truncation");
                break;
            }
        }
    }

    assert_eq!(
        collected, expected,
        "resumed walk must return every record value once"
    );
    assert!(
        pages >= 3,
        "a limit of two over five records must take several pages"
    );
}

/// Record-children listing under a multi-record root is paged the same way: a small
/// limit truncates the first page and hands back a key cursor, and resuming from it
/// returns the remaining record keys with no gap or overlap.
#[test]
fn shared_data_children_resumes_across_key_pages_exactly_once() {
    let project = native_project("data-children-resume-pages");
    let place = checked_place(&project, "counter");
    let identities = (1..=5).map(|id| vec![SavedKey::Int(id)]);
    write_tree_values(
        &project,
        &place,
        identities,
        &field_path(&place, "value"),
        b"42",
    );
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");
    let root = [DataPathSegment::Root("counter".into())];

    let mut collected = Vec::new();
    let mut resume: Option<SavedKey> = None;
    let mut pages = 0;
    loop {
        let page = data_children(&program, &store, &root, 2, resume.as_ref())
            .expect("list a record-children page");
        pages += 1;
        for child in &page.children {
            match child {
                DataChild::Key(key) => collected.push(key.clone()),
                other => panic!("record children must be keys, got {other:?}"),
            }
        }
        match page.cursor {
            Some(cursor) => {
                assert!(
                    page.truncated,
                    "a page that yields a cursor must report truncation"
                );
                resume = Some(cursor);
            }
            None => {
                assert!(!page.truncated, "the final page must not report truncation");
                break;
            }
        }
    }

    let expected: Vec<SavedKey> = (1..=5).map(SavedKey::Int).collect();
    assert_eq!(
        collected, expected,
        "resumed children must return every record key once"
    );
    assert!(
        pages >= 3,
        "a limit of two over five records must take several pages"
    );
}

fn temporal_key_project(name: &str) -> support::TempProject {
    let project = support::temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Event\n\
             \x20   notes(day: date): string\n\
             store ^events(day: date): Event\n",
        );
    });
    support::commit_catalog_if_clean(&project);
    project
}

fn assert_store_corruption(error: ToolingError) {
    match error {
        ToolingError::Store(error) => assert_eq!(error.code(), "store.corruption", "{error:?}"),
        other => panic!("expected store corruption, got {other:?}"),
    }
}

#[test]
fn tooling_rejects_malformed_temporal_root_keys() {
    let project = temporal_key_project("data-tooling-malformed-temporal-root");
    write_record_presence(&project, "events", &[SavedKey::Date(0)]);
    write_record_presence(
        &project,
        "events",
        &[SavedKey::Date(SUPPORTED_DATE_MAX_DAYS + 1)],
    );
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");
    let root = [DataPathSegment::Root("events".into())];

    assert_store_corruption(
        data_children(&program, &store, &root, 10, None)
            .expect_err("children rejects malformed root key"),
    );
    let path = resolve_data_path(&program, &root)
        .expect("resolve root path")
        .expect("root path");
    let read = read_data_path(&store, &path).expect("read uses bounded root child presence");
    assert_eq!(read.presence, DataPresence::ChildrenOnly);
    assert!(read.payload.is_none(), "{read:?}");
    assert_store_corruption(
        walk_data(&program, &store, &path, None, 10).expect_err("walk rejects malformed root key"),
    );
    assert_eq!(
        count_integrity_problems(&store, &program)
            .expect_err("integrity rejects malformed root key")
            .code(),
        "store.corruption"
    );
}

#[test]
fn tooling_rejects_malformed_temporal_layer_keys() {
    let project = temporal_key_project("data-tooling-malformed-temporal-layer");
    let place = checked_place(&project, "events");
    let malformed = SavedKey::Date(SUPPORTED_DATE_MAX_DAYS + 1);
    write_tree_value(
        &project,
        "events",
        &[SavedKey::Date(0)],
        &keyed_field_path(&place, "notes", SavedKey::Date(0)),
        b"valid".to_vec(),
    );
    write_tree_value(
        &project,
        "events",
        &[SavedKey::Date(0)],
        &keyed_field_path(&place, "notes", malformed),
        b"x".to_vec(),
    );
    let program = checked_program(&project);
    let store =
        TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open native store");
    let layer = [
        DataPathSegment::Root("events".into()),
        DataPathSegment::Key(SavedKey::Date(0)),
        DataPathSegment::Layer("notes".into()),
    ];

    assert_store_corruption(
        data_children(&program, &store, &layer, 10, None)
            .expect_err("children rejects malformed layer key"),
    );
    let path = resolve_data_path(&program, &layer)
        .expect("resolve layer path")
        .expect("layer path");
    let read = read_data_path(&store, &path).expect("read uses bounded layer child presence");
    assert_eq!(read.presence, DataPresence::ChildrenOnly);
    assert!(read.payload.is_none(), "{read:?}");
    assert_store_corruption(
        walk_data(&program, &store, &path, None, 10).expect_err("walk rejects malformed layer key"),
    );
    assert_eq!(
        count_integrity_problems(&store, &program)
            .expect_err("integrity rejects malformed layer key")
            .code(),
        "store.corruption"
    );
}

#[test]
fn data_integrity_passes_on_a_healthy_seeded_project() {
    // Render contract: the text format prints a human `integrity verified` line that
    // counts field cells, the stored `(path, value)` pairs it decodes. The seeded
    // fixture has one field cell. The typed empty problem list is asserted elsewhere.
    let (_project, dir) = seeded_project("data-integrity-ok");
    let output = marrow(&["data", "integrity", &dir]);
    let json_output = marrow(&["data", "integrity", "--format", "json", &dir]);
    let jsonl_output = marrow(&["data", "integrity", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("integrity verified (1 cells)"), "{stdout}");

    assert_eq!(json_output.status.code(), Some(0), "{json_output:?}");
    let value = support::json(json_output.stdout);
    assert_eq!(value["cells"], serde_json::json!(1));
    assert!(value.get("records").is_none(), "{value}");
    assert_store_snapshot(&value);

    assert_eq!(jsonl_output.status.code(), Some(0), "{jsonl_output:?}");
    let stdout = String::from_utf8(jsonl_output.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    let summary: serde_json::Value = serde_json::from_str(lines[0]).expect("summary json");
    assert_eq!(summary["kind"], serde_json::json!("summary"));
    assert_eq!(summary["cells"], serde_json::json!(1));
    assert_store_snapshot(&summary);
}

#[test]
fn data_integrity_reads_backup_while_live_store_is_locked() {
    let (project, dir) = seeded_project("data-integrity-backup");
    let archive = support::backup_artifact(&project, "counter.mwbackup");
    let archive_arg = archive.to_str().expect("backup path utf8");

    let live = support::marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(live.status.code(), Some(0), "{live:?}");
    let live = support::json(live.stdout);

    let _writer = TreeStore::open(&project.join(".data").join("marrow.redb"))
        .expect("hold the native writer open");
    let backup = support::marrow(&[
        "data",
        "integrity",
        "--backup",
        archive_arg,
        "--format",
        "json",
        &dir,
    ]);

    assert_eq!(backup.status.code(), Some(0), "{backup:?}");
    let backup = support::json(backup.stdout);
    assert_eq!(backup["project"], live["project"]);
    assert_eq!(backup["cells"], live["cells"]);
    assert_eq!(backup["problems"], live["problems"]);
    assert_stable_store_snapshot_eq(&backup, &live);
}

#[test]
fn data_integrity_reports_required_field_completeness_and_repair() {
    let (project, dir) = seeded_project("data-integrity-incomplete-repair");
    let place = checked_place(&project, "counter");
    let store_catalog_id = place.store_catalog_id.clone().expect("accepted store id");
    let value_id = member_path_catalog_id(&place, &["value"]);
    let value_path = field_path(&place, "value");

    delete_tree_path(&project, "counter", &[SavedKey::Int(1)], &value_path);

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.incomplete");
    assert_eq!(problem["kind"], serde_json::json!("tooling"), "{value}");
    assert_eq!(
        problem["store_catalog_id"],
        serde_json::json!(store_catalog_id),
        "{value}"
    );
    assert_eq!(
        problem["record_identity"],
        serde_json::json!([int_key_json(1)]),
        "{value}"
    );
    assert_eq!(
        problem["missing_member_catalog_id"],
        serde_json::json!(value_id.as_str()),
        "{value}"
    );
    assert_eq!(problem["parent_path"], serde_json::json!([]), "{value}");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^counter(1).value"),
        "{value}"
    );

    write_tree_value(
        &project,
        "counter",
        &[SavedKey::Int(1)],
        &value_path,
        b"42".to_vec(),
    );
    let repaired = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(repaired.status.code(), Some(0), "{repaired:?}");
    assert_eq!(json(repaired)["problems"], serde_json::json!([]));
}

#[test]
fn data_integrity_reports_missing_required_child_per_keyed_entry() {
    let project = support::temp_dir("data-integrity-incomplete-keyed-entry");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Log\n\
         \x20\x20\x20\x20sessions(day: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required note: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20mood: string\n\
         store ^logs(id: int): Log\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "logs");
    let store_catalog_id = place.store_catalog_id.clone().expect("accepted store id");
    let sessions_id = member_path_catalog_id(&place, &["sessions"]);
    let note_id = member_path_catalog_id(&place, &["sessions", "note"]);
    let mood_id = member_path_catalog_id(&place, &["sessions", "mood"]);
    let mood_path = vec![
        StoreDataPathSegment::Member(sessions_id.clone()),
        StoreDataPathSegment::Key(SavedKey::Int(7)),
        StoreDataPathSegment::Member(mood_id),
    ];
    write_record_presence(&project, "logs", &[SavedKey::Int(1)]);
    write_tree_value(
        &project,
        "logs",
        &[SavedKey::Int(1)],
        &mood_path,
        b"calm".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.incomplete");
    assert_eq!(
        problem["store_catalog_id"],
        serde_json::json!(store_catalog_id),
        "{value}"
    );
    assert_eq!(
        problem["record_identity"],
        serde_json::json!([int_key_json(1)]),
        "{value}"
    );
    assert_eq!(
        problem["parent_path"],
        serde_json::json!([
            { "member_catalog_id": sessions_id.as_str() },
            { "key": int_key_json(7) }
        ]),
        "{value}"
    );
    assert_eq!(
        problem["missing_member_catalog_id"],
        serde_json::json!(note_id.as_str()),
        "{value}"
    );
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^logs(1).sessions(7).note"),
        "{value}"
    );
}

#[test]
fn data_integrity_accepts_a_keyed_group_entry_node() {
    let project = support::temp_dir("data-integrity-keyed-group-node");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Post\n\
         \x20\x20\x20\x20markers(seq: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         store ^posts(id: int): Post\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "posts");
    let marker_id = member_path_catalog_id(&place, &["markers"]);
    let marker_path = vec![
        StoreDataPathSegment::Member(marker_id),
        StoreDataPathSegment::Key(SavedKey::Int(1)),
    ];

    write_tree_node(&project, "posts", &[SavedKey::Int(1)], &marker_path);

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json(output)["problems"], serde_json::json!([]));
}

#[test]
fn data_integrity_reports_path_nodes_outside_keyed_group_entries() {
    let project = support::temp_dir("data-integrity-invalid-path-nodes");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Item\n\
         \x20\x20\x20\x20label: string\n\
         \x20\x20\x20\x20tags(seq: int): string\n\
         \x20\x20\x20\x20meta\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         store ^items(id: int): Item\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "items");
    let label_path = vec![StoreDataPathSegment::Member(member_path_catalog_id(
        &place,
        &["label"],
    ))];
    let tag_path = vec![
        StoreDataPathSegment::Member(member_path_catalog_id(&place, &["tags"])),
        StoreDataPathSegment::Key(SavedKey::Int(1)),
    ];
    let meta_path = vec![StoreDataPathSegment::Member(member_path_catalog_id(
        &place,
        &["meta"],
    ))];

    write_record_presence(&project, "items", &[SavedKey::Int(1)]);
    write_tree_node(&project, "items", &[SavedKey::Int(1)], &label_path);
    write_tree_node(&project, "items", &[SavedKey::Int(1)], &tag_path);
    write_tree_node(&project, "items", &[SavedKey::Int(1)], &meta_path);

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problems = value["problems"].as_array().expect("problems");
    assert_eq!(problems.len(), 3, "{value}");
    assert!(
        problems
            .iter()
            .all(|problem| problem["code"] == serde_json::json!("data.orphan")),
        "{value}"
    );
}

#[test]
fn data_integrity_does_not_require_missing_optional_fields() {
    let project = support::temp_dir("data-integrity-optional-missing");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         \x20\x20\x20\x20label: string\n\
         store ^counter(id: int): Counter\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    write_record_presence(&project, "counter", &[SavedKey::Int(1)]);
    write_tree_value(
        &project,
        "counter",
        &[SavedKey::Int(1)],
        &field_path(&place, "value"),
        b"42".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json(output)["problems"], serde_json::json!([]));
}

#[test]
fn data_integrity_skips_pending_required_members_without_accepted_ids() {
    let (project, _dir) = seeded_project("data-integrity-pending-required");
    write(
        &project,
        "src/app.mw",
        "module app\n\
         \n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         \x20\x20\x20\x20required label: string\n\
         store ^counter(id: int): Counter\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var c: Counter\n\
         \x20\x20\x20\x20c.value = 42\n\
         \x20\x20\x20\x20c.label = \"ok\"\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n",
    );

    let (_records, problems) = check_pending_source_against_accepted_store(&project);

    assert_eq!(problems, 0);
}

#[test]
fn data_integrity_skips_defaulted_required_members_without_accepted_ids() {
    let (project, _dir) = seeded_project("data-integrity-defaulted-required");
    write(
        &project,
        "src/app.mw",
        "module app\n\
         \n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         \x20\x20\x20\x20required label: string\n\
         store ^counter(id: int): Counter\n\
         \n\
         evolve\n\
         \x20\x20\x20\x20default Counter.label = \"unknown\"\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var c: Counter\n\
         \x20\x20\x20\x20c.value = 42\n\
         \x20\x20\x20\x20c.label = \"ok\"\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n",
    );

    let (_records, problems) = check_pending_source_against_accepted_store(&project);

    assert_eq!(problems, 0);
}

#[test]
fn data_integrity_reports_deleted_defaulted_required_field_after_apply()
-> Result<(), Box<dyn std::error::Error>> {
    let project = native_books_project(
        "data-integrity-defaulted-required-after-apply",
        REQUIRED_BASELINE_SOURCE,
    );
    let baseline = commit_catalog(&project);
    let baseline_place = root_place(&baseline, "books")?;
    {
        let store = open_native_store(&project);
        seed_title_only(&store, &baseline_place, 1, "Dune");
    }
    write(&project, "src/books.mw", REQUIRED_DEFAULT_SOURCE);
    let dir = project.to_str().unwrap().to_string();
    let apply = support::marrow(&["evolve", "apply", "--format", "json", &dir]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    let place = checked_place(&project, "books");
    let store_catalog_id = place.store_catalog_id.clone().expect("accepted store id");
    let pages_id = member_path_catalog_id(&place, &["pages"]);
    let pages_path = field_path(&place, "pages");
    delete_tree_path(&project, "books", &[SavedKey::Int(1)], &pages_path);

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.incomplete");
    assert_eq!(
        problem["store_catalog_id"],
        serde_json::json!(store_catalog_id),
        "{value}"
    );
    assert_eq!(
        problem["record_identity"],
        serde_json::json!([int_key_json(1)]),
        "{value}"
    );
    assert_eq!(
        problem["missing_member_catalog_id"],
        serde_json::json!(pages_id.as_str()),
        "{value}"
    );
    assert_eq!(problem["parent_path"], serde_json::json!([]), "{value}");

    Ok(())
}

#[test]
fn data_json_commands_report_catalog_intent_not_store_corruption() {
    let (project, dir) = seeded_project("data-integrity-catalog-intent-json");
    write(
        &project,
        "src/app.mw",
        "module app\n\
         \n\
         resource Counter\n\
         \x20\x20\x20\x20a: int\n\
         \x20\x20\x20\x20b: int\n\
         \x20\x20\x20\x20c: int\n\
         store ^counter(id: int): Counter\n\
         \n\
         evolve\n\
         \x20\x20\x20\x20rename Counter.a -> Counter.c\n\
         \x20\x20\x20\x20rename Counter.b -> Counter.c\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var c: Counter\n\
         \x20\x20\x20\x20c.a = 1\n\
         \x20\x20\x20\x20c.b = 2\n\
         \x20\x20\x20\x20c.c = 3\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n",
    );

    for args in [
        vec!["data", "roots", "--format", "json", &dir],
        vec!["data", "stats", "--format", "json", &dir],
        vec!["data", "dump", "--format", "json", &dir],
        vec!["data", "get", "--format", "json", &dir, "^counter(1).a"],
        vec!["data", "integrity", "--format", "json", &dir],
    ] {
        let output = support::marrow(&args);

        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        let value = support::json(output.stdout);
        let diagnostics = value["diagnostics"].as_array().expect("diagnostics");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == serde_json::json!("check.catalog_intent")),
            "{args:?}: {value}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !value.to_string().contains("store.corruption") && !stderr.contains("store.corruption"),
            "catalog-intent state must not render as store corruption for {args:?}: stdout={value} stderr={stderr}"
        );
    }
}

#[test]
fn data_integrity_accepts_singleton_fields_and_keyed_tree_members() {
    let project = support::temp_dir("data-integrity-singleton-members");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         use std::clock\n\n\
         resource Settings\n\
         \x20\x20\x20\x20maxLoans: int\n\
         \x20\x20\x20\x20theme: string\n\
         store ^settings: Settings\n\n\
         resource Hits\n\
         \x20\x20\x20\x20when(moment: instant): int\n\
         store ^hits: Hits\n\n\
         pub fn seed()\n\
         \x20\x20\x20\x20^settings.maxLoans = 5\n\
         \x20\x20\x20\x20^settings.theme = \"dark\"\n\
         \x20\x20\x20\x20^hits.when(std::clock::parseInstant(\"2020-01-01T00:00:00Z\")) = 1\n",
    );
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json(output)["problems"], serde_json::json!([]));
}

#[test]
fn data_integrity_reports_an_undeclared_store_cell_as_data_orphan() {
    let (project, dir) = seeded_project("data-integrity-orphan");
    // A data cell under a store catalog id the schema does not declare: a dropped
    // root left it behind. The declared-cell walk never visits it, so only the
    // actual-cell orphan scan catches it.
    write_orphan_cell(
        &project,
        "cat_000000000000000000000000deadbeef",
        "cat_00000000000000000000000000000001",
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.orphan");
    assert_eq!(problem["kind"], serde_json::json!("tooling"), "{value}");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("<undeclared saved root>")
    );
    let text = value.to_string();
    assert!(
        !text.contains("deadbeef") && !text.contains("cat_"),
        "{value}"
    );
    assert_eq!(
        problem["help"],
        serde_json::json!(
            "run `marrow data integrity` after source-native evolution or maintenance repair"
        )
    );
}

#[test]
fn data_integrity_reports_an_undeclared_member_cell_as_data_orphan() {
    let (project, dir) = seeded_project("data-integrity-orphan-member");
    // The store id is the real one, but the member catalog id is undeclared: a
    // dropped field left this cell behind.
    let place = checked_place(&project, "counter");
    let store_catalog_id = place.store_catalog_id.expect("accepted store id");
    write_orphan_cell(
        &project,
        &store_catalog_id,
        "cat_000000000000000000000000cafef00d",
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.orphan");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^counter(1).<undeclared member>")
    );
    let text = value.to_string();
    assert!(
        !text.contains("cafef00d") && !text.contains("cat_"),
        "{value}"
    );
}

#[test]
fn data_integrity_reports_a_leaf_without_its_record_presence_as_data_orphan() {
    let project = native_project("data-integrity-leaf-without-node");
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    write_tree_value_without_node(
        &project,
        "counter",
        &[SavedKey::Int(1)],
        &field_path(&place, "value"),
        b"leaf without node".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.orphan");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^counter(1).value")
    );
}

#[test]
fn data_integrity_reports_an_extra_key_below_a_scalar_field_as_data_orphan() {
    let (project, dir) = seeded_project("data-integrity-orphan-extra-key");
    let place = checked_place(&project, "counter");
    let mut path = field_path(&place, "value");
    path.push(StoreDataPathSegment::Key(SavedKey::Int(99)));
    write_tree_value(
        &project,
        "counter",
        &[SavedKey::Int(1)],
        &path,
        b"7".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.orphan");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^counter(1).value(99)")
    );
}

#[test]
fn data_integrity_reports_a_keyed_member_value_without_its_key_as_data_orphan() {
    let project = support::temp_dir("data-integrity-orphan-missing-key");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Hits\n\
         \x20\x20\x20\x20when(moment: instant): int\n\
         store ^hits: Hits\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "hits");
    write_tree_value(
        &project,
        "hits",
        &[],
        &field_path(&place, "when"),
        b"7".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.orphan");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^hits.when")
    );
}

#[test]
fn data_integrity_reports_a_non_canonical_value_as_data_decode() {
    let project = native_project("data-integrity-decode");
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    write_tree_value(
        &project,
        "counter",
        &[SavedKey::Int(1)],
        &field_path(&place, "value"),
        b"+1".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.decode");
    assert_eq!(problem["kind"], serde_json::json!("tooling"), "{value}");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^counter(1).value")
    );
    assert_eq!(
        value["problems"].as_array().expect("problems").len(),
        1,
        "{value}"
    );
}

#[test]
fn data_integrity_reports_a_corrupt_identity_leaf_as_data_decode() {
    let project = support::temp_dir("data-integrity-identity");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Author\n\
         \x20\x20\x20\x20required name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20authorId: Id(^authors)\n\
         store ^books(id: int): Book\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "books");
    let mut corrupt = encode_identity_keys(&[SavedKey::Int(7)]);
    corrupt.push(0xFF);
    write_tree_value(
        &project,
        "books",
        &[SavedKey::Int(1)],
        &field_path(&place, "authorId"),
        corrupt,
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.decode");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^books(1).authorId")
    );
}

#[test]
fn data_integrity_reports_a_wrong_typed_identity_leaf_as_data_key_type() {
    let project = support::temp_dir("data-integrity-identity-key-type");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Author\n\
         \x20\x20\x20\x20required name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20authorId: Id(^authors)\n\
         store ^books(id: int): Book\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "books");
    let wrong_typed = encode_identity_keys(&[SavedKey::Str("not-an-int".into())]);
    write_tree_value(
        &project,
        "books",
        &[SavedKey::Int(1)],
        &field_path(&place, "authorId"),
        wrong_typed,
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.key_type");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^books(1).authorId")
    );
    // The identity-reference key mismatch names both scalars in the same surface
    // convention as every other type-naming diagnostic: lowercase, backticked, with
    // the grammatical indefinite article, never the internal capitalized identifier.
    assert_eq!(
        problem["message"],
        serde_json::json!(
            "stored `Id(^authors)` reference has a `string` key where the schema declares an `int`"
        )
    );
}

#[test]
fn data_integrity_reports_a_dangling_identity_leaf_reference() {
    let project = support::temp_dir("data-integrity-dangling-ref");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Author\n\
         \x20\x20\x20\x20name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n\
         \x20\x20\x20\x20authorId: Id(^authors)\n\
         store ^books(id: int): Book\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let book_place = checked_place(&project, "books");
    let author_id = member_path_catalog_id(&book_place, &["authorId"]);
    write_tree_value(
        &project,
        "books",
        &[SavedKey::Int(1)],
        &field_path(&book_place, "authorId"),
        encode_identity_keys(&[SavedKey::Int(7)]),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.dangling_ref");
    assert_eq!(problem["kind"], serde_json::json!("tooling"), "{value}");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^books(1).authorId"),
        "{value}"
    );
    assert_eq!(
        problem["containing_identity"],
        serde_json::json!([int_key_json(1)]),
        "{value}"
    );
    assert_eq!(
        problem["field_catalog_id"],
        serde_json::json!(author_id.as_str()),
        "{value}"
    );
    assert_eq!(
        problem["referenced_root"],
        serde_json::json!("authors"),
        "{value}"
    );
    assert_eq!(
        problem["referenced_identity"],
        serde_json::json!([int_key_json(7)]),
        "{value}"
    );

    write_record_presence(&project, "authors", &[SavedKey::Int(7)]);
    let repaired = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(repaired.status.code(), Some(0), "{repaired:?}");
    assert_eq!(json(repaired)["problems"], serde_json::json!([]));
}

#[test]
fn activation_integrity_count_excludes_dangling_identity_leaf_reference() {
    let project = support::temp_dir("activation-integrity-dangling-ref-report-only");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Author\n\
         \x20\x20\x20\x20name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n\
         \x20\x20\x20\x20authorId: Id(^authors)\n\
         store ^books(id: int): Book\n",
    );
    let book_place = checked_place(&project, "books");
    write_tree_value(
        &project,
        "books",
        &[SavedKey::Int(1)],
        &field_path(&book_place, "authorId"),
        encode_identity_keys(&[SavedKey::Int(7)]),
    );
    let program = checked_program(&project);
    let store = TreeStore::open_read_only(&project.join(".data").join("marrow.redb"))
        .expect("open store read-only");

    assert_eq!(
        count_integrity_problems(&store, &program).expect("count data integrity problems"),
        (1, 1)
    );
    assert_eq!(
        count_activation_integrity_problems(&store, &program)
            .expect("count activation integrity problems"),
        (1, 0)
    );
}

#[test]
fn data_integrity_reports_a_wrong_typed_keyed_member_key_as_data_key_type() {
    let project = support::temp_dir("data-integrity-layer-key-type");
    write(&project, "marrow.json", NATIVE_STORE_CONFIG);
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Hits\n\
         \x20\x20\x20\x20when(moment: instant): int\n\
         store ^hits: Hits\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "hits");
    write_tree_value(
        &project,
        "hits",
        &[],
        &keyed_field_path(&place, "when", SavedKey::Str("not-an-instant".into())),
        b"7".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    let problem = integrity_problem(&value, "data.key_type");
    assert_eq!(
        problem["source_span"]["path"],
        serde_json::json!("^hits.when(\"not-an-instant\")")
    );
    // The message names both scalars in the surface convention: lowercase,
    // backticked, with the grammatical indefinite article, never the internal
    // capitalized identifier.
    assert_eq!(
        problem["message"],
        serde_json::json!("stored key is a `string` where the schema declares an `instant`")
    );
}

#[test]
fn data_integrity_reports_a_wrong_typed_record_key_as_data_key_type() {
    let project = native_project("data-integrity-key-type");
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    write_tree_value(
        &project,
        "counter",
        &[SavedKey::Str("oops".into())],
        &field_path(&place, "value"),
        b"7".to_vec(),
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output);
    integrity_problem(&value, "data.key_type");
}

/// Build and seed a project with a non-unique `byShelf(shelf, id)` index over enough
/// books that the index spans several store pages, the shape the silent-truncation
/// finding reproduces. The records are written through the production `run` pipeline,
/// so the index family is populated exactly as a real write path leaves it.
fn by_shelf_project(name: &str, books: i64) -> support::TempProject {
    let mut seed = String::from("pub fn seed()\n    transaction\n");
    for id in 1..=books {
        // Two shelves keep several keys per shelf so a dropped range hides many rows.
        let shelf = if id % 2 == 0 { "fiction" } else { "history" };
        seed.push_str(&format!(
            "        ^books({id}).title = \"t{id}\"\n        ^books({id}).shelf = \"{shelf}\"\n"
        ));
    }
    let project = support::temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            &format!(
                "module shelf\n\n\
                 resource Book\n\
                 \x20\x20\x20\x20required title: string\n\
                 \x20\x20\x20\x20shelf: string\n\
                 store ^books(id: int): Book\n\n\
                 \x20\x20\x20\x20index byShelf(shelf, id)\n\n\
                 pub fn count_fiction()\n\
                 \x20\x20\x20\x20var c = 0\n\
                 \x20\x20\x20\x20for id in keys(^books.byShelf(\"fiction\"))\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20c = c + 1\n\
                 \x20\x20\x20\x20print($\"{{c}}\")\n\n\
                 {seed}"
            ),
        );
    });
    let seed = marrow(&["run", project.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    project
}

/// The catalog id of one named index on a place, the id the index family is keyed by.
fn index_catalog_id(project: &Path, root: &str, index: &str) -> marrow_store::cell::CatalogId {
    let place = checked_place(project, root);
    let raw = place
        .indexes
        .iter()
        .find(|candidate| candidate.name == index)
        .and_then(|candidate| candidate.catalog_id.clone())
        .expect("index has a catalog id");
    marrow_store::cell::CatalogId::new(raw).expect("index catalog id is well-formed")
}

/// A single-byte flip in an index page-structure region can make the backend's range
/// scan silently truncate, dropping committed index entries from every enumeration
/// while leaving the data records intact. The completeness cross-check derives the
/// expected entry count from the data family, so dropping entries the records still
/// imply must fail `data integrity` closed as `store.corruption` rather than report a
/// store that an index-driven read would silently under-return.
#[test]
fn data_integrity_fails_closed_when_index_entries_are_silently_dropped() {
    let project = by_shelf_project("data-integrity-index-truncated", 60);
    let dir = project.to_str().unwrap().to_string();

    // A clean store verifies and an index-driven read returns the full row set.
    let clean = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(
        clean.status.code(),
        Some(0),
        "clean store verifies: {clean:?}"
    );

    // Resolve the index id, store id, and title path before opening any store handle:
    // `checked_place` opens the store write-capable, so it must not race a held handle.
    let store_path = project.join(".data").join("marrow.redb");
    let by_shelf = index_catalog_id(&project, "books", "byShelf");
    let place = checked_place(&project, "books");
    let store_id =
        marrow_store::cell::CatalogId::new(place.store_catalog_id.clone().unwrap()).unwrap();
    let title_path = field_path(&place, "title");

    {
        let store = TreeStore::open(&store_path).expect("open seeded store");
        store.begin().expect("begin");
        // Drop the byShelf entries for several fiction records, the rows a truncated
        // range scan would hide. The data records themselves are left untouched.
        for id in [2i64, 4, 6, 8, 10] {
            store
                .delete_index_entry(
                    &by_shelf,
                    &[SavedKey::Str("fiction".into()), SavedKey::Int(id)],
                    &[SavedKey::Int(id)],
                )
                .expect("drop index entry");
        }
        store.commit().expect("commit dropped entries");
    }

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "dropped index entries must fail integrity closed: {output:?}"
    );
    let value = json(output);
    assert_eq!(
        value["code"],
        serde_json::json!("store.corruption"),
        "{value}"
    );

    // The data records are intact: the failure is the missing index entries, not lost data.
    {
        let store = TreeStore::open_read_only(&store_path).expect("reopen store read-only");
        assert!(
            store
                .read_data_value(&store_id, &[SavedKey::Int(2)], &title_path)
                .expect("read record")
                .is_some(),
            "the dropped index entry leaves its data record in place"
        );
    }

    // Recover must not bless the index-incomplete store either.
    let recover = marrow(&["data", "recover", "--format", "json", &dir]);
    assert_eq!(
        recover.status.code(),
        Some(1),
        "recover must fail closed on a silently-truncated index: {recover:?}"
    );
    assert_eq!(
        support::json(recover.stdout)["code"],
        serde_json::json!("store.corruption"),
    );
}

/// Backup carries data cells and rebuilds the index family on restore, so it must not
/// archive a store whose committed index entries were silently dropped: a backup of an
/// index-incomplete store would mask the under-read. Backup fails closed before writing
/// the artifact.
#[test]
fn backup_fails_closed_when_index_entries_are_silently_dropped() {
    let project = by_shelf_project("backup-index-truncated", 40);
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let by_shelf = index_catalog_id(&project, "books", "byShelf");
    {
        let store =
            TreeStore::open(&project.join(".data").join("marrow.redb")).expect("open seeded store");
        store.begin().expect("begin");
        for id in [2i64, 4, 6] {
            store
                .delete_index_entry(
                    &by_shelf,
                    &[SavedKey::Str("fiction".into()), SavedKey::Int(id)],
                    &[SavedKey::Int(id)],
                )
                .expect("drop index entry");
        }
        store.commit().expect("commit dropped entries");
    }

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(
        backup.status.code(),
        Some(1),
        "backup must fail closed on a silently-truncated index: {backup:?}"
    );
    assert!(
        !archive.exists(),
        "a failed backup leaves no archive for the index-incomplete store"
    );
}

/// The number of `fiction` rows an index-driven read returns, the count the finding
/// watched silently collapse. `count_fiction` walks `^books.byShelf("fiction")`, so it
/// reads through the index exactly as a user query would.
fn fiction_row_count(dir: &str) -> Option<i64> {
    // Invoke the binary directly: the `support_data::marrow` wrapper re-commits the
    // catalog in-process first, which would open a flipped store and panic instead of
    // letting the subprocess report the typed store error this sweep inspects.
    let run = support::marrow(&["run", "--entry", "shelf::count_fiction", dir]);
    if run.status.code() != Some(0) {
        return None;
    }
    String::from_utf8(run.stdout).ok()?.trim().parse().ok()
}

/// A real single-byte flip in the store body must never leave `data integrity`
/// reporting a verified, problem-free store while an index-driven read silently
/// under-returns. This flips one byte at a sampling of offsets across the high and low
/// regions where index pages live and, for each, requires the fail-closed invariant:
/// either `data integrity` reports the damage as `store.corruption`, or the index
/// still returns the full row set. A flip that silently truncates an index range would
/// otherwise pass the structural check yet drop rows — exactly what the completeness
/// cross-check now catches. The deterministic counterpart that drops index entries
/// directly is `data_integrity_fails_closed_when_index_entries_are_silently_dropped`;
/// this one proves the same closure against actual file corruption.
#[test]
fn data_integrity_never_blesses_an_index_that_silently_under_reads() {
    let project = by_shelf_project("data-integrity-index-flip-sweep", 80);
    let dir = project.to_str().unwrap().to_string();
    let store_path = project.join(".data").join("marrow.redb");

    let full = fiction_row_count(&dir).expect("clean index read");
    assert_eq!(full, 40, "fixture seeds 40 fiction rows");
    let clean = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(
        clean.status.code(),
        Some(0),
        "clean store verifies: {clean:?}"
    );

    let pristine = fs::read(&store_path).expect("read store bytes");
    let len = pristine.len();
    assert!(len > 4096, "seeded multi-page store: {len} bytes");

    // Sample offsets across the body past the header page: the tail holds the higher
    // index page-structure region and the mid-body the lower one. A bounded sample
    // keeps the end-to-end flip coverage fast while still landing in both regions.
    let body = len - 4096;
    let offsets: Vec<usize> = (0..24).map(|i| 4096 + body * i / 24).collect();
    for offset in offsets {
        for bit in [0x01u8, 0x80] {
            let mut bytes = pristine.clone();
            bytes[offset] ^= bit;
            fs::write(&store_path, &bytes).expect("write flipped store");

            let integrity = support::marrow_bounded(
                &["data", "integrity", "--format", "json", &dir],
                std::time::Duration::from_secs(20),
            );
            // A flip redb cannot even open is fail-closed too; the forbidden outcome is
            // an exit-0 verified verdict over an index that no longer returns every row.
            if integrity.status.code() == Some(0) {
                let value = support::json(integrity.stdout);
                assert_eq!(
                    value["problems"],
                    serde_json::json!([]),
                    "an exit-0 integrity verdict reports no problems: offset {offset:#x}"
                );
                // A read that fails to open is itself fail-closed and acceptable — a
                // write-capable open can surface damage a read-only verdict did not.
                if let Some(read) = fiction_row_count(&dir) {
                    assert_eq!(
                        read, full,
                        "integrity verified a store at offset {offset:#x} whose index read under-returns"
                    );
                }
            }
        }
    }

    fs::write(&store_path, &pristine).expect("restore pristine store");
}
