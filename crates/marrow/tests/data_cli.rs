use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedPlace, checked_saved_root_place,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};

mod support;

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create dir");
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn marrow(args: &[&str]) -> std::process::Output {
    for arg in args {
        let path = Path::new(arg);
        if path.is_dir() {
            support::commit_catalog_if_clean(path);
        }
    }
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

const CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;
const SRC: &str = "module app\n\
                   \n\
                   resource Counter at ^counter(id: int)\n\
                   \x20\x20\x20\x20required value: int\n\
                   \n\
                   pub fn seed()\n\
                   \x20\x20\x20\x20var c: Counter\n\
                   \x20\x20\x20\x20c.value = 42\n\
                   \x20\x20\x20\x20transaction\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n";

fn native_project(name: &str) -> PathBuf {
    let root = temp_dir(name);
    write(&root, "marrow.json", CONFIG);
    write(&root, "src/app.mw", SRC);
    root
}

#[test]
fn data_roots_lists_stored_roots() {
    let project = native_project("data-roots");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    let output = marrow(&["data", "roots", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("^counter"), "{stdout}");
}

#[test]
fn data_stats_counts_roots_and_records() {
    let project = native_project("data-stats");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    let output = marrow(&["data", "stats", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("roots: 1"), "{stdout}");
    assert!(
        stdout.contains("records: ") && !stdout.contains("records: 0"),
        "{stdout}"
    );
}

#[test]
fn inspecting_an_unseeded_project_reports_no_data_and_creates_nothing() {
    let project = native_project("data-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "roots", &dir]);
    // Inspection is read-only: it must not create the store file.
    let created = project.join(".data").join("marrow.redb").exists();
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("(no saved data)"), "{stdout}");
    assert!(!created, "inspection must not create the store");
}

/// Seed the `native_project` fixture and return its directory string. The fixture
/// stores one record, `^counter(1).value = 42`.
fn seeded_project(name: &str) -> (PathBuf, String) {
    let project = native_project(name);
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    (project, dir)
}

#[test]
fn data_dump_prints_each_record_as_path_and_value() {
    let (project, dir) = seeded_project("data-dump");
    let output = marrow(&["data", "dump", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    // The one seeded record renders as its Marrow path and raw value text.
    assert!(stdout.contains("^counter(1).value"), "{stdout}");
    assert!(stdout.contains("42"), "{stdout}");
}

#[test]
fn data_dump_of_an_unseeded_project_prints_empty_and_creates_nothing() {
    let project = native_project("data-dump-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "dump", &dir]);
    let created = project.join(".data").join("marrow.redb").exists();
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("(no saved data)"), "{stdout}");
    assert!(!created, "dump must not create the store");
}

fn checked_program(project: &Path) -> CheckedProgram {
    support::commit_catalog_if_clean(project);
    let config_text = fs::read_to_string(project.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let (report, program) = marrow_check::check_project(project, &config).expect("check project");
    assert!(
        !report.has_errors(),
        "tree-cell fixture project must check cleanly: {report:#?}"
    );
    program
}

fn checked_place(project: &Path, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(
        &checked_program(project),
        root,
        marrow_syntax::SourceSpan::default(),
    )
    .expect("checked saved root")
}

fn catalog_id(raw: &str) -> CatalogId {
    CatalogId::new(raw.to_string()).expect("catalog id")
}

fn member_catalog_id(members: &[CheckedSavedMember], name: &str) -> CatalogId {
    let member = members
        .iter()
        .find(|member| member.name == name)
        .expect("checked member");
    catalog_id(&member.catalog_id)
}

fn field_path(place: &CheckedSavedPlace, name: &str) -> Vec<DataPathSegment> {
    vec![DataPathSegment::Member(member_catalog_id(
        &place.root_members,
        name,
    ))]
}

fn keyed_field_path(place: &CheckedSavedPlace, name: &str, key: SavedKey) -> Vec<DataPathSegment> {
    vec![
        DataPathSegment::Member(member_catalog_id(&place.root_members, name)),
        DataPathSegment::Key(key),
    ]
}

fn write_tree_value(
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
        .write_data_value(&catalog_id(&place.store_catalog_id), identity, path, value)
        .expect("write tree-cell value");
}

fn encode_identity_keys(keys: &[SavedKey]) -> Vec<u8> {
    encode_identity_payload(keys)
}

#[test]
fn data_integrity_passes_on_a_healthy_seeded_project() {
    let (project, dir) = seeded_project("data-integrity-ok");
    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("integrity verified"), "{stdout}");
}

#[test]
fn data_integrity_accepts_singleton_fields_and_keyed_tree_members() {
    let project = temp_dir("data-integrity-singleton-members");
    write(
        &project,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
    );
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         use std::clock\n\n\
         resource Settings at ^settings\n\
         \x20\x20\x20\x20maxLoans: int\n\
         \x20\x20\x20\x20theme: string\n\n\
         resource Hits at ^hits\n\
         \x20\x20\x20\x20when(moment: instant): int\n\n\
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

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("integrity verified"), "{stdout}");
}

/// Write one data leaf directly under a fabricated store catalog id and member, by
/// passing low-level catalog ids the schema never declares. This stands in for data
/// a dropped root or field left behind in the store.
fn write_orphan_cell(project: &Path, store_catalog: &str, member_catalog: &str) {
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

#[test]
fn data_integrity_reports_an_undeclared_store_cell_as_data_orphan() {
    let (project, dir) = seeded_project("data-integrity-orphan");
    // A data cell under a store catalog id the schema does not declare: a dropped
    // root left it behind. The declared-cell walk never visits it, so only the
    // actual-cell orphan scan catches it.
    write_orphan_cell(&project, "cat_00000000deadbeef", "cat_0000000000000001");

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.orphan"), "{stderr}");
}

#[test]
fn data_integrity_reports_an_undeclared_member_cell_as_data_orphan() {
    let (project, dir) = seeded_project("data-integrity-orphan-member");
    // The store id is the real one, but the member catalog id is undeclared: a
    // dropped field left this cell behind.
    let place = checked_place(&project, "counter");
    write_orphan_cell(&project, &place.store_catalog_id, "cat_00000000cafef00d");

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.orphan"), "{stderr}");
}

#[test]
fn data_integrity_reports_an_undecodable_data_cell_key_as_store_corruption() {
    let (project, dir) = seeded_project("data-integrity-corrupt-key");
    // A data-family cell key (the `00 01 20` tree-cell data prefix) whose body does
    // not decode under the key grammar: an unterminated store id. Restore replays
    // any data/index-family key, so this writes a structurally corrupt cell.
    {
        let store_dir = project.join(".data");
        let store = TreeStore::open(&store_dir.join("marrow.redb")).expect("open native store");
        store
            .restore_cell(&[0x00, 0x01, 0x20, b'x'], b"corrupt".to_vec())
            .expect("write corrupt data-family cell");
    }

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("store.corruption"), "{stderr}");
}

#[test]
fn data_integrity_reports_an_orphan_problem_with_a_tooling_kind() {
    let (project, dir) = seeded_project("data-integrity-orphan-json");
    write_orphan_cell(&project, "cat_00000000deadbeef", "cat_0000000000000001");

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
    let problem = value["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .find(|problem| problem["code"] == serde_json::json!("data.orphan"))
        .expect("an orphan problem");
    assert_eq!(problem["kind"], serde_json::json!("tooling"), "{value}");
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

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.decode"), "{stderr}");
    assert!(stderr.contains("^counter(1).value"), "{stderr}");
}

#[test]
fn data_integrity_reports_a_corrupt_identity_leaf_as_data_decode() {
    let project = temp_dir("data-integrity-identity");
    write(
        &project,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
    );
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Author at ^authors(id: int)\n\
         \x20\x20\x20\x20required name: string\n\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20authorId: Id(^authors)\n",
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

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.decode"), "{stderr}");
    assert!(stderr.contains("^books(1).authorId"), "{stderr}");
}

#[test]
fn data_integrity_reports_a_wrong_typed_identity_leaf_as_data_key_type() {
    let project = temp_dir("data-integrity-identity-key-type");
    write(
        &project,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
    );
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Author at ^authors(id: int)\n\
         \x20\x20\x20\x20required name: string\n\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20authorId: Id(^authors)\n",
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

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.key_type"), "{stderr}");
    assert!(stderr.contains("^books(1).authorId"), "{stderr}");
}

#[test]
fn data_integrity_reports_a_wrong_typed_keyed_member_key_as_data_key_type() {
    let project = temp_dir("data-integrity-layer-key-type");
    write(
        &project,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
    );
    write(
        &project,
        "src/app.mw",
        "module app\n\n\
         resource Hits at ^hits\n\
         \x20\x20\x20\x20when(moment: instant): int\n",
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

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.key_type"), "{stderr}");
    assert!(
        stderr.contains("^hits.when(\"not-an-instant\")"),
        "{stderr}"
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

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.key_type"), "{stderr}");
}

#[test]
fn data_get_reads_a_path_value_and_reports_absence() {
    let (project, dir) = seeded_project("data-get");
    let present = marrow(&["data", "get", &dir, "^counter(1).value"]);
    let absent = marrow(&["data", "get", &dir, "^counter(2).value"]);
    let malformed = marrow(&["data", "get", &dir, "counter(1)"]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(present.status.code(), Some(0), "{present:?}");
    assert!(
        String::from_utf8(present.stdout).unwrap().contains("42"),
        "present value"
    );

    assert_eq!(absent.status.code(), Some(0), "{absent:?}");
    assert!(
        String::from_utf8(absent.stdout)
            .unwrap()
            .contains("(absent)"),
        "absent marker"
    );

    // A path that does not parse fails before touching the store: a usage error.
    assert_eq!(malformed.status.code(), Some(2), "{malformed:?}");
}

#[test]
fn data_get_distinguishes_a_children_only_path_from_absent() {
    // `^counter(1)` is a record identity node: it has a `.value` child but no
    // direct value, so `get` must report it differently from a truly absent path.
    let (project, dir) = seeded_project("data-get-children");
    let children = marrow(&["data", "get", &dir, "^counter(1)"]);
    fs::remove_dir_all(&project).ok();
    assert_eq!(children.status.code(), Some(0), "{children:?}");
    let out = String::from_utf8(children.stdout).unwrap();
    assert!(
        out.contains("has children"),
        "children-only marker, got: {out}"
    );
}

#[test]
fn data_get_and_integrity_on_an_unseeded_project_create_nothing() {
    let project = native_project("data-readonly");
    let dir = project.to_str().unwrap().to_string();
    let get = marrow(&["data", "get", &dir, "^counter(1).value"]);
    let integrity = marrow(&["data", "integrity", &dir]);
    // Read-only: no command may create the store file.
    let created = project.join(".data").join("marrow.redb").exists();
    fs::remove_dir_all(&project).ok();

    // An absent path on an empty store is a successful, queryable absence.
    assert_eq!(get.status.code(), Some(0), "{get:?}");
    assert!(
        String::from_utf8(get.stdout).unwrap().contains("(absent)"),
        "absent on empty store"
    );
    // Nothing to verify is healthy.
    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    assert!(!created, "inspection must not create the store");
}

#[test]
fn data_roots_format_json_emits_a_structured_envelope() {
    let (project, dir) = seeded_project("data-roots-json");
    let output = marrow(&["data", "roots", "--format", "json", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
    assert_eq!(value["project"], serde_json::json!(dir));
    assert_eq!(value["roots"], serde_json::json!(["counter"]));
}

#[test]
fn data_stats_format_json_emits_counts() {
    let (project, dir) = seeded_project("data-stats-json");
    let output = marrow(&["data", "stats", "--format", "json", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
    assert_eq!(value["roots"], serde_json::json!(1));
    assert_eq!(value["records"], serde_json::json!(1));
}

#[test]
fn data_commands_page_through_large_native_store() {
    const RECORDS: usize = 150;

    let project = native_project("data-paged");
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    for id in 1..=RECORDS {
        write_tree_value(
            &project,
            "counter",
            &[SavedKey::Int(id as i64)],
            &value_path,
            b"7".to_vec(),
        );
    }

    let stats = marrow(&["data", "stats", "--format", "json", &dir]);
    let dump = marrow(&["data", "dump", "--format", "jsonl", &dir]);
    let integrity = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(stats.status.code(), Some(0), "{stats:?}");
    let stats_json: serde_json::Value = serde_json::from_slice(&stats.stdout).expect("stats json");
    assert_eq!(stats_json["records"], serde_json::json!(RECORDS));

    assert_eq!(dump.status.code(), Some(0), "{dump:?}");
    let dump_stdout = String::from_utf8(dump.stdout).expect("dump utf8");
    let lines = dump_stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), RECORDS + 1);
    let summary: serde_json::Value = serde_json::from_str(lines[RECORDS]).expect("summary json");
    assert_eq!(summary["records"], serde_json::json!(RECORDS));

    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    let integrity_stdout = String::from_utf8(integrity.stdout).expect("integrity utf8");
    assert!(
        integrity_stdout.contains(&format!("({RECORDS} records)")),
        "{integrity_stdout}"
    );
}

#[test]
fn data_dump_format_jsonl_emits_a_record_then_a_summary() {
    let (project, dir) = seeded_project("data-dump-jsonl");
    let output = marrow(&["data", "dump", "--format", "jsonl", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("record json");
    assert_eq!(record["path"], serde_json::json!("^counter(1).value"));
    assert!(record["value_b64"].is_string(), "{record}");
    let summary: serde_json::Value = serde_json::from_str(lines[1]).expect("summary json");
    assert_eq!(summary["kind"], serde_json::json!("summary"));
    assert_eq!(summary["records"], serde_json::json!(1));
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
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
    let problem = &value["problems"][0];
    assert_eq!(problem["code"], serde_json::json!("data.decode"));
    // `data.*` has no dedicated kind, so `kind_for_code`'s default arm classifies
    // it as tooling.
    assert_eq!(problem["kind"], serde_json::json!("tooling"));
}
