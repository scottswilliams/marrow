//! `marrow data integrity`: the saved-data integrity verdicts. Problems are asserted
//! by typed diagnostic code, tooling kind, and structured payloads. Display paths
//! are checked only where the rendered operator path is the contract. The shared
//! child-page limit guard is asserted on its typed query error.

use std::fs;
use std::path::Path;

use crate::support;
use crate::support_data;
use crate::support_evolve;
use marrow_check::tooling::{
    DataQuerySegment, QueryError, ToolingError, count_activation_integrity_problems,
    count_integrity_problems, data_children,
};
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use support::write;
use support_data::{
    checked_place, checked_program, delete_tree_path, encode_identity_keys, field_path,
    integrity_problem, json, keyed_field_path, marrow, member_path_catalog_id, native_project,
    seeded_project, write_orphan_cell, write_record_node, write_tree_value,
    write_tree_value_without_node,
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
        &[DataQuerySegment::Root("counter".into())],
        0,
        None,
    )
    .expect_err("shared child pages reject a zero limit");

    assert!(
        matches!(error, ToolingError::Query(QueryError::ZeroLimit)),
        "expected a typed zero-limit query error, got {error:?}"
    );
}

#[test]
fn data_integrity_passes_on_a_healthy_seeded_project() {
    // Render contract: the text format prints a human `integrity verified` line. The
    // typed empty problem list is asserted by `data_commands_page_through_large_native_store`
    // and `data_integrity_accepts_singleton_fields_and_keyed_tree_members`.
    let (_project, dir) = seeded_project("data-integrity-ok");
    let output = marrow(&["data", "integrity", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("integrity verified"), "{stdout}");
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
    assert_eq!(support::json(backup.stdout), live);
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
        DataPathSegment::Member(sessions_id.clone()),
        DataPathSegment::Key(SavedKey::Int(7)),
        DataPathSegment::Member(mood_id),
    ];
    write_record_node(&project, "logs", &[SavedKey::Int(1)]);
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
    write_record_node(&project, "counter", &[SavedKey::Int(1)]);
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
fn data_integrity_reports_deleted_defaulted_required_field_after_apply() {
    let project = native_books_project(
        "data-integrity-defaulted-required-after-apply",
        REQUIRED_BASELINE_SOURCE,
    );
    let baseline = commit_catalog(&project);
    let baseline_place = root_place(&baseline, "books");
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
fn data_integrity_reports_a_leaf_without_its_record_node_as_data_orphan() {
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
    path.push(DataPathSegment::Key(SavedKey::Int(99)));
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
fn data_integrity_reports_an_orphan_problem_with_a_tooling_kind() {
    let (project, dir) = seeded_project("data-integrity-orphan-json");
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
        serde_json::json!("<undeclared saved root>"),
        "{value}"
    );
    let text = value.to_string();
    assert!(
        !text.contains("deadbeef") && !text.contains("cat_"),
        "{text}"
    );
    assert_eq!(
        problem["help"],
        serde_json::json!(
            "run `marrow data integrity` after source-native evolution or maintenance repair"
        ),
        "{value}"
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

    write_record_node(&project, "authors", &[SavedKey::Int(7)]);
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
    let _problem = integrity_problem(&value, "data.key_type");
}

#[test]
fn data_integrity_format_json_problems_carry_a_tooling_kind() {
    let project = native_project("data-integrity-json");
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
    let problem = &value["problems"][0];
    assert_eq!(problem["code"], serde_json::json!("data.decode"));
    // `data.*` integrity problems carry the tooling kind.
    assert_eq!(problem["kind"], serde_json::json!("tooling"));
}
