mod support;

use std::process::{Command, Output};

use support::{json_records_in_stderr, marrow, temp_project, unique_temp_path, write};

/// A native-store project whose entry writes one field inside a `transaction`,
/// plus a reader that dumps the store and a second writer for the dry-vs-real
/// comparison.
const SRC: &str = "module app\n\n\
                   resource Book\n\
                   \x20\x20\x20\x20required title: string\n\
                   \x20\x20\x20\x20pages: int\n\
                   store ^books(id: int): Book\n\n\
                   pub fn add()\n\
                   \x20\x20\x20\x20transaction\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^books(1).pages = 272\n";

/// The human-rendered line `run --dry-run` prints under its default text format when
/// planned writes are not committed. The typed fact is the JSON report's
/// `committed == false`; this golden pins only the text rendering.
const NOT_COMMITTED_TEXT_GOLDEN: &str = "not committed";

/// The human-rendered planned-write line `run --dry-run` prints for the `^flags(1).on`
/// bool write under its default text format: the planned path tab-joined to the typed
/// scalar `true`. The typed oracle is the JSON report's stored codec byte (`value_b64`),
/// asserted in the same test; this golden pins only that a `bool` renders as `true`, never
/// the raw codec byte `1`.
const DRY_RUN_BOOL_WRITE_TEXT_GOLDEN: &str = "would write ^flags(1).on\ttrue";

fn native_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::add" } }"#,
        );
        write(root, "src/app.mw", SRC);
    })
}

fn marrow_with_env(args: &[&str], key: &str, value: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .env(key, value)
        .output()
        .expect("run marrow with env")
}

fn faulting_dry_run_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20const boom = 1 / 0\n",
        );
    })
}

fn dry_run_after_caught_transaction_fault_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).title = \"old\"\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20try\n\
             \x20\x20\x20\x20\x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"inside\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20const boom = 1 / 0\n\
             \x20\x20\x20\x20catch err: Error\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).title = \"after\"\n",
        );
    })
}

#[test]
fn dry_run_leaves_saved_data_unchanged() {
    let project = native_project("dryrun-untouched");
    let dir = project.to_str().unwrap().to_string();

    // The store starts empty. Dump its records as the typed JSON envelope before the dry run.
    let before = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(before.status.code(), Some(0), "before: {before:?}");
    let before_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(before.stdout).expect("utf8").trim())
            .expect("dump json");

    // A dry run reports writes from isolated execution without committing them. The
    // plan is tooling output on stderr; under json it is one envelope whose `planned`
    // records carry the write op and field path as typed fields.
    let dry = marrow(&["run", "--dry-run", "--format", "json", &dir]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dry.stderr).expect("utf8").trim()).expect("json");
    assert_eq!(dry_json["committed"], false, "{dry_json}");
    // It planned writes to the title and pages fields, asserted on the typed
    // op/path of the planned records rather than the rendered "would write" text.
    let planned = dry_json["planned"].as_array().expect("planned array");
    for field in ["^books(1).title", "^books(1).pages"] {
        assert!(
            planned
                .iter()
                .any(|step| step["op"] == "write" && step["path"] == field),
            "dry run must plan a write to {field}: {dry_json}"
        );
    }

    // The saved data is unchanged: the dump after reads back the same records as the dump
    // before, asserted on the parsed `records` array rather than the rendered dump text.
    let after = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(after.status.code(), Some(0), "after: {after:?}");
    let after_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(after.stdout).expect("utf8").trim())
            .expect("dump json");
    assert_eq!(
        before_json["records"], after_json["records"],
        "dry run must leave saved data unchanged: the same records read back"
    );
}

#[test]
fn dry_run_temp_dir_create_failure_uses_path_aware_io_error() {
    let project = native_project("dryrun-tempdir-file");
    let dir = project.to_str().unwrap().to_string();
    let tmp_file = unique_temp_path("dryrun-tmpdir-file");
    std::fs::write(&tmp_file, "not a directory").expect("write TMPDIR file");

    let output = marrow_with_env(
        &["run", "--dry-run", &dir],
        "TMPDIR",
        tmp_file.to_str().expect("utf8 temp path"),
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.starts_with("io.read: failed to read "),
        "dry-run temp-dir creation must use the path-aware I/O renderer: {stderr}"
    );
    assert!(
        stderr.contains("marrow-dry-run-store-"),
        "the attempted temp path, not only the temp root, should be rendered: {stderr}"
    );
    assert!(
        !stderr.contains("run.dry_run_isolation"),
        "ordinary temp-dir I/O failures must not collapse to allocation exhaustion: {stderr}"
    );
}

#[test]
fn dry_run_does_not_persist_writes_after_a_caught_transaction_failure() {
    let project = dry_run_after_caught_transaction_fault_project("dryrun-caught-transaction-fault");
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let dry = marrow(&["run", "--dry-run", "--format", "json", &dir]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dry.stderr).expect("utf8").trim()).expect("json");
    assert_eq!(dry_json["committed"], false, "{dry_json}");
    let planned = dry_json["planned"].as_array().expect("planned array");
    assert!(
        planned
            .iter()
            .any(|step| step["op"] == "write" && step["path"] == "^books(2).title"),
        "the dry run should report the write after the caught transaction failure: {dry_json}"
    );
    assert!(
        !planned
            .iter()
            .any(|step| step["op"] == "write" && step["path"] == "^books(1).title"),
        "the dry run report must exclude writes discarded by the failed transaction: {dry_json}"
    );

    let dump = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dump.stdout).expect("utf8").trim())
            .expect("dump json");
    let records = dump_json["records"].as_array().expect("records array");
    assert_eq!(
        records.len(),
        1,
        "dry-run isolation must preserve only the seeded saved record: {dump_json}"
    );
    assert!(
        records
            .iter()
            .any(|record| record["path"] == "^books(1).title" && record["value_b64"] == "b2xk"),
        "dry-run isolation must leave the seeded value unchanged: {dump_json}"
    );
}

#[test]
fn dry_run_plan_matches_a_real_run() {
    // The dry run's planned value writes are exactly the leaf records a real run
    // commits. Run the entry for real, dump the store, and assert each expected
    // value path appears in the real store's dump.
    let dry_project = native_project("dryrun-plan-dry");
    let real_project = native_project("dryrun-plan-real");
    let dry_dir = dry_project.to_str().unwrap().to_string();
    let real_dir = real_project.to_str().unwrap().to_string();

    let dry = marrow(&["run", "--dry-run", "--format", "json", &dry_dir]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    // The dry-run report is tooling output on stderr, off the program's stdout.
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dry.stderr).expect("utf8").trim()).expect("json");
    assert_eq!(dry_json["committed"], false);
    let planned_paths: Vec<String> = dry_json["planned"]
        .as_array()
        .expect("planned array")
        .iter()
        .filter(|step| step["value_b64"].is_string())
        .filter_map(|step| step["path"].as_str().map(str::to_string))
        .collect();

    // A real run commits the writes; its dump holds those exact field paths. Read the
    // committed records as the typed `data dump --format json` envelope and pin the
    // plan-vs-real equivalence on the parsed `path` field of each record, never a
    // substring of the rendered dump.
    assert_eq!(
        marrow(&["run", &real_dir]).status.code(),
        Some(0),
        "real run"
    );
    let real_dump = marrow(&["data", "dump", "--format", "json", &real_dir]);
    assert_eq!(real_dump.status.code(), Some(0), "real dump: {real_dump:?}");
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(real_dump.stdout).expect("utf8").trim())
            .expect("dump json");
    let real_paths: Vec<&str> = real_json["records"]
        .as_array()
        .expect("records array")
        .iter()
        .filter_map(|record| record["path"].as_str())
        .collect();

    // Every planned field value the dry run reported is a committed leaf record in the real store.
    for path in &planned_paths {
        assert!(
            real_paths.contains(&path.as_str()),
            "real run is missing a path the dry run planned: {path}\n{real_json}"
        );
    }
    assert!(
        planned_paths.contains(&"^books(1).title".to_string())
            && planned_paths.contains(&"^books(1).pages".to_string()),
        "the plan must cover both field writes: {planned_paths:?}"
    );
}

#[test]
fn dry_run_renders_a_bool_write_as_its_typed_value() {
    // The dry-run text report renders a `bool` leaf write as `true`, not the codec
    // byte `1`, through the same typed-value path the trace uses.
    let project = temp_project("dryrun-bool", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Flag\n\
             \x20\x20\x20\x20on: bool\n\
             store ^flags(id: int): Flag\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^flags(1).on = true\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    // Typed oracle: the planned bool write targets the `on` member of `^flags(1)` and
    // carries the stored codec byte for `true`; the JSON report keeps the raw bytes.
    let json_run = marrow(&["run", "--dry-run", "--format", "json", &dir]);
    assert_eq!(json_run.status.code(), Some(0), "{json_run:?}");
    let report: serde_json::Value =
        serde_json::from_str(String::from_utf8(json_run.stderr).expect("utf8").trim())
            .expect("json");
    let planned = report["planned"].as_array().expect("planned array");
    let write = planned
        .iter()
        .find(|step| step["op"] == "write" && step["path"] == "^flags(1).on")
        .expect("a planned write to ^flags(1).on");
    assert_eq!(write["target"]["store"], serde_json::json!("flags"));
    assert_eq!(
        write["target"]["path"],
        serde_json::json!([{ "member": "on" }])
    );
    assert_eq!(
        write["value_b64"],
        serde_json::json!("MQ=="),
        "stored bool codec byte"
    );

    // Render contract: the text dry-run report renders that codec byte as the typed
    // scalar `true`, never the byte `1`. Pinned by the explicitly-marked golden.
    let text_run = marrow(&["run", "--dry-run", &dir]);
    assert_eq!(text_run.status.code(), Some(0), "{text_run:?}");
    let stderr = String::from_utf8(text_run.stderr).expect("utf8");
    assert!(
        stderr.contains(DRY_RUN_BOOL_WRITE_TEXT_GOLDEN),
        "a bool must dry-run as `true`, not `1`: {stderr}"
    );
}

#[test]
fn dry_run_renders_enum_and_identity_writes_as_names() {
    let project = temp_project("dryrun-enum-identity-writes", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\
             resource Order\n\
             \x20\x20\x20\x20state: Status\n\
             store ^orders(id: int): Order\n\
             resource Author\n\
             \x20\x20\x20\x20name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20\x20\x20\x20author: Id(^authors)\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^orders(1).state = Status::archived\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).author = Id(^authors, 7)\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let json_run = marrow(&["run", "--dry-run", "--format", "json", &dir]);
    assert_eq!(json_run.status.code(), Some(0), "{json_run:?}");
    let report: serde_json::Value =
        serde_json::from_str(String::from_utf8(json_run.stderr).expect("utf8").trim())
            .expect("json");
    let planned = report["planned"].as_array().expect("planned array");
    for path in ["^orders(1).state", "^books(1).author"] {
        let write = planned
            .iter()
            .find(|step| step["op"] == "write" && step["path"] == path)
            .unwrap_or_else(|| panic!("missing planned write to {path}: {report}"));
        assert!(write["value_b64"].is_string(), "{write}");
    }

    let text_run = marrow(&["run", "--dry-run", &dir]);
    assert_eq!(text_run.status.code(), Some(0), "{text_run:?}");
    let stderr = String::from_utf8(text_run.stderr).expect("utf8");
    assert!(
        stderr.contains("would write ^orders(1).state\tapp::Status::archived"),
        "enum write must dry-run as a member path: {stderr}"
    );
    assert!(
        stderr.contains("would write ^books(1).author\t^authors(7)"),
        "identity write must dry-run as a rooted identity: {stderr}"
    );
    assert!(
        !stderr.contains("cat_"),
        "catalog ids must not leak into dry-run text: {stderr}"
    );
    assert!(
        !stderr.contains("^books(1).author\t0x"),
        "identity bytes must not leak into dry-run text: {stderr}"
    );
}

#[test]
fn dry_run_renders_enum_index_keys_as_member_names() {
    let project = temp_project("dryrun-enum-index-key", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\
             resource Order\n\
             \x20\x20\x20\x20state: Status\n\
             store ^orders(id: int): Order\n\
             \x20\x20\x20\x20index byState(state, id)\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^orders(1).state = Status::archived\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let output = marrow(&["run", "--dry-run", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("index:^orders.byState(app::Status::archived, 1)"),
        "enum index key must dry-run as a member path: {stderr}"
    );
    assert!(
        !stderr.contains("cat_"),
        "catalog ids must not leak into enum index dry-run text: {stderr}"
    );
}

#[test]
fn dry_run_reports_maintenance_whole_root_deletes() {
    let project = temp_project("dryrun-root-delete", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             \x20\x20\x20\x20shelf: string\n\
             store ^books(id: int): Book\n\n\
             \x20\x20\x20\x20index byShelf(shelf, id)\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20^books(1).shelf = \"fiction\"\n\n\
             pub fn dropRoot()\n\
             \x20\x20\x20\x20delete ^books\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let dry = marrow(&[
        "run",
        "--dry-run",
        "--maintenance",
        "--format",
        "json",
        "--entry",
        "app::dropRoot",
        &dir,
    ]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    // The dry-run report is tooling output on stderr, off the program's stdout.
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dry.stderr).expect("utf8").trim()).expect("json");

    assert_eq!(dry_json["committed"], false);
    assert_eq!(dry_json["writes"], 0);
    assert_eq!(dry_json["deletes"], 2);
    let planned = dry_json["planned"].as_array().expect("planned array");
    assert!(
        planned.iter().any(|step| {
            step["op"] == "delete"
                && step["target"]["kind"] == "data"
                && step["target"]["store"] == "books"
                && step["target"]["identity"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                && step["target"]["path"].as_array().is_some_and(Vec::is_empty)
        }),
        "dry-run report must include the data root delete: {dry_json}"
    );
    assert!(
        planned.iter().any(|step| {
            step["op"] == "delete"
                && step["target"]["kind"] == "index"
                && step["target"]["index"] == "^books.byShelf"
                && step["target"]["keys"].as_array().is_some_and(Vec::is_empty)
                && step["target"]["identity"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
        }),
        "dry-run report must include the index root delete: {dry_json}"
    );

    let dump = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dump.stdout).expect("utf8").trim())
            .expect("dump json");
    assert!(
        dump_json["records"]
            .as_array()
            .expect("records array")
            .iter()
            .any(|record| record["path"] == "^books(1).title"),
        "dry run must leave the seeded record in place: {dump_json}"
    );
}

#[test]
fn dry_run_reports_non_root_deletes() {
    let project = temp_project("dryrun-nonroot-delete", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20details\n\
             \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
             store ^books(id: int): Book\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).details.note = \"kept\"\n\n\
             pub fn dropDetails()\n\
             \x20\x20\x20\x20delete ^books(1).details\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let dry = marrow(&[
        "run",
        "--dry-run",
        "--format",
        "json",
        "--entry",
        "app::dropDetails",
        &dir,
    ]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    // The dry-run report is tooling output on stderr, off the program's stdout.
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dry.stderr).expect("utf8").trim()).expect("json");

    assert_eq!(dry_json["committed"], false);
    assert_eq!(dry_json["writes"], 0);
    assert_eq!(dry_json["deletes"], 1);
    let planned = dry_json["planned"].as_array().expect("planned array");
    assert!(
        planned.iter().any(|step| {
            step["op"] == "delete"
                && step["target"]["kind"] == "data"
                && step["target"]["store"] == "books"
                && step["target"]["identity"]
                    .as_array()
                    .is_some_and(|keys| keys.len() == 1 && keys[0] == "1")
                && step["target"]["path"]
                    .as_array()
                    .is_some_and(|path| path.len() == 1 && path[0]["member"] == "details")
        }),
        "dry-run report must include the group delete: {dry_json}"
    );

    let dump = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dump.stdout).expect("utf8").trim())
            .expect("dump json");
    assert!(
        dump_json["records"]
            .as_array()
            .expect("records array")
            .iter()
            .any(|record| record["path"] == "^books(1).details.note"),
        "dry run must leave the seeded group data in place: {dump_json}"
    );
}

#[test]
fn dry_run_keeps_the_program_output_on_stdout() {
    // The program's own `print` output still lands on stdout; the dry-run report is
    // separate (stderr under text).
    let project = temp_project("dryrun-stdout", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"ran\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert_eq!(stdout, "ran\n", "program output must stay on stdout");
    // Text mode has no typed surface; the golden pins the rendered summary line.
    assert!(stderr.contains(NOT_COMMITTED_TEXT_GOLDEN), "{stderr}");
}

#[test]
fn dry_run_json_keeps_program_output_off_the_record_stream() {
    // Under `--format json` the dry-run report is tooling output and must not
    // share the program's stdout stream: stdout is exactly the program's own
    // `print` output, and the planned-write report lands on stderr as parseable
    // JSON. Mixing them would corrupt a consumer parsing stdout as JSON.
    let project = temp_project("dryrun-json-streams", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"ran\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", "--format", "json", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // Program output owns stdout untouched; no JSON record leaked onto it.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "ran\n", "program output must own stdout: {stdout}");

    // The dry-run report is one JSON envelope on stderr, off the program's stream.
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    let report: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("the dry-run report is JSON on stderr");
    assert_eq!(report["committed"], false, "{report}");
    let planned = report["planned"].as_array().expect("planned array");
    assert!(
        planned
            .iter()
            .any(|step| step["op"] == "write" && step["path"] == "^books(1).title"),
        "the planned write must appear in the stderr report: {report}"
    );
}

#[test]
fn dry_run_json_flushes_the_plan_when_the_run_faults() {
    let project = faulting_dry_run_project("dryrun-json-fault");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = json_records_in_stderr(output.stderr);
    let [report, fault] = records.as_slice() else {
        panic!("expected dry-run JSON report and fault envelope: {records:?}");
    };
    assert_eq!(report["committed"], false, "{report}");
    assert_eq!(report["writes"], 2, "{report}");
    assert_eq!(report["deletes"], 0, "{report}");
    assert_eq!(fault["code"], "run.divide_by_zero", "{fault}");
    assert_eq!(fault["kind"], "runtime", "{fault}");
    assert_eq!(fault["data"], serde_json::json!({}), "{fault}");
    assert!(
        report["planned"]
            .as_array()
            .expect("planned array")
            .iter()
            .any(|step| step["op"] == "write" && step["path"] == "^books(1).title"),
        "faulting dry run must include the planned write: {report}"
    );

    let dump = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dump.stdout).expect("utf8").trim())
            .expect("dump json");
    assert_eq!(
        dump_json["records"],
        serde_json::json!([]),
        "faulting dry run must leave saved data unchanged: {dump_json}"
    );
}

#[test]
fn dry_run_rejects_jsonl_format_before_running() {
    let project = faulting_dry_run_project("dryrun-jsonl-fault");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown format: jsonl"), "{stderr}");
}

#[test]
fn dry_run_does_not_promise_native_file_byte_stability() {
    // The dry-run contract is logical saved-data stability. Normal run setup may
    // still open, fence, and stamp the configured store before entry isolation, so
    // no CLI surface may promise byte-for-byte file identity.
    let help = marrow(&["run", "--help"]);
    let help_text = String::from_utf8(help.stdout).expect("utf8");
    assert!(
        !help_text.contains("byte-for-byte"),
        "run --help must not promise byte-for-byte file identity: {help_text}"
    );
    assert!(
        help_text.contains("saved data"),
        "run --help must describe the real saved-data guarantee: {help_text}"
    );
}

#[test]
fn dry_run_composes_with_trace() {
    // `--dry-run --trace` traces the run and still does not commit its writes.
    // The trace names the write; the dry-run report says it was not committed.
    let project = native_project("dryrun-trace");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    // The trace names the write path it traced; that path presence is structural.
    assert!(stderr.contains("^books(1).title"), "trace: {stderr}");
    // Text mode has no typed surface; the golden pins the rendered summary line.
    assert!(
        stderr.contains(NOT_COMMITTED_TEXT_GOLDEN),
        "report: {stderr}"
    );

    // The store is still empty after the composed dry run: the typed dump envelope holds
    // no records, asserted on the parsed `records` array rather than the empty-store text.
    let dump = marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_json: serde_json::Value =
        serde_json::from_str(String::from_utf8(dump.stdout).expect("utf8").trim())
            .expect("dump json");
    assert_eq!(
        dump_json["records"],
        serde_json::json!([]),
        "store must be empty: {dump_json}"
    );
}
