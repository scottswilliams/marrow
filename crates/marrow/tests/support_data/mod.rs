// Each data_cli test binary includes this whole module but uses only the helpers
// its cases need, so a helper unused by one binary is not dead across the split.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

use marrow_check::{CheckedProgram, CheckedSavedPlace, checked_saved_root_place};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::support::{self, TempProject, member_catalog_id, write};

/// Run the binary, first committing the pending catalog of any directory argument
/// so `data`'s read-only commands observe a frozen catalog the way a prior `run`
/// would have left it.
pub(crate) fn marrow(args: &[&str]) -> std::process::Output {
    for arg in args {
        let path = Path::new(arg);
        if path.is_dir() {
            support::commit_catalog_if_clean(path);
        }
    }
    support::marrow(args)
}

pub(crate) fn json(output: std::process::Output) -> serde_json::Value {
    support::json(output.stdout)
}

pub(crate) fn integrity_problem(value: &serde_json::Value, code: &str) -> serde_json::Value {
    value["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .find(|problem| problem["code"] == serde_json::json!(code))
        .cloned()
        .unwrap_or_else(|| panic!("{code} not found in {value:#?}"))
}

pub(crate) fn native_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", support::counter_source());
    })
}

/// Seed the `native_project` fixture and return its directory string. The fixture
/// stores one record, `^counter(1).value = 42`.
pub(crate) fn seeded_project(name: &str) -> (TempProject, String) {
    let project = native_project(name);
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    (project, dir)
}

pub(crate) fn checked_program(project: impl AsRef<Path>) -> CheckedProgram {
    let project = project.as_ref();
    support::commit_catalog_if_clean(project);
    let config_text = fs::read_to_string(project.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    // Bind the program against the engine-resident accepted catalog so its saved roots
    // carry the same catalog ids the live store keys cells under.
    let accepted = support::native_store_path(project, &config)
        .filter(|path| path.exists())
        .and_then(|path| {
            marrow_store::tree::TreeStore::open_read_only(&path)
                .expect("open store read-only")
                .read_catalog_snapshot()
                .expect("read store catalog snapshot")
        });
    let (report, program) =
        marrow_check::check_project_with_catalog(project, &config, accepted.as_ref())
            .expect("check project");
    assert!(
        !report.has_errors(),
        "tree-cell fixture project must check cleanly: {report:#?}"
    );
    program
}

pub(crate) fn checked_place(project: impl AsRef<Path>, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(
        &checked_program(project),
        root,
        marrow_syntax::SourceSpan::default(),
    )
    .expect("checked saved root")
}

pub(crate) fn catalog_id(raw: &str) -> CatalogId {
    CatalogId::new(raw.to_string()).expect("catalog id")
}

pub(crate) fn checked_catalog_id(raw: &Option<String>) -> CatalogId {
    CatalogId::new(raw.clone().expect("accepted catalog id")).expect("catalog id")
}

pub(crate) fn field_path(place: &CheckedSavedPlace, name: &str) -> Vec<DataPathSegment> {
    vec![DataPathSegment::Member(member_catalog_id(
        &place.root_members,
        name,
    ))]
}

pub(crate) fn keyed_field_path(
    place: &CheckedSavedPlace,
    name: &str,
    key: SavedKey,
) -> Vec<DataPathSegment> {
    vec![
        DataPathSegment::Member(member_catalog_id(&place.root_members, name)),
        DataPathSegment::Key(key),
    ]
}

pub(crate) fn write_tree_value(
    project: &Path,
    root: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    value: Vec<u8>,
) {
    let place = checked_place(project, root);
    let store_dir = project.join(".data");
    fs::create_dir_all(&store_dir).expect("create store dir");
    let store = TreeStore::open(&store_dir.join("marrow.redb")).expect("open native store");
    let store_id = checked_catalog_id(&place.store_catalog_id);
    store
        .write_node(&store_id, identity)
        .expect("write tree-cell node");
    store
        .write_data_value(&store_id, identity, path, value)
        .expect("write tree-cell value");
}

pub(crate) fn write_record_node(project: &Path, root: &str, identity: &[SavedKey]) {
    let place = checked_place(project, root);
    let store_dir = project.join(".data");
    fs::create_dir_all(&store_dir).expect("create store dir");
    let store = TreeStore::open(&store_dir.join("marrow.redb")).expect("open native store");
    store
        .write_node(&checked_catalog_id(&place.store_catalog_id), identity)
        .expect("write tree-cell node");
}

pub(crate) fn write_tree_value_without_node(
    project: &Path,
    root: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    value: Vec<u8>,
) {
    let place = checked_place(project, root);
    let store_dir = project.join(".data");
    fs::create_dir_all(&store_dir).expect("create store dir");
    let store = TreeStore::open(&store_dir.join("marrow.redb")).expect("open native store");
    store
        .write_data_value(
            &checked_catalog_id(&place.store_catalog_id),
            identity,
            path,
            value,
        )
        .expect("write tree-cell value");
}

pub(crate) fn encode_identity_keys(keys: &[SavedKey]) -> Vec<u8> {
    encode_identity_payload(keys)
}

/// Write one data leaf directly under a fabricated store catalog id and member, by
/// passing low-level catalog ids the schema never declares. This stands in for data
/// a dropped root or field left behind in the store.
pub(crate) fn write_orphan_cell(project: &Path, store_catalog: &str, member_catalog: &str) {
    let store_dir = project.join(".data");
    fs::create_dir_all(&store_dir).expect("create store dir");
    let store = TreeStore::open(&store_dir.join("marrow.redb")).expect("open native store");
    let path = vec![DataPathSegment::Member(catalog_id(member_catalog))];
    store
        .write_data_value(
            &catalog_id(store_catalog),
            &[SavedKey::Int(1)],
            &path,
            b"left-behind".to_vec(),
        )
        .expect("write orphan tree-cell value");
}
