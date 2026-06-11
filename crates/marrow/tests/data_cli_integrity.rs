//! `marrow data integrity`: the saved-data integrity verdicts. Problems are asserted
//! by their typed diagnostic code, tooling kind, and rendered path span; the healthy
//! text verdict is a render contract pinned by explicitly-marked prose. The shared
//! child-page limit guard is asserted on its typed query error.

use marrow_check::tooling::{DataQuerySegment, QueryError, ToolingError, data_children};
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

mod support;
mod support_data;

use support::write;
use support_data::{
    checked_place, checked_program, encode_identity_keys, field_path, integrity_problem, json,
    keyed_field_path, marrow, native_project, seeded_project, write_orphan_cell, write_tree_value,
};

const NATIVE_STORE_CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;

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
