use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
fn data_roots_lists_the_saved_roots() {
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

/// Write `records` to `path` as a Marrow archive, reusing the store's own
/// archive writer over an in-memory store so the bytes match a real backup. This
/// lets a test plant a deliberately-malformed value at a declared path, then
/// restore it through the CLI to exercise integrity over corruption.
fn write_archive_with(path: &Path, records: &[(Vec<u8>, Vec<u8>)]) {
    use marrow_store::mem::MemStore;
    let mut store = MemStore::new();
    for (key, value) in records {
        store.write(key, value.clone());
    }
    let file = fs::File::create(path).expect("create archive");
    let mut writer = std::io::BufWriter::new(file);
    marrow_store::archive::write_archive(&store, &mut writer).expect("write archive");
    use std::io::Write;
    writer.flush().expect("flush archive");
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

#[test]
fn data_integrity_reports_a_non_canonical_value_as_data_decode() {
    use marrow_store::path::{PathSegment, SavedKey, encode_path};

    // An empty project, with a hand-built archive planting a bad value at the
    // declared int field `^counter(1).value`. Restore writes it as-is, then
    // integrity finds the mismatch.
    let project = native_project("data-integrity-decode");
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("corrupt.mra");
    let bad_path = encode_path(&[
        PathSegment::Root("counter".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("value".into()),
    ]);
    // `+1` is not a canonical int form, so `decode_value` rejects it.
    write_archive_with(&archive, &[(bad_path, b"+1".to_vec())]);
    assert_eq!(
        marrow(&["restore", &dir, archive.to_str().unwrap()])
            .status
            .code(),
        Some(0)
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
    use marrow_store::path::{PathSegment, SavedKey, encode_key_value, encode_path};

    // A `Book.authorId` typed reference to `Author` stores the referenced identity's
    // canonical single-key encoding. Planting a value with a valid key followed by
    // trailing garbage cannot decode back to one clean key, so integrity flags it as
    // a `data.decode` on the identity leaf.
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
         \x20\x20\x20\x20authorId: Author::Id\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("corrupt.mra");
    let leaf_path = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("authorId".into()),
    ]);
    // A valid `Author::Id(7)` key with an extra trailing byte: decodes one key but
    // leaves bytes over, which `decode_identity_arity` rejects.
    let mut corrupt = encode_key_value(&SavedKey::Int(7));
    corrupt.push(0xFF);
    write_archive_with(&archive, &[(leaf_path, corrupt)]);
    assert_eq!(
        marrow(&["restore", &dir, archive.to_str().unwrap()])
            .status
            .code(),
        Some(0)
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
    use marrow_store::path::{PathSegment, SavedKey, encode_key_value, encode_path};

    // A `Book.authorId` typed reference to `Author` (an `int`-keyed resource) stores
    // the referenced identity's canonical key encoding. A planted leaf that holds a
    // single *string* key decodes back as one clean key by arity alone, so the
    // arity-only check passes it — but `Author`'s identity key is declared `int`, so
    // the stored reference points at a record that cannot exist. Integrity must flag
    // the inner key as a `data.key_type` mismatch, not silently accept it.
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
         \x20\x20\x20\x20authorId: Author::Id\n",
    );
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("wrongkey.mra");
    let leaf_path = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("authorId".into()),
    ]);
    // One clean string key where `Author`'s identity key is declared `int`: decodes
    // by arity but is the wrong scalar for the referenced keyspace.
    let wrong_typed = encode_key_value(&SavedKey::Str("not-an-int".into()));
    write_archive_with(&archive, &[(leaf_path, wrong_typed)]);
    assert_eq!(
        marrow(&["restore", &dir, archive.to_str().unwrap()])
            .status
            .code(),
        Some(0)
    );

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.key_type"), "{stderr}");
    assert!(stderr.contains("^books(1).authorId"), "{stderr}");
}

#[test]
fn data_integrity_reports_orphan_data_under_an_unknown_root() {
    use marrow_store::path::{PathSegment, SavedKey, encode_path};

    let project = native_project("data-integrity-orphan");
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("orphan.mra");
    // `^ghosts(1).value` is under a root the schema does not declare.
    let orphan_path = encode_path(&[
        PathSegment::Root("ghosts".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("value".into()),
    ]);
    write_archive_with(&archive, &[(orphan_path, b"7".to_vec())]);
    assert_eq!(
        marrow(&["restore", &dir, archive.to_str().unwrap()])
            .status
            .code(),
        Some(0)
    );

    let output = marrow(&["data", "integrity", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("data.orphan"), "{stderr}");
    assert!(stderr.contains("^ghosts(1).value"), "{stderr}");
}

#[test]
fn data_integrity_reports_a_wrong_typed_record_key_as_data_key_type() {
    use marrow_store::path::{PathSegment, SavedKey, encode_path};

    // `^counter` declares an `int` identity, but this hand-built key is a string.
    // The member chain still resolves, so this is a key-type mismatch — not an
    // orphan — and integrity must flag it as data the schema cannot trust.
    let project = native_project("data-integrity-key-type");
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("badkey.mra");
    let bad_path = encode_path(&[
        PathSegment::Root("counter".into()),
        PathSegment::RecordKey(SavedKey::Str("oops".into())),
        PathSegment::Field("value".into()),
    ]);
    write_archive_with(&archive, &[(bad_path, b"7".to_vec())]);
    assert_eq!(
        marrow(&["restore", &dir, archive.to_str().unwrap()])
            .status
            .code(),
        Some(0)
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
    use marrow_store::path::{PathSegment, SavedKey, encode_path};

    let project = native_project("data-integrity-json");
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("orphan.mra");
    let orphan_path = encode_path(&[
        PathSegment::Root("ghosts".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("value".into()),
    ]);
    write_archive_with(&archive, &[(orphan_path, b"7".to_vec())]);
    assert_eq!(
        marrow(&["restore", &dir, archive.to_str().unwrap()])
            .status
            .code(),
        Some(0)
    );

    let output = marrow(&["data", "integrity", "--format", "json", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
    let problem = &value["problems"][0];
    assert_eq!(problem["code"], serde_json::json!("data.orphan"));
    // `data.*` has no dedicated kind, so `kind_for_code`'s default arm classifies
    // it as tooling.
    assert_eq!(problem["kind"], serde_json::json!("tooling"));
}
