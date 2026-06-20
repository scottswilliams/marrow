//! `marrow data roots|stats|dump`: the saved-data inventory commands. Structured
//! formats are asserted as parsed JSON/JSONL fields; the text format is a render
//! contract pinned by explicitly-marked prose.

use std::fs;

use crate::support;
use crate::support_data;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use support_data::{
    assert_stable_store_snapshot_eq, assert_store_snapshot, checked_place, field_path, json,
    marrow, native_project, seeded_project, write_record_presence, write_tree_values,
};

#[test]
fn data_roots_lists_stored_roots() {
    // Render contract: the text format prints one `^root` line per saved root; the
    // typed root list is covered by the JSON-format test in this file.
    let (_project, dir) = seeded_project("data-roots");
    let output = marrow(&["data", "roots", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "^counter\n", "{stdout}");
}

#[test]
fn data_stats_counts_roots_records_and_cells() {
    // Render contract: the text format prints `roots:`/`records:`/`cells:` count
    // lines; the typed counts are covered by the JSON-format test in this file.
    // The seeded fixture stores one entity (`^counter(1)`) with one field cell.
    let (_project, dir) = seeded_project("data-stats");
    let output = marrow(&["data", "stats", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "roots: 1\nrecords: 1\ncells: 1\n", "{stdout}");
}

#[test]
fn data_inventory_does_not_treat_an_overlong_node_as_a_shorter_record() {
    let project = native_project("data-overlong-node");
    let dir = project.to_str().unwrap().to_string();
    write_record_presence(&project, "counter", &[SavedKey::Int(1), SavedKey::Int(2)]);

    let stats = marrow(&["data", "stats", "--format", "json", &dir]);
    let get = marrow(&["data", "get", "--format", "json", &dir, "^counter(1)"]);

    assert_eq!(stats.status.code(), Some(0), "{stats:?}");
    let stats_json = json(stats);
    assert_eq!(stats_json["roots"], serde_json::json!(0));
    assert_eq!(stats_json["records"], serde_json::json!(0));
    assert_eq!(stats_json["cells"], serde_json::json!(0));

    assert_eq!(get.status.code(), Some(0), "{get:?}");
    let get_json = json(get);
    assert_eq!(get_json["presence"], serde_json::json!("absent"));
}

#[test]
fn data_inventory_ignores_overlong_nodes_under_composite_roots() {
    let project = support::temp_project_uncommitted("data-composite-overlong-node", |root| {
        support::write(root, "marrow.json", support::native_config());
        support::write(
            root,
            "src/app.mw",
            "module app\n\
             \n\
             resource Pair\n\
             \x20\x20\x20\x20value: int\n\
             store ^pairs(left: int, right: int): Pair\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    write_record_presence(
        &project,
        "pairs",
        &[SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)],
    );

    let roots = marrow(&["data", "roots", "--format", "json", &dir]);
    let stats = marrow(&["data", "stats", "--format", "json", &dir]);
    let get = marrow(&["data", "get", "--format", "json", &dir, "^pairs"]);

    assert_eq!(roots.status.code(), Some(0), "{roots:?}");
    assert_eq!(json(roots)["roots"], serde_json::json!([]));

    assert_eq!(stats.status.code(), Some(0), "{stats:?}");
    let stats_json = json(stats);
    assert_eq!(stats_json["roots"], serde_json::json!(0));
    assert_eq!(stats_json["records"], serde_json::json!(0));
    assert_eq!(stats_json["cells"], serde_json::json!(0));

    assert_eq!(get.status.code(), Some(0), "{get:?}");
    assert_eq!(json(get)["presence"], serde_json::json!("absent"));
}

#[test]
fn inspecting_an_unseeded_project_reports_no_data_and_writes_no_records() {
    // The data harness freezes the clean project before invoking the command, so
    // `data roots` observes a committed empty store and writes none of its own.
    let project = native_project("data-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "roots", &dir]);
    let roots_json = marrow(&["data", "roots", "--format", "json", &dir]);
    let stats = marrow(&["data", "stats", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Render contract: an empty store prints a human placeholder, not a bare line.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "(no saved data)\n", "{stdout}");
    assert_eq!(roots_json.status.code(), Some(0), "{roots_json:?}");
    let roots_json = json(roots_json);
    assert_eq!(roots_json["roots"], serde_json::json!([]));
    assert_store_snapshot(&roots_json);
    // Inspection writes no cells: the store holds zero saved data cells.
    let stats_json = json(stats);
    assert_eq!(stats_json["records"], serde_json::json!(0));
    assert_eq!(stats_json["cells"], serde_json::json!(0));
}

#[test]
fn data_roots_without_native_store_reports_null_snapshot() {
    let project = support::temp_project("data-roots-memory-store", |root| {
        support::write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        support::write(root, "src/app.mw", support::counter_source());
    });
    let dir = project.to_str().unwrap().to_string();

    let roots = marrow(&["data", "roots", "--format", "json", &dir]);

    assert_eq!(roots.status.code(), Some(0), "{roots:?}");
    let roots = json(roots);
    assert_eq!(roots["roots"], serde_json::json!([]));
    assert_eq!(roots["store_snapshot"], serde_json::Value::Null);
}

#[test]
fn data_dump_prints_each_record_as_path_and_value() {
    // Render contract: the text format prints `<path>\t<value>` for the one record;
    // the typed record is covered by the JSONL-format test in this file.
    let (_project, dir) = seeded_project("data-dump");
    let output = marrow(&["data", "dump", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "^counter(1).value\t42\n", "{stdout}");
}

#[test]
fn data_inventory_reads_backup_while_live_store_is_locked() {
    let (project, dir) = seeded_project("data-backup-inventory");
    let archive = support::backup_artifact(&project, "counter.mwbackup");
    let archive_arg = archive.to_str().expect("backup path utf8");

    let live_dump = support::marrow(&["data", "dump", "--format", "json", &dir]);
    let live_roots = support::marrow(&["data", "roots", "--format", "json", &dir]);
    let live_stats = support::marrow(&["data", "stats", "--format", "json", &dir]);
    assert_eq!(live_dump.status.code(), Some(0), "{live_dump:?}");
    assert_eq!(live_roots.status.code(), Some(0), "{live_roots:?}");
    assert_eq!(live_stats.status.code(), Some(0), "{live_stats:?}");
    let live_dump = support::json(live_dump.stdout);
    let live_roots = support::json(live_roots.stdout);
    let live_stats = support::json(live_stats.stdout);

    let _writer = TreeStore::open(&project.join(".data").join("marrow.redb"))
        .expect("hold the native writer open");
    let locked_live = support::marrow(&["data", "dump", "--format", "json", &dir]);
    assert_eq!(locked_live.status.code(), Some(1), "{locked_live:?}");
    let locked_error = support::json(locked_live.stdout);
    assert_eq!(locked_error["code"], serde_json::json!("store.locked"));

    let backup_dump = support::marrow(&[
        "data",
        "dump",
        "--backup",
        archive_arg,
        "--format",
        "json",
        &dir,
    ]);
    let backup_roots = support::marrow(&[
        "data",
        "roots",
        "--backup",
        archive_arg,
        "--format",
        "json",
        &dir,
    ]);
    let backup_stats = support::marrow(&[
        "data",
        "stats",
        "--backup",
        archive_arg,
        "--format",
        "json",
        &dir,
    ]);

    assert_eq!(backup_dump.status.code(), Some(0), "{backup_dump:?}");
    assert_eq!(backup_roots.status.code(), Some(0), "{backup_roots:?}");
    assert_eq!(backup_stats.status.code(), Some(0), "{backup_stats:?}");
    assert_eq!(support::json(backup_dump.stdout), live_dump);
    let backup_roots = support::json(backup_roots.stdout);
    assert_eq!(backup_roots["project"], live_roots["project"]);
    assert_eq!(backup_roots["roots"], live_roots["roots"]);
    assert_stable_store_snapshot_eq(&backup_roots, &live_roots);
    assert_eq!(support::json(backup_stats.stdout), live_stats);
}

#[test]
fn data_backup_dump_ignores_live_catalog_artifact_while_live_store_is_locked() {
    for catalog_state in ["corrupt", "drifted", "absent"] {
        let (project, dir) = seeded_project(&format!("data-backup-live-catalog-{catalog_state}"));
        let live_dump = support::marrow(&["data", "dump", "--format", "json", &dir]);
        assert_eq!(live_dump.status.code(), Some(0), "{live_dump:?}");
        let live_dump = support::json(live_dump.stdout);

        let archive = support::backup_artifact(&project, &format!("{catalog_state}.mwbackup"));
        let archive_arg = archive.to_str().expect("backup path utf8");
        let catalog_path = project.join("marrow.catalog.json");
        match catalog_state {
            "corrupt" => fs::write(&catalog_path, "{ not catalog json")
                .expect("write corrupt catalog artifact"),
            "drifted" => {
                let accepted = fs::read_to_string(&catalog_path).expect("read catalog artifact");
                let accepted = marrow_catalog::CatalogMetadata::from_json(&accepted)
                    .expect("parse catalog artifact");
                let drifted =
                    marrow_catalog::CatalogMetadata::new(accepted.epoch + 1, accepted.entries)
                        .expect("catalog builds");
                fs::write(
                    &catalog_path,
                    drifted.to_json_pretty().expect("catalog renders"),
                )
                .expect("write drifted catalog artifact");
            }
            "absent" => fs::remove_file(&catalog_path).expect("remove catalog artifact"),
            other => panic!("unknown catalog state {other}"),
        }

        let _writer = TreeStore::open(&project.join(".data").join("marrow.redb"))
            .expect("hold the native writer open");
        let backup_dump = support::marrow(&[
            "data",
            "dump",
            "--backup",
            archive_arg,
            "--format",
            "json",
            &dir,
        ]);

        assert_eq!(
            backup_dump.status.code(),
            Some(0),
            "{catalog_state}: {backup_dump:?}"
        );
        assert_eq!(support::json(backup_dump.stdout), live_dump);
    }
}

#[test]
fn data_backup_flag_usage_is_tight() {
    let cases: &[(&[&str], &str)] = &[
        (
            &[
                "data",
                "dump",
                "--backup",
                "one.mwbackup",
                "--backup",
                "two.mwbackup",
                "proj",
            ],
            "duplicate --backup",
        ),
        (&["data", "roots", "--backup"], "missing value for --backup"),
        (
            &["data", "recover", "--backup", "state.mwbackup", "proj"],
            "unknown data recover option: --backup",
        ),
        (
            &[
                "data",
                "stats",
                "--backup",
                "state.mwbackup",
                "proj",
                "extra",
            ],
            "marrow data stats accepts one project directory",
        ),
    ];

    for (args, expected) in cases {
        let output = support::marrow(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}

#[test]
fn data_dump_of_an_unseeded_project_prints_empty_and_writes_no_records() {
    // A committed-but-unseeded project: its catalog artifact is committed, but no record
    // has been saved. The dump prints the empty placeholder and writes no record of its own.
    let project = native_project("data-dump-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "dump", &dir]);
    let stats = marrow(&["data", "stats", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Render contract: an empty store dump prints a human placeholder.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "(no saved data)\n", "{stdout}");
    // Inspection writes no cells: the store holds zero saved data cells.
    let stats_json = json(stats);
    assert_eq!(stats_json["records"], serde_json::json!(0));
    assert_eq!(stats_json["cells"], serde_json::json!(0));
}

#[test]
fn data_tools_skip_a_pending_member_instead_of_reporting_corruption() {
    // A committed store, then source adds a sparse field not yet applied: its
    // catalog id is unbound. The data tools skip the pending member rather than
    // reporting `store.corruption`, and the committed data still reads.
    let (project, dir) = seeded_project("data-pending-member");
    support::write(
        &project,
        "src/app.mw",
        "module app\n\
         \n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         \x20\x20\x20\x20note: string\n\
         store ^counter(id: int): Counter\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var c: Counter\n\
         \x20\x20\x20\x20c.value = 42\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n",
    );

    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    let dump = marrow(&["data", "dump", &dir]);
    assert_eq!(dump.status.code(), Some(0), "{dump:?}");
    let get = marrow(&["data", "get", &dir, "^counter(1).value"]);
    assert_eq!(get.status.code(), Some(0), "{get:?}");
    assert!(
        String::from_utf8_lossy(&get.stdout).contains("42"),
        "committed data still reads: {get:?}"
    );
}

#[test]
fn data_roots_format_json_emits_a_structured_envelope() {
    let (_project, dir) = seeded_project("data-roots-json");
    let output = marrow(&["data", "roots", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output);
    let project = fs::canonicalize(&dir)
        .expect("canonical project path")
        .display()
        .to_string();
    assert_eq!(value["project"], serde_json::json!(project));
    assert_eq!(value["roots"], serde_json::json!(["counter"]));
    assert_store_snapshot(&value);
}

#[test]
fn data_roots_format_json_reports_canonical_absolute_project_for_relative_path() {
    let (project, _dir) = seeded_project("data-roots-canonical-project");
    let cwd = project.parent().expect("temp project parent");
    let relative = project
        .file_name()
        .expect("temp project name")
        .to_str()
        .expect("utf8 project name");
    let output = support::marrow_in(cwd, &["data", "roots", "--format", "json", relative]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = support::json(output.stdout);
    let expected = fs::canonicalize(&project)
        .expect("canonical project path")
        .display()
        .to_string();
    assert_eq!(value["project"], serde_json::json!(expected));
}

#[test]
fn data_stats_format_json_emits_counts() {
    let (_project, dir) = seeded_project("data-stats-json");
    let output = marrow(&["data", "stats", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output);
    assert_eq!(value["roots"], serde_json::json!(1));
    assert_eq!(value["records"], serde_json::json!(1));
    assert_eq!(value["cells"], serde_json::json!(1));
}

#[test]
fn data_dump_format_json_keys_the_field_cell_array_as_cells() {
    // The dump enumerates field cells `(path, value_b64)`, so the JSON envelope keys
    // them under `cells`, matching the JSONL summary and `data integrity` rather than
    // the entity-scoped `records` count.
    let (_project, dir) = seeded_project("data-dump-json-cells");
    let output = marrow(&["data", "dump", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let dump = json(output);
    assert!(dump.get("records").is_none(), "{dump}");
    let cells = dump["cells"].as_array().expect("cells array");
    assert_eq!(cells.len(), 1, "{dump}");
    assert_eq!(cells[0]["path"], serde_json::json!("^counter(1).value"));
    assert!(cells[0]["value_b64"].is_string(), "{dump}");
}

#[test]
fn data_dump_format_json_of_an_empty_store_keys_an_empty_cells_array() {
    let project = native_project("data-dump-json-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "dump", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let dump = json(output);
    assert!(dump.get("records").is_none(), "{dump}");
    assert_eq!(dump["cells"], serde_json::json!([]));
}

#[test]
fn data_dump_format_jsonl_emits_a_record_then_a_summary() {
    let (_project, dir) = seeded_project("data-dump-jsonl");
    let output = marrow(&["data", "dump", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("record json");
    assert_eq!(record["path"], serde_json::json!("^counter(1).value"));
    assert!(record["value_b64"].is_string(), "{record}");
    let summary: serde_json::Value = serde_json::from_str(lines[1]).expect("summary json");
    assert_eq!(summary["kind"], serde_json::json!("summary"));
    assert_eq!(summary["cells"], serde_json::json!(1));
    assert!(summary.get("records").is_none(), "{summary}");
}

#[test]
fn data_commands_page_through_large_native_store() {
    const RECORDS: usize = 150;

    let project = native_project("data-paged");
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    let identities = (1..=RECORDS).map(|id| vec![SavedKey::Int(id as i64)]);
    write_tree_values(&project, &place, identities, &value_path, b"7");

    let stats = marrow(&["data", "stats", "--format", "json", &dir]);
    let dump = marrow(&["data", "dump", "--format", "jsonl", &dir]);
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(stats.status.code(), Some(0), "{stats:?}");
    let stats_json: serde_json::Value = serde_json::from_slice(&stats.stdout).expect("stats json");
    assert_eq!(stats_json["records"], serde_json::json!(RECORDS));
    assert_eq!(stats_json["cells"], serde_json::json!(RECORDS));

    assert_eq!(dump.status.code(), Some(0), "{dump:?}");
    let dump_stdout = String::from_utf8(dump.stdout).expect("dump utf8");
    let lines = dump_stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), RECORDS + 1);
    let summary: serde_json::Value = serde_json::from_str(lines[RECORDS]).expect("summary json");
    assert_eq!(summary["cells"], serde_json::json!(RECORDS));
    assert!(summary.get("records").is_none(), "{summary}");

    // Paging through every cell finds no integrity problems, asserted as the typed
    // empty problem list rather than the rendered cell-count text. Integrity verifies
    // field cells, so its count is the stored `(path, value)` pairs.
    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    let integrity_json: serde_json::Value =
        serde_json::from_slice(&integrity.stdout).expect("integrity json");
    assert_eq!(integrity_json["cells"], serde_json::json!(RECORDS));
    assert_eq!(integrity_json["problems"], serde_json::json!([]));
}
